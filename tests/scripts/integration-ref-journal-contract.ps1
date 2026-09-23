$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$AuditBaseline = '03ec755eb109975ecc8911f26cc75ee482f32a7a'
$ZeroOid = '0000000000000000000000000000000000000000'
$ZeroHash = '0' * 64
$PlanNames = @(
    '2026-07-12-security-performance-dashboard-remediation-program.md'
    '2026-07-12-security-remediation.md'
    '2026-07-12-performance-remediation.md'
    '2026-07-12-dashboard-ux-remediation.md'
    '2026-07-13-wave0-bootstrap.md'
)
$BootstrapPaths = @(
    'scripts/remediation/control-schema-v2.json'
    'scripts/remediation/publish-plan-set.ps1'
    'scripts/remediation/update-integration-ref.ps1'
    'tests/scripts/plan-set-publisher-contract.ps1'
    'tests/scripts/integration-ref-journal-contract.ps1'
)
$RowKeys = @(
    'schema_version', 'seq', 'phase', 'wave', 'unit', 'attempt_id', 'old_tip', 'new_tip',
    'microplan_sha256', 'lease_sha256', 'recovery_generation', 'recovery_claim_sha256',
    'utc', 'prev_row_sha256', 'row_sha256'
)
$script:UpdaterChildren = [System.Collections.Generic.List[System.Diagnostics.Process]]::new()

if (-not ('DynamoIntegrationContractNative' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

public static class DynamoIntegrationContractNative
{
    const uint FILE_READ_ATTRIBUTES = 0x80;
    const uint FILE_SHARE_READ = 1, FILE_SHARE_WRITE = 2, FILE_SHARE_DELETE = 4;
    const uint OPEN_EXISTING = 3;
    const uint FILE_FLAG_BACKUP_SEMANTICS = 0x02000000;
    const uint FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;

    [StructLayout(LayoutKind.Sequential)]
    struct FILE_ID_INFO {
        public ulong VolumeSerialNumber;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 16)] public byte[] FileId;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr security,
        uint creation, uint flags, IntPtr template);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetFileInformationByHandleEx(SafeFileHandle handle, int infoClass,
        out FILE_ID_INFO info, uint size);

    static string LongPath(string path)
    {
        if (path.StartsWith(@"\\?\", StringComparison.Ordinal)) return path;
        if (path.StartsWith(@"\\", StringComparison.Ordinal)) return @"\\?\UNC\" + path.Substring(2);
        return @"\\?\" + Path.GetFullPath(path);
    }

    public static string GetIdentity(string path)
    {
        using (var handle = CreateFileW(LongPath(path), FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, IntPtr.Zero, OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, IntPtr.Zero)) {
            if (handle.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error(), "CreateFileW failed for contract identity");
            FILE_ID_INFO info;
            if (!GetFileInformationByHandleEx(handle, 18, out info, (uint)Marshal.SizeOf(typeof(FILE_ID_INFO))))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "FileIdInfo failed for contract identity");
            return info.VolumeSerialNumber.ToString("x16") + ":" + BitConverter.ToString(info.FileId).Replace("-", "").ToLowerInvariant();
        }
    }
}
'@
}

function Assert-Contract {
    param(
        [Parameter(Mandatory)][bool]$Condition,
        [Parameter(Mandatory)][string]$Message
    )

    if (-not $Condition) {
        throw "integration-ref-journal-contract: $Message"
    }
}

function Invoke-Git {
    param(
        [Parameter(Mandatory)][string]$WorkingDirectory,
        [Parameter(Mandatory)][string[]]$Arguments,
        [switch]$AllowFailure
    )

    $output = @(& git -C $WorkingDirectory @Arguments 2>&1)
    $exitCode = $LASTEXITCODE
    if (-not $AllowFailure -and $exitCode -ne 0) {
        throw "git -C '$WorkingDirectory' $($Arguments -join ' ') failed ($exitCode): $($output -join [Environment]::NewLine)"
    }

    [pscustomobject]@{ ExitCode = $exitCode; Output = $output }
}

function Get-RepositorySnapshot {
    param([Parameter(Mandatory)][string]$Repository)

    $head = (Invoke-Git -WorkingDirectory $Repository -Arguments @('rev-parse', 'HEAD')).Output[-1].Trim()
    $status = @((Invoke-Git -WorkingDirectory $Repository -Arguments @('status', '--porcelain=v1', '--untracked-files=all')).Output)
    $common = (Invoke-Git -WorkingDirectory $Repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()
    $controlRoot = Join-Path $common 'dynamo-remediation'
    $control = Get-TreeFingerprint -Root $controlRoot
    $refs = @((Invoke-Git -WorkingDirectory $Repository -Arguments @('for-each-ref', '--format=%(refname)%00%(objectname)', 'refs/dynamo-remediation/')).Output)
    $refLogs = Get-TreeFingerprint -Root (Join-Path $common 'logs/refs/dynamo-remediation')
    [pscustomobject]@{
        Head = $head
        Status = $status
        CommonDirectory = [System.IO.Path]::GetFullPath($common)
        Control = $control
        Refs = $refs
        RefLogs = $refLogs
    }
}

function Assert-RepositoryUnchanged {
    param([Parameter(Mandatory)]$Before, [Parameter(Mandatory)]$After)

    Assert-Contract ($Before.Head -ceq $After.Head) 'the real repository HEAD changed'
    Assert-Contract ((@($Before.Status) -join "`n") -ceq (@($After.Status) -join "`n")) 'the real repository status changed'
    Assert-Contract ($Before.CommonDirectory -ceq $After.CommonDirectory) 'the real Git common directory changed'
    Assert-Contract ((@($Before.Control) -join "`n") -ceq (@($After.Control) -join "`n")) 'the real Git common-dir control state changed'
    Assert-Contract ((@($Before.Refs) -join "`n") -ceq (@($After.Refs) -join "`n")) 'the real remediation refs changed'
    Assert-Contract ((@($Before.RefLogs) -join "`n") -ceq (@($After.RefLogs) -join "`n")) 'the real remediation reflogs changed'
}

function Get-TreeFingerprint {
    param([Parameter(Mandatory)][string]$Root)

    if (-not (Test-Path -LiteralPath $Root)) {
        return @('<absent>')
    }

    $fullRoot = [System.IO.Path]::GetFullPath($Root)
    $pending = [System.Collections.Generic.Stack[string]]::new()
    $records = [System.Collections.Generic.List[string]]::new()
    $pending.Push($fullRoot)
    while ($pending.Count -gt 0) {
        $path = $pending.Pop()
        $item = Get-Item -LiteralPath $path -Force
        $relative = if ($path -ceq $fullRoot) { '<root>' } else { [System.IO.Path]::GetRelativePath($fullRoot, $path).Replace('\', '/') }
        $reparse = ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0
        $identity = Get-NativePathIdentity $path
        $owner = Get-PathOwnerDescriptor $path
        $acl = Get-PathAclDescriptor $path
        if ($item.PSIsContainer) {
            $records.Add("D|$relative|reparse=$reparse|identity=$identity|owner=$owner|acl=$acl")
            if (-not $reparse) {
                foreach ($child in @(Get-ChildItem -LiteralPath $path -Force | Sort-Object FullName -Descending)) { $pending.Push($child.FullName) }
            }
        }
        elseif ($reparse) {
            $records.Add("F|$relative|reparse=True|identity=$identity|owner=$owner|acl=$acl")
        }
        else {
            $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
            $records.Add("F|$relative|reparse=False|length=$($item.Length)|hash=$hash|identity=$identity|owner=$owner|acl=$acl")
        }
    }
    @($records | Sort-Object -CaseSensitive)
}

function Get-OwnedRootIdentity {
    param([Parameter(Mandatory)][string]$Root)

    $item = Get-Item -LiteralPath $Root -Force
    Assert-Contract (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) 'suite root is a reparse point'
    $full = [System.IO.Path]::GetFullPath($item.FullName)
    if ($IsWindows) {
        return "$full|$([DynamoIntegrationContractNative]::GetIdentity($full))"
    }
    $native = @(& stat -c '%d:%i' -- $full 2>&1)
    Assert-Contract ($LASTEXITCODE -eq 0 -and $native.Count -eq 1) 'could not read suite-root native inode identity'
    "$full|$($native[0].Trim())"
}

function Copy-FileExact {
    param(
        [Parameter(Mandatory)][string]$Source,
        [Parameter(Mandatory)][string]$Destination
    )

    $null = New-Item -ItemType Directory -Path (Split-Path -Parent $Destination) -Force
    [System.IO.File]::WriteAllBytes($Destination, [System.IO.File]::ReadAllBytes($Source))
}

function Invoke-Publisher {
    param(
        [Parameter(Mandatory)][string]$Repository,
        [Parameter(Mandatory)][string]$EvidenceRoot
    )

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = (Get-Command pwsh -ErrorAction Stop).Source
    $psi.WorkingDirectory = $Repository
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    foreach ($name in @('GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES', 'GIT_INDEX_FILE', 'GIT_NAMESPACE', 'GIT_REPLACE_REF_BASE', 'GIT_CEILING_DIRECTORIES', 'GIT_DISCOVERY_ACROSS_FILESYSTEM')) { $null = $psi.Environment.Remove($name) }
    $psi.Environment['DYNAMO_REMEDIATION_EVIDENCE_ROOT'] = $EvidenceRoot
    foreach ($argument in @('-NoProfile', '-File', (Join-Path $Repository 'scripts/remediation/publish-plan-set.ps1'))) { $null = $psi.ArgumentList.Add($argument) }
    $process = [System.Diagnostics.Process]::Start($psi)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(60000)) {
        $process.Kill($true)
        $process.WaitForExit()
        $null = $stdout.GetAwaiter().GetResult()
        $null = $stderr.GetAwaiter().GetResult()
        throw 'integration-ref-journal-contract: publisher child exceeded 60-second timeout and was killed'
    }
    $output = @(($stdout.GetAwaiter().GetResult() -split '\r?\n') + ($stderr.GetAwaiter().GetResult() -split '\r?\n') | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $exitCode = $process.ExitCode
    Assert-Contract ($exitCode -eq 0) "publisher fixture setup failed: $($output -join [Environment]::NewLine)"
    $lines = @($output | ForEach-Object { "$_" } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    Assert-Contract ($lines.Count -eq 1) 'publisher fixture did not return one JSON handoff'
    $lines[0] | ConvertFrom-Json -ErrorAction Stop
}

function Invoke-Updater {
    param(
        [Parameter(Mandatory)][string]$Repository,
        [Parameter(Mandatory)][string[]]$Arguments,
        [hashtable]$Environment = @{}
    )

    Complete-Updater -Running (Start-Updater -Repository $Repository -Arguments $Arguments -Environment $Environment)
}

function Start-Updater {
    param(
        [Parameter(Mandatory)][string]$Repository,
        [Parameter(Mandatory)][string[]]$Arguments,
        [hashtable]$Environment = @{}
    )

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = (Get-Command pwsh -ErrorAction Stop).Source
    $psi.WorkingDirectory = $Repository
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    foreach ($name in @('GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES', 'GIT_INDEX_FILE', 'GIT_NAMESPACE', 'GIT_REPLACE_REF_BASE', 'GIT_CEILING_DIRECTORIES', 'GIT_DISCOVERY_ACROSS_FILESYSTEM')) {
        $null = $psi.Environment.Remove($name)
    }
    $psi.Environment['DYNAMO_REMEDIATION_TEST_MODE'] = '1'
    foreach ($argument in @('-NoProfile', '-File', (Join-Path $Repository 'scripts/remediation/update-integration-ref.ps1')) + $Arguments) {
        $null = $psi.ArgumentList.Add($argument)
    }
    foreach ($key in $Environment.Keys) {
        $psi.Environment[$key] = [string]$Environment[$key]
    }
    $process = [System.Diagnostics.Process]::Start($psi)
    $script:UpdaterChildren.Add($process)
    [pscustomobject]@{
        Process = $process
        StandardOutput = $process.StandardOutput.ReadToEndAsync()
        StandardError = $process.StandardError.ReadToEndAsync()
    }
}

function Complete-Updater {
    param([Parameter(Mandatory)]$Running)

    try {
        if (-not $Running.Process.WaitForExit(60000)) {
            $Running.Process.Kill($true)
            $Running.Process.WaitForExit()
            $null = $Running.StandardOutput.GetAwaiter().GetResult()
            $null = $Running.StandardError.GetAwaiter().GetResult()
            throw 'integration-ref-journal-contract: updater child exceeded 60-second timeout and was killed'
        }
        [pscustomobject]@{
            ExitCode = $Running.Process.ExitCode
            Output = @($Running.StandardOutput.GetAwaiter().GetResult(), $Running.StandardError.GetAwaiter().GetResult()) |
                Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
        }
    }
    finally {
        $null = $script:UpdaterChildren.Remove($Running.Process)
        $Running.Process.Dispose()
    }
}

function Assert-Failed {
    param([Parameter(Mandatory)]$Invocation, [Parameter(Mandatory)][string]$Case)

    Assert-Contract ($Invocation.ExitCode -ne 0) "$Case unexpectedly succeeded"
    $text = @($Invocation.Output | ForEach-Object { "$_" }) -join "`n"
    Assert-Contract ($text -notmatch '(?i)(mongodb(?:\+srv)?://|authorization:\s*bearer|client_secret|access_token|refresh_token|cookie:)') "$Case leaked credential-shaped output"
}

function New-CommitObject {
    param(
        [Parameter(Mandatory)][string]$Repository,
        [Parameter(Mandatory)][string]$Parent,
        [Parameter(Mandatory)][string]$Message
    )

    $tree = (Invoke-Git -WorkingDirectory $Repository -Arguments @('rev-parse', "$Parent^{tree}")).Output[-1].Trim()
    $saved = @{}
    $variables = @{
        GIT_AUTHOR_NAME = 'Dynamo Contract Fixture'
        GIT_AUTHOR_EMAIL = 'fixture.invalid@localhost'
        GIT_COMMITTER_NAME = 'Dynamo Contract Fixture'
        GIT_COMMITTER_EMAIL = 'fixture.invalid@localhost'
    }
    try {
        foreach ($key in $variables.Keys) {
            $saved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
            [Environment]::SetEnvironmentVariable($key, $variables[$key], 'Process')
        }
        $result = @($Message | git -C $Repository commit-tree $tree -p $Parent 2>&1)
        $exitCode = $LASTEXITCODE
    }
    finally {
        foreach ($key in $saved.Keys) {
            [Environment]::SetEnvironmentVariable($key, $saved[$key], 'Process')
        }
    }
    Assert-Contract ($exitCode -eq 0) "commit-tree failed: $($result -join [Environment]::NewLine)"
    $commit = $result[-1].Trim()
    Assert-Contract ($commit -match '^[0-9a-f]{40}$') 'generated fixture commit is malformed'
    $commit
}

function Get-WaveRef {
    param([Parameter(Mandatory)][string]$Repository, [Parameter(Mandatory)][string]$Wave)

    $refName = "refs/dynamo-remediation/$Wave/integration"
    $symbolic = Invoke-Git -WorkingDirectory $Repository -Arguments @('symbolic-ref', '-q', $refName) -AllowFailure
    Assert-Contract ($symbolic.ExitCode -in @(0,1)) "could not classify wave ref as direct/symbolic: $refName"
    Assert-Contract ($symbolic.ExitCode -eq 1) "wave ref is symbolic instead of direct: $refName"
    $exists = Invoke-Git -WorkingDirectory $Repository -Arguments @('show-ref', '--exists', $refName) -AllowFailure
    if ($exists.ExitCode -eq 2) { return $null }
    Assert-Contract ($exists.ExitCode -eq 0) "could not classify wave ref existence: $refName"
    $result = Invoke-Git -WorkingDirectory $Repository -Arguments @('show-ref', '--verify', '--hash', $refName) -AllowFailure
    Assert-Contract ($result.ExitCode -eq 0 -and $result.Output.Count -eq 1 -and $result.Output[0].Trim() -cmatch '^[0-9a-f]{40}$') "wave ref is not an exact direct 40-hex ref: $refName"
    $result.Output[0].Trim()
}

function Get-WaveStateRoot {
    param([Parameter(Mandatory)]$Fixture, [Parameter(Mandatory)][string]$Wave)

    Join-Path $Fixture.CommonDirectory "dynamo-remediation/integration-state-v1/$($Fixture.ExecutionBaseline)/$($Fixture.PlanSetSha256)/waves/$Wave"
}

function Get-DomainHash {
    param([Parameter(Mandatory)][string]$Domain, [Parameter(Mandatory)][string]$CanonicalJson)

    $prefix = [System.Text.UTF8Encoding]::new($false).GetBytes("$Domain`0")
    $body = [System.Text.UTF8Encoding]::new($false).GetBytes($CanonicalJson)
    $bytes = [byte[]]::new($prefix.Length + $body.Length)
    [System.Buffer]::BlockCopy($prefix, 0, $bytes, 0, $prefix.Length)
    [System.Buffer]::BlockCopy($body, 0, $bytes, $prefix.Length, $body.Length)
    [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
}

function ConvertFrom-JsonElementToMinimalText {
    param([Parameter(Mandatory)][System.Text.Json.JsonElement]$Element)

    $stream = [System.IO.MemoryStream]::new()
    $writer = [System.Text.Json.Utf8JsonWriter]::new($stream, [System.Text.Json.JsonWriterOptions]@{ Indented = $false })
    try {
        $Element.WriteTo($writer)
        $writer.Flush()
        [System.Text.UTF8Encoding]::new($false, $true).GetString($stream.ToArray())
    }
    finally {
        $writer.Dispose()
        $stream.Dispose()
    }
}

function Assert-CanonicalRows {
    param(
        [Parameter(Mandatory)][string]$WaveRoot,
        [Parameter(Mandatory)][string]$Repository
    )

    $rowsRoot = Join-Path $WaveRoot 'journal/rows'
    Assert-Contract (Test-Path -LiteralPath $rowsRoot -PathType Container) 'immutable journal/rows directory is missing'
    $rows = @(Get-ChildItem -LiteralPath $rowsRoot -File | Sort-Object Name)
    Assert-Contract ($rows.Count -gt 0) 'immutable row directory is empty'
    $expectedPrevious = $ZeroHash
    $seenAttempts = @{}
    $wave = Split-Path -Leaf $WaveRoot
    $phaseSlugs = @{
        Intent = 'intent'
        Committed = 'committed'
        RecoveredCommitted = 'recovered-committed'
        AbortedNoRefChange = 'aborted-no-ref-change'
    }
    $rowObjects = @()
    for ($rowIndex = 0; $rowIndex -lt $rows.Count; $rowIndex++) {
        $rowFile = $rows[$rowIndex]
        Assert-Contract ($rowFile.Name -match '^\d{20}-(intent|committed|recovered-committed|aborted-no-ref-change)\.json$') "noncanonical final row leaf: $($rowFile.Name)"
        $bytes = [System.IO.File]::ReadAllBytes($rowFile.FullName)
        Assert-Contract (-not ($bytes.Length -ge 3 -and $bytes[0] -eq 0xef -and $bytes[1] -eq 0xbb -and $bytes[2] -eq 0xbf)) "row contains a forbidden UTF-8 BOM: $($rowFile.Name)"
        $raw = [System.Text.UTF8Encoding]::new($false, $true).GetString($bytes)
        Assert-Contract (-not $raw.Contains("`r") -and $raw.EndsWith("`n", [System.StringComparison]::Ordinal) -and -not $raw.Substring(0, $raw.Length - 1).Contains("`n")) "row must be compact with exactly one terminal LF: $($rowFile.Name)"
        $rowDocument = [System.Text.Json.JsonDocument]::Parse($raw)
        try {
            $rawNames = @($rowDocument.RootElement.EnumerateObject() | ForEach-Object Name)
            $uniqueNames = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
            foreach ($rawName in $rawNames) { Assert-Contract ($uniqueNames.Add($rawName)) "row contains duplicate JSON key ${rawName}: $($rowFile.Name)" }
            $normalizedRow = (ConvertFrom-JsonElementToMinimalText $rowDocument.RootElement) + "`n"
        }
        finally { $rowDocument.Dispose() }
        Assert-Contract ($normalizedRow -ceq $raw) "row is not its canonical minimal JSON serialization: $($rowFile.Name)"
        $row = $raw | ConvertFrom-Json -ErrorAction Stop
        $keys = @($row.PSObject.Properties.Name)
        Assert-Contract (($keys -join ',') -ceq ($RowKeys -join ',')) "row key order/schema mismatch: $($rowFile.Name)"
        Assert-Contract ($row.schema_version -eq 1 -and $row.seq -is [long]) "row schema/sequence type mismatch: $($rowFile.Name)"
        Assert-Contract ([long]$row.seq -eq ($rowIndex + 1)) "row sequence is not contiguous: $($rowFile.Name)"
        Assert-Contract ($phaseSlugs.ContainsKey([string]$row.phase)) "unknown row phase: $($row.phase)"
        $expectedName = '{0:d20}-{1}.json' -f [long]$row.seq, $phaseSlugs[[string]$row.phase]
        Assert-Contract ($rowFile.Name -ceq $expectedName) "filename does not match row sequence/phase: $($rowFile.Name)"
        Assert-Contract ($row.wave -ceq $wave -and $row.wave -match '^[a-z][a-z0-9-]{0,63}$') "row wave mismatch: $($rowFile.Name)"
        Assert-Contract ($row.unit -match '^[a-z][a-z0-9-]{0,63}$') "row unit is noncanonical: $($rowFile.Name)"
        Assert-Contract ($row.old_tip -match '^[0-9a-f]{40}$' -and $row.new_tip -match '^[0-9a-f]{40}$') "row tips are malformed: $($rowFile.Name)"
        Assert-Contract ($row.microplan_sha256 -match '^[0-9a-f]{64}$') "row microplan hash is malformed: $($rowFile.Name)"
        Assert-Contract ($raw -match '"utc":"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}Z"') "row UTC is noncanonical: $($rowFile.Name)"
        Assert-Contract ($row.prev_row_sha256 -ceq $expectedPrevious) "broken previous-row hash: $($rowFile.Name)"
        Assert-Contract ($row.attempt_id -match '^[0-9a-f]{32}$') "attempt ID is malformed: $($rowFile.Name)"
        Assert-Contract ($row.lease_sha256 -match '^[0-9a-f]{64}$') "lease hash is malformed: $($rowFile.Name)"
        Assert-Contract ($row.recovery_claim_sha256 -match '^[0-9a-f]{64}$') "claim hash is malformed: $($rowFile.Name)"
        if ([int]$row.recovery_generation -eq 0) {
            Assert-Contract ($row.recovery_claim_sha256 -ceq $ZeroHash) 'normal row has a nonzero recovery claim hash'
        }
        else {
            Assert-Contract ([int]$row.recovery_generation -ge 1 -and $row.recovery_claim_sha256 -ne $ZeroHash) 'recovery row has no generation-bound claim hash'
        }
        $suffix = ',"row_sha256":"(?<hash>[0-9a-f]{64})"}$'
        $canonical = $raw.Substring(0, $raw.Length - 1)
        $match = [regex]::Match($canonical, $suffix)
        Assert-Contract ($match.Success) "row_sha256 is not the final canonical property: $($rowFile.Name)"
        $preimage = $canonical.Substring(0, $match.Index) + "}`n"
        $actualHash = Get-DomainHash -Domain 'dynamo-integration-row-v1' -CanonicalJson $preimage
        Assert-Contract ($actualHash -ceq $row.row_sha256) "row hash mismatch: $($rowFile.Name)"
        $expectedPrevious = $row.row_sha256
        $rowObjects += $row
        if (-not $seenAttempts.ContainsKey($row.attempt_id)) { $seenAttempts[$row.attempt_id] = @() }
        $seenAttempts[$row.attempt_id] = @($seenAttempts[$row.attempt_id]) + $row.phase
    }
    foreach ($attempt in $seenAttempts.Keys) {
        $attemptRows = @($rowObjects | Where-Object { $_.attempt_id -ceq $attempt })
        $phases = @($attemptRows | ForEach-Object { $_.phase })
        $intents = @($attemptRows | Where-Object { $_.phase -eq 'Intent' })
        $terminals = @($attemptRows | Where-Object { $_.phase -in @('Committed', 'RecoveredCommitted', 'AbortedNoRefChange') })
        Assert-Contract ($attemptRows.Count -eq 2 -and $intents.Count -eq 1 -and $terminals.Count -eq 1) "completed attempt lacks exact Intent+terminal pair: $attempt"
        Assert-Contract ([long]$intents[0].seq -lt [long]$terminals[0].seq) "terminal does not follow Intent for attempt $attempt"
        foreach ($property in @('wave', 'unit', 'attempt_id', 'old_tip', 'new_tip', 'microplan_sha256', 'lease_sha256')) {
            Assert-Contract ($intents[0].$property -ceq $terminals[0].$property) "attempt rows disagree on ${property}: $attempt"
        }
    }
    $tail = $rowObjects[-1]
    Assert-Contract ($tail.phase -in @('Committed', 'RecoveredCommitted', 'AbortedNoRefChange')) 'journal tail is not terminal'
    $expectedRef = if ($tail.phase -eq 'AbortedNoRefChange') { $tail.old_tip } else { $tail.new_tip }
    $actualRef = Get-WaveRef -Repository $Repository -Wave $wave
    if ($null -eq $actualRef) { $actualRef = $ZeroOid }
    Assert-Contract ($actualRef -ceq $expectedRef) "terminal journal tail disagrees with direct ref for $wave (phase=$($tail.phase), expected=$expectedRef, actual=$actualRef)"
}

function Get-RefLogCount {
    param([Parameter(Mandatory)][string]$Repository, [Parameter(Mandatory)][string]$Wave)

    $result = Invoke-Git -WorkingDirectory $Repository -Arguments @('reflog', 'show', '--format=%H', "refs/dynamo-remediation/$Wave/integration") -AllowFailure
    if ($result.ExitCode -ne 0) { return 0 }
    @($result.Output | Where-Object { $_ -match '^[0-9a-f]{40}$' }).Count
}

function Assert-HashedCanonicalObject {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string[]]$Keys,
        [Parameter(Mandatory)][string]$HashProperty,
        [Parameter(Mandatory)][string]$Domain
    )

    $raw = [System.Text.UTF8Encoding]::new($false, $true).GetString([System.IO.File]::ReadAllBytes($Path))
    $rawBytes = [System.IO.File]::ReadAllBytes($Path)
    Assert-Contract (-not ($rawBytes.Length -ge 3 -and $rawBytes[0] -eq 0xef -and $rawBytes[1] -eq 0xbb -and $rawBytes[2] -eq 0xbf)) "persisted object contains a forbidden UTF-8 BOM: $Path"
    Assert-Contract (-not $raw.Contains("`r") -and $raw.EndsWith("`n", [System.StringComparison]::Ordinal) -and -not $raw.Substring(0, $raw.Length - 1).Contains("`n")) "persisted object is not compact+one-LF: $Path"
    $document = [System.Text.Json.JsonDocument]::Parse($raw)
    try {
        $rawNames = @($document.RootElement.EnumerateObject() | ForEach-Object Name)
        $uniqueNames = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
        foreach ($rawName in $rawNames) { Assert-Contract ($uniqueNames.Add($rawName)) "persisted object contains duplicate JSON key ${rawName}: $Path" }
        $normalized = (ConvertFrom-JsonElementToMinimalText $document.RootElement) + "`n"
    }
    finally { $document.Dispose() }
    Assert-Contract ($normalized -ceq $raw) "persisted object is not its canonical minimal JSON serialization: $Path"
    $object = $raw | ConvertFrom-Json -ErrorAction Stop
    Assert-Contract ((@($object.PSObject.Properties.Name) -join ',') -ceq ($Keys -join ',')) "persisted object schema mismatch: $Path"
    foreach ($timestampProperty in @('utc','created_at')) {
        if ($Keys -contains $timestampProperty) {
            Assert-Contract ($raw -match ('"' + $timestampProperty + '":"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}Z"')) "persisted object $timestampProperty is noncanonical: $Path"
        }
    }
    $canonical = $raw.Substring(0, $raw.Length - 1)
    $pattern = [string]::Concat(',"', [regex]::Escape($HashProperty), '":"(?<hash>[0-9a-f]{64})"}$')
    $match = [regex]::Match($canonical, $pattern)
    Assert-Contract ($match.Success) "persisted object self-hash is not final: $Path"
    $preimage = $canonical.Substring(0, $match.Index) + "}`n"
    Assert-Contract ((Get-DomainHash -Domain $Domain -CanonicalJson $preimage) -ceq $object.$HashProperty) "persisted object self-hash mismatch: $Path"
    $object
}

function Assert-GlobalJournalContinuityOracle {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][string]$Wave,
        [Parameter(Mandatory)][object[]]$Rows
    )

    Assert-Contract ($Rows.Count -gt 0 -and $Rows.Count % 2 -eq 0) "$Wave completed journal is not a nonempty sequence of operation pairs"
    $expectedOld = $ZeroOid
    for ($index = 0; $index -lt $Rows.Count; $index += 2) {
        $intent = $Rows[$index]
        $terminal = $Rows[$index + 1]
        Assert-Contract ($intent.phase -ceq 'Intent' -and $terminal.phase -cne 'Intent' -and $terminal.attempt_id -ceq $intent.attempt_id) "$Wave journal operation pair ordering is invalid"
        Assert-Contract ($intent.old_tip -ceq $expectedOld) "$Wave journal does not continue from the prior terminal result"
        if ($index -eq 0) {
            Assert-Contract ($intent.old_tip -ceq $ZeroOid -and $intent.new_tip -ceq $Fixture.ExecutionBaseline) "$Wave first journal pair is not the immutable baseline anchor"
        }
        $expectedOld = if ($terminal.phase -in @('Committed','RecoveredCommitted')) { [string]$terminal.new_tip } else { [string]$terminal.old_tip }
    }
    $actualRef = Get-WaveRef -Repository $Fixture.Repository -Wave $Wave
    if ($expectedOld -ceq $ZeroOid) {
        Assert-Contract ($null -eq $actualRef) "$Wave zero-result journal tail retained an authoritative ref"
    }
    else {
        Assert-Contract ($actualRef -ceq $expectedOld) "$Wave direct authoritative ref contradicts the completed journal tail result"
    }
}

function Assert-StateCompletion {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][string]$Wave,
        [ValidateSet('Normal', 'Recovered', 'Either')][string]$Expected = 'Either'
    )

    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $Wave
    $expectedWaveNames = @('journal','leases','recovery')
    Assert-Contract (((Get-ChildItem -LiteralPath $waveRoot -Force | ForEach-Object Name | Sort-Object -CaseSensitive) -join "`n") -ceq (($expectedWaveNames | Sort-Object -CaseSensitive) -join "`n")) "$Wave root inventory is not exact"
    $journalRoot = Join-Path $waveRoot 'journal'
    $recoveryRoot = Join-Path $waveRoot 'recovery'
    Assert-Contract (((Get-ChildItem -LiteralPath $journalRoot -Force | ForEach-Object Name | Sort-Object -CaseSensitive) -join "`n") -ceq "orphans`nrows`ntmp") "$Wave journal directory inventory is not exact"
    Assert-Contract (((Get-ChildItem -LiteralPath $recoveryRoot -Force | ForEach-Object Name | Sort-Object -CaseSensitive) -join "`n") -ceq "archives`nclosed") "$Wave recovery directory inventory is not exact"
    $fixedDirectories = @(
        $waveRoot,
        $journalRoot,
        (Join-Path $journalRoot 'rows'),
        (Join-Path $journalRoot 'tmp'),
        (Join-Path $journalRoot 'orphans'),
        (Join-Path $waveRoot 'leases'),
        $recoveryRoot,
        (Join-Path $recoveryRoot 'archives'),
        (Join-Path $recoveryRoot 'closed')
    )
    foreach ($fixedDirectory in $fixedDirectories) {
        $fixedItem = Get-Item -LiteralPath $fixedDirectory -Force
        Assert-Contract ($fixedItem.PSIsContainer -and ($fixedItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) "$Wave fixed directory is missing, not a directory, or a reparse point: $fixedDirectory"
    }
    foreach ($leafDirectory in @((Join-Path $journalRoot 'rows'),(Join-Path $journalRoot 'tmp'),(Join-Path $journalRoot 'orphans'),(Join-Path $waveRoot 'leases'),(Join-Path $recoveryRoot 'archives'),(Join-Path $recoveryRoot 'closed'))) {
        $invalidLeaf = @(Get-ChildItem -LiteralPath $leafDirectory -Force | Where-Object { $_.PSIsContainer -or ($_.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 })
        Assert-Contract ($invalidLeaf.Count -eq 0) "$Wave fixed leaf directory contains an extra directory or reparse entry: $leafDirectory"
    }
    Assert-Contract (-not (Test-Path -LiteralPath (Join-Path $waveRoot 'active-ref-update.lock'))) "$Wave retained an active integration lease"
    Assert-Contract (-not (Test-Path -LiteralPath (Join-Path $waveRoot 'recovery/active-recovery.claim'))) "$Wave retained an active recovery claim"
    Assert-Contract (@(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'journal/tmp') -Force).Count -eq 0) "$Wave retained an unarchived journal temp"
    $rowFiles = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'journal/rows') -File | Sort-Object Name)
    $rowObjects = @($rowFiles | ForEach-Object { Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json -ErrorAction Stop })
    Assert-GlobalJournalContinuityOracle -Fixture $Fixture -Wave $Wave -Rows $rowObjects
    $attemptIds = @($rowObjects | ForEach-Object attempt_id | Sort-Object -Unique -CaseSensitive)
    $leaseKeys = @('schema_version', 'execution_baseline', 'plan_set_sha256', 'wave', 'ref_name', 'unit', 'attempt_id', 'old_tip', 'new_tip', 'microplan_sha256', 'owner', 'expected_seq', 'expected_tail_sha256', 'created_at', 'lease_sha256')
    $claimKeys = @('schema_version', 'execution_baseline', 'plan_set_sha256', 'wave', 'ref_name', 'attempt_id', 'generation', 'lease_sha256', 'snapshot_tail_sha256', 'snapshot_ref_tip', 'prior_claim_sha256', 'owner', 'created_at', 'claim_sha256')
    $ownerKeys = @('machine_identity_sha256', 'pid', 'process_start_identity', 'attempt_id', 'git_common_dir_identity_sha256')
    $closedLeaseFiles = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'leases') -File -Force | Sort-Object Name)
    Assert-Contract ($closedLeaseFiles.Count -eq $attemptIds.Count) "$Wave does not contain exactly one closed lease per journal attempt"
    $leasesByAttempt = @{}
    foreach ($closedLeaseFile in $closedLeaseFiles) {
        Assert-Contract ($closedLeaseFile.Name -match '^closed-ref-update\.(?<attempt>[0-9a-f]{32})\.(?<hash>[0-9a-f]{64})\.lock$') "$Wave contains a malformed/extra closed lease filename"
        $closedLease = Assert-HashedCanonicalObject -Path $closedLeaseFile.FullName -Keys $leaseKeys -HashProperty 'lease_sha256' -Domain 'dynamo-integration-lease-v1'
        Assert-Contract ($closedLease.attempt_id -ceq $Matches['attempt'] -and $closedLease.lease_sha256 -ceq $Matches['hash']) "$Wave closed lease filename/content tuple mismatch"
        Assert-Contract ((@($closedLease.owner.PSObject.Properties.Name) -join ',') -ceq ($ownerKeys -join ',')) "$Wave closed lease owner schema mismatch"
        Assert-Contract ($closedLease.attempt_id -in $attemptIds -and -not $leasesByAttempt.ContainsKey([string]$closedLease.attempt_id)) "$Wave contains an extra or duplicate closed lease"
        $leasesByAttempt[[string]$closedLease.attempt_id] = $closedLease
    }
    $closedClaimRoot = Join-Path $waveRoot 'recovery/closed'
    $archiveClaimRoot = Join-Path $waveRoot 'recovery/archives'
    $allClosedClaimFiles = @(Get-ChildItem -LiteralPath $closedClaimRoot -File -Force | Sort-Object Name)
    $allArchiveClaimFiles = @(Get-ChildItem -LiteralPath $archiveClaimRoot -File -Force | Sort-Object Name)
    foreach ($attemptId in $attemptIds) {
        $attemptRows = @($rowObjects | Where-Object { $_.attempt_id -ceq $attemptId })
        $attemptLease = $leasesByAttempt[$attemptId]
        foreach ($attemptRow in $attemptRows) {
            foreach ($property in @('wave','unit','attempt_id','old_tip','new_tip','microplan_sha256','lease_sha256')) {
                Assert-Contract ($attemptRow.$property -ceq $attemptLease.$property) "$Wave lease/journal mismatch at $property for $attemptId"
            }
        }
        $attemptArchives = @($allArchiveClaimFiles | Where-Object { $_.Name -like "recovery-claim.$attemptId.*" } | Sort-Object Name)
        $attemptClosedClaims = @($allClosedClaimFiles | Where-Object { $_.Name -like "closed-recovery.$attemptId.*" })
        Assert-Contract ($attemptClosedClaims.Count -le 1) "$Wave contains multiple closed claims for $attemptId"
        $priorClaim = $ZeroHash
        [long]$expectedGeneration = 1
        $claimTuples = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
        foreach ($archiveFile in $attemptArchives) {
            Assert-Contract ($archiveFile.Name -match '^recovery-claim\.(?<attempt>[0-9a-f]{32})\.g(?<generation>[0-9]{10})\.(?<hash>[0-9a-f]{64})\.claim$') "$Wave contains a malformed recovery claim archive"
            $archiveClaim = Assert-HashedCanonicalObject -Path $archiveFile.FullName -Keys $claimKeys -HashProperty 'claim_sha256' -Domain 'dynamo-recovery-claim-v1'
            Assert-Contract ((@($archiveClaim.owner.PSObject.Properties.Name) -join ',') -ceq ($ownerKeys -join ',')) "$Wave archived claim owner schema mismatch"
            Assert-Contract ($archiveClaim.attempt_id -ceq $attemptId -and [long]$archiveClaim.generation -eq $expectedGeneration -and $archiveClaim.claim_sha256 -ceq $Matches['hash'] -and $archiveClaim.prior_claim_sha256 -ceq $priorClaim -and $archiveClaim.lease_sha256 -ceq $attemptLease.lease_sha256) "$Wave archived claim chain/tuple mismatch for $attemptId"
            $null = $claimTuples.Add(("{0}|{1}" -f [long]$archiveClaim.generation, [string]$archiveClaim.claim_sha256))
            $priorClaim = [string]$archiveClaim.claim_sha256
            $expectedGeneration++
        }
        if ($attemptClosedClaims.Count -eq 1) {
            $closedClaimFile = $attemptClosedClaims[0]
            Assert-Contract ($closedClaimFile.Name -match '^closed-recovery\.(?<attempt>[0-9a-f]{32})\.g(?<generation>[0-9]{10})\.(?<hash>[0-9a-f]{64})\.claim$') "$Wave contains a malformed closed recovery claim"
            $closedAttemptClaim = Assert-HashedCanonicalObject -Path $closedClaimFile.FullName -Keys $claimKeys -HashProperty 'claim_sha256' -Domain 'dynamo-recovery-claim-v1'
            Assert-Contract ((@($closedAttemptClaim.owner.PSObject.Properties.Name) -join ',') -ceq ($ownerKeys -join ',')) "$Wave closed claim owner schema mismatch"
            Assert-Contract ($closedAttemptClaim.attempt_id -ceq $attemptId -and [long]$closedAttemptClaim.generation -eq $expectedGeneration -and $closedAttemptClaim.claim_sha256 -ceq $Matches['hash'] -and $closedAttemptClaim.prior_claim_sha256 -ceq $priorClaim -and $closedAttemptClaim.lease_sha256 -ceq $attemptLease.lease_sha256) "$Wave closed claim does not exactly extend the archive chain for $attemptId"
            $null = $claimTuples.Add(("{0}|{1}" -f [long]$closedAttemptClaim.generation, [string]$closedAttemptClaim.claim_sha256))
        }
        else {
            Assert-Contract ($attemptArchives.Count -eq 0) "$Wave has an incomplete archive-only recovery chain for $attemptId"
        }
        $attemptTerminal = @($attemptRows | Where-Object { $_.phase -ne 'Intent' })[0]
        if ([long]$attemptTerminal.recovery_generation -gt 0) {
            Assert-Contract ($claimTuples.Contains(("{0}|{1}" -f [long]$attemptTerminal.recovery_generation, [string]$attemptTerminal.recovery_claim_sha256))) "$Wave terminal does not resolve to its exact claim generation/hash for $attemptId"
        }
    }
    foreach ($claimFile in @($allClosedClaimFiles) + @($allArchiveClaimFiles)) {
        Assert-Contract (@($attemptIds | Where-Object { $claimFile.Name.Contains(('.' + $_ + '.'), [System.StringComparison]::Ordinal) }).Count -eq 1) "$Wave contains a claim for an unknown attempt"
    }
    $tail = Get-Content -LiteralPath $rowFiles[-1].FullName -Raw | ConvertFrom-Json -ErrorAction Stop
    $leasePath = Join-Path $waveRoot "leases/closed-ref-update.$($tail.attempt_id).$($tail.lease_sha256).lock"
    Assert-Contract (Test-Path -LiteralPath $leasePath -PathType Leaf) "$Wave terminal has no exact closed lease"
    $lease = Assert-HashedCanonicalObject -Path $leasePath -Keys $leaseKeys -HashProperty 'lease_sha256' -Domain 'dynamo-integration-lease-v1'
    Assert-Contract ((@($lease.owner.PSObject.Properties.Name) -join ',') -ceq ($ownerKeys -join ',')) "$Wave lease owner schema mismatch"
    foreach ($property in @('wave', 'unit', 'attempt_id', 'old_tip', 'new_tip', 'microplan_sha256', 'lease_sha256')) {
        Assert-Contract ($lease.$property -ceq $tail.$property) "$Wave closed lease disagrees with terminal on $property"
    }
    Assert-Contract ($lease.ref_name -ceq "refs/dynamo-remediation/$Wave/integration") "$Wave lease ref_name mismatch"

    $closedRoot = Join-Path $waveRoot 'recovery/closed'
    $closedClaims = @(if (Test-Path -LiteralPath $closedRoot) { Get-ChildItem -LiteralPath $closedRoot -File | Where-Object { $_.Name -like "closed-recovery.$($tail.attempt_id).*" } })
    if ($closedClaims.Count -eq 0) {
        Assert-Contract ($Expected -ne 'Recovered') "$Wave expected RecoveredComplete but has no closed claim"
        Assert-Contract ($tail.phase -ceq 'Committed' -and [int]$tail.recovery_generation -eq 0 -and $tail.recovery_claim_sha256 -ceq $ZeroHash) "$Wave NormalComplete terminal invariants failed"
        return
    }
    Assert-Contract ($Expected -ne 'Normal') "$Wave expected NormalComplete but has a recovery claim"
    Assert-Contract ($closedClaims.Count -eq 1) "$Wave has multiple closed claims for the terminal attempt"
    $claim = Assert-HashedCanonicalObject -Path $closedClaims[0].FullName -Keys $claimKeys -HashProperty 'claim_sha256' -Domain 'dynamo-recovery-claim-v1'
    Assert-Contract ((@($claim.owner.PSObject.Properties.Name) -join ',') -ceq ($ownerKeys -join ',')) "$Wave claim owner schema mismatch"
    Assert-Contract ($claim.attempt_id -ceq $tail.attempt_id -and $claim.lease_sha256 -ceq $tail.lease_sha256) "$Wave closed claim does not bind terminal attempt/lease"
    Assert-Contract ($closedClaims[0].Name -ceq ("closed-recovery.{0}.g{1:d10}.{2}.claim" -f $claim.attempt_id, [int]$claim.generation, $claim.claim_sha256)) "$Wave closed claim filename mismatch"
    $terminalBindsClosedClaim = [int]$tail.recovery_generation -eq [int]$claim.generation -and $tail.recovery_claim_sha256 -ceq $claim.claim_sha256
    $closedClaimBindsTerminal = $claim.snapshot_tail_sha256 -ceq $tail.row_sha256
    Assert-Contract ($terminalBindsClosedClaim -or $closedClaimBindsTerminal) "$Wave terminal/closed-claim binding is not acyclic and exact"
    if ($closedClaimBindsTerminal -and [int]$tail.recovery_generation -gt 0) {
        $archivedClaimPath = Join-Path $waveRoot ("recovery/archives/recovery-claim.{0}.g{1:d10}.{2}.claim" -f $tail.attempt_id, [int]$tail.recovery_generation, $tail.recovery_claim_sha256)
        Assert-Contract (Test-Path -LiteralPath $archivedClaimPath -PathType Leaf) "$Wave recovery terminal does not bind its exact archived claim"
        $archivedClaim = Assert-HashedCanonicalObject -Path $archivedClaimPath -Keys $claimKeys -HashProperty 'claim_sha256' -Domain 'dynamo-recovery-claim-v1'
        Assert-Contract ($archivedClaim.attempt_id -ceq $tail.attempt_id -and $archivedClaim.lease_sha256 -ceq $tail.lease_sha256 -and [int]$archivedClaim.generation -eq [int]$tail.recovery_generation -and $archivedClaim.claim_sha256 -ceq $tail.recovery_claim_sha256) "$Wave archived claim does not match the recovery terminal"
    }
}

function Get-NativePathIdentity {
    param([Parameter(Mandatory)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) { return '<absent>' }
    if ($IsWindows) {
        return [DynamoIntegrationContractNative]::GetIdentity([System.IO.Path]::GetFullPath($Path))
    }
    $output = @(& stat -c '%d:%i' -- ([System.IO.Path]::GetFullPath($Path)) 2>&1)
    Assert-Contract ($LASTEXITCODE -eq 0 -and $output.Count -eq 1) "could not read native inode identity: $Path"
    $output[0].Trim()
}

function Get-PathAclDescriptor {
    param([Parameter(Mandatory)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) { return '<absent>' }
    if (-not $IsWindows) { return [string](Get-Item -LiteralPath $Path -Force).UnixFileMode }
    $item = Get-Item -LiteralPath $Path -Force
    $acl = [System.IO.FileSystemAclExtensions]::GetAccessControl($item)
    $acl.GetSecurityDescriptorSddlForm([System.Security.AccessControl.AccessControlSections]::Access)
}

function Get-PathOwnerDescriptor {
    param([Parameter(Mandatory)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) { return '<absent>' }
    if (-not $IsWindows) { return [string](Get-Item -LiteralPath $Path -Force).UnixFileMode }
    $item = Get-Item -LiteralPath $Path -Force
    $acl = [System.IO.FileSystemAclExtensions]::GetAccessControl($item)
    $acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value
}

function Get-RecoveryMutationSnapshot {
    param([Parameter(Mandatory)]$Fixture, [Parameter(Mandatory)][string]$Wave)

    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $Wave
    $refRoot = Join-Path $Fixture.CommonDirectory "refs/dynamo-remediation/$Wave"
    $refLogRoot = Join-Path $Fixture.CommonDirectory "logs/refs/dynamo-remediation/$Wave"
    [pscustomobject]@{
        WaveTree = @(Get-TreeFingerprint -Root $waveRoot)
        RefTree = @(Get-TreeFingerprint -Root $refRoot)
        RefLogTree = @(Get-TreeFingerprint -Root $refLogRoot)
        ActiveLeaseIdentity = Get-NativePathIdentity (Join-Path $waveRoot 'active-ref-update.lock')
        ActiveClaimIdentity = Get-NativePathIdentity (Join-Path $waveRoot 'recovery/active-recovery.claim')
        ActiveLeaseAcl = Get-PathAclDescriptor (Join-Path $waveRoot 'active-ref-update.lock')
        ActiveClaimAcl = Get-PathAclDescriptor (Join-Path $waveRoot 'recovery/active-recovery.claim')
    }
}

function Assert-RecoveryMutationSnapshotEqual {
    param([Parameter(Mandatory)]$Before, [Parameter(Mandatory)]$After, [Parameter(Mandatory)][string]$Case)

    Assert-Contract ((@($Before.WaveTree) -join "`n") -ceq (@($After.WaveTree) -join "`n")) "$Case mutated the wave state tree"
    Assert-Contract ((@($Before.RefTree) -join "`n") -ceq (@($After.RefTree) -join "`n")) "$Case mutated the raw ref tree"
    Assert-Contract ((@($Before.RefLogTree) -join "`n") -ceq (@($After.RefLogTree) -join "`n")) "$Case mutated the raw reflog tree"
    Assert-Contract ($Before.ActiveLeaseIdentity -ceq $After.ActiveLeaseIdentity) "$Case replaced or moved the active lease"
    Assert-Contract ($Before.ActiveClaimIdentity -ceq $After.ActiveClaimIdentity) "$Case replaced or moved the active claim"
    Assert-Contract ($Before.ActiveLeaseAcl -ceq $After.ActiveLeaseAcl) "$Case changed the active lease ACL"
    Assert-Contract ($Before.ActiveClaimAcl -ceq $After.ActiveClaimAcl) "$Case changed the active claim ACL"
}

function Get-SecurityNegativeSnapshot {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][string]$ManifestPath
    )

    $bindingPath = Join-Path $Fixture.CommonDirectory 'dynamo-remediation/plan-set-binding-v1.json'
    [pscustomobject]@{
        ControlTree = @(Get-TreeFingerprint -Root (Join-Path $Fixture.CommonDirectory 'dynamo-remediation'))
        EvidenceTree = @(Get-TreeFingerprint -Root $Fixture.EvidenceRoot)
        RefTree = @(Get-TreeFingerprint -Root (Join-Path $Fixture.CommonDirectory 'refs/dynamo-remediation'))
        RefLogTree = @(Get-TreeFingerprint -Root (Join-Path $Fixture.CommonDirectory 'logs/refs/dynamo-remediation'))
        CommonIdentity = Get-NativePathIdentity $Fixture.CommonDirectory
        BindingIdentity = Get-NativePathIdentity $bindingPath
        BindingAcl = Get-PathAclDescriptor $bindingPath
        ManifestIdentity = Get-NativePathIdentity $ManifestPath
        ManifestAcl = Get-PathAclDescriptor $ManifestPath
    }
}

function Assert-SecurityNegativeSnapshotEqual {
    param([Parameter(Mandatory)]$Before, [Parameter(Mandatory)]$After, [Parameter(Mandatory)][string]$Case)

    foreach ($property in @('ControlTree','EvidenceTree','RefTree','RefLogTree')) {
        Assert-Contract ((@($Before.$property) -join "`n") -ceq (@($After.$property) -join "`n")) "$Case changed $property"
    }
    foreach ($property in @('CommonIdentity','BindingIdentity','BindingAcl','ManifestIdentity','ManifestAcl')) {
        Assert-Contract ($Before.$property -ceq $After.$property) "$Case changed $property"
    }
}

function Assert-NormalFailpointRecovery {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][string]$Failpoint,
        [Parameter(Mandatory)][int]$Ordinal
    )

    $wave = "fail-normal-$Ordinal"
    $arguments = @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "fail-$Ordinal", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
    $failure = Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_TEST_FAILPOINT = $Failpoint
    }
    Assert-Failed -Invocation $failure -Case "normal failpoint $Failpoint"
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    Assert-Contract (Test-Path -LiteralPath $waveRoot -PathType Container) "$Failpoint did not leave its reviewable fixed-root state"
    $refBeforeRecovery = Get-WaveRef -Repository $Fixture.Repository -Wave $wave
    $reflogBeforeRecovery = Get-RefLogCount -Repository $Fixture.Repository -Wave $wave
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments $arguments) -Case "normal call after $Failpoint"
    $recovery = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Recover', '-Wave', $wave)
    Assert-Contract ($recovery.ExitCode -eq 0) "recovery after $Failpoint failed: $($recovery.Output -join [Environment]::NewLine)"
    $refAfterRecovery = Get-WaveRef -Repository $Fixture.Repository -Wave $wave
    $reflogAfterRecovery = Get-RefLogCount -Repository $Fixture.Repository -Wave $wave
    Assert-Contract ($reflogAfterRecovery -eq $reflogBeforeRecovery) "recovery after $Failpoint repeated a ref CAS"
    if ($null -ne $refBeforeRecovery) {
        Assert-Contract ($refAfterRecovery -ceq $refBeforeRecovery) "recovery after $Failpoint changed an already-observed ref"
    }
    Assert-CanonicalRows -WaveRoot $waveRoot -Repository $Fixture.Repository
    Assert-StateCompletion -Fixture $Fixture -Wave $wave -Expected $(if ($Failpoint -eq 'AfterLeaseRename') { 'Normal' } else { 'Recovered' })
    if ($Failpoint -in @('AfterIntentTempFsync', 'AfterTerminalTempFsync')) {
        $orphans = @(Get-ChildItem -LiteralPath $waveRoot -File -Recurse | Where-Object { $_.FullName -match '[\\/]orphans?[\\/]' -and $_.Name -match '[0-9a-f]{64}' })
        Assert-Contract ($orphans.Count -ge 1) "$Failpoint temp row was not archived to an immutable hash-named orphan"
    }
    $completeBefore = Get-TreeFingerprint -Root $waveRoot
    $completeAgain = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave)
    Assert-Contract ($completeAgain.ExitCode -eq 0) "completed recovery after $Failpoint is not idempotent"
    Assert-Contract (($completeBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $waveRoot) -join "`n")) "completed recovery after $Failpoint was not read-only"
}

function Assert-RecoveryFailpointChain {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][string[]]$RecoveryFailpoints,
        [Parameter(Mandatory)][int]$Ordinal,
        [ValidateSet('AfterLeaseCreate', 'AfterIntentRowRename', 'AfterTerminalRowRename')][string]$SeedFailpoint = 'AfterIntentRowRename'
    )

    $wave = "fail-recovery-$Ordinal"
    $normalArguments = @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "recovery-$Ordinal", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
    $normalFailure = Invoke-Updater -Repository $Fixture.Repository -Arguments $normalArguments -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_TEST_FAILPOINT = $SeedFailpoint
    }
    Assert-Failed -Invocation $normalFailure -Case "recovery failpoint seed $Ordinal"
    $reflogBefore = Get-RefLogCount -Repository $Fixture.Repository -Wave $wave
    foreach ($failpoint in $RecoveryFailpoints) {
        $failure = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Recover', '-Wave', $wave) -Environment @{
            DYNAMO_REMEDIATION_TEST_MODE = '1'
            DYNAMO_REMEDIATION_TEST_FAILPOINT = $failpoint
        }
        Assert-Failed -Invocation $failure -Case "recovery failpoint $failpoint"
        if ($failpoint -eq 'AfterClaimRename') {
            $completedBefore = Get-TreeFingerprint -Root (Get-WaveStateRoot -Fixture $Fixture -Wave $wave)
            $completedRead = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave)
            Assert-Contract ($completedRead.ExitCode -eq 0) 'AfterClaimRename did not leave an exact readable completion'
            Assert-Contract (($completedBefore -join "`n") -ceq ((Get-TreeFingerprint -Root (Get-WaveStateRoot -Fixture $Fixture -Wave $wave)) -join "`n")) 'AfterClaimRename completion read was not state-preserving'
        }
        else {
            Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $normalArguments) -Case "normal call after recovery failpoint $failpoint"
        }
    }
    $recovery = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave)
    Assert-Contract ($recovery.ExitCode -eq 0) "recovery chain $($RecoveryFailpoints -join ',') did not converge: $($recovery.Output -join [Environment]::NewLine)"
    Assert-Contract ((Get-RefLogCount -Repository $Fixture.Repository -Wave $wave) -eq $reflogBefore) 'recovery failpoint chain repeated the ambiguous ref CAS'
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    Assert-CanonicalRows -WaveRoot $waveRoot -Repository $Fixture.Repository
    Assert-StateCompletion -Fixture $Fixture -Wave $wave -Expected Recovered
    $activeClaims = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'recovery') -File -Recurse | Where-Object { $_.FullName -ceq (Join-Path $waveRoot 'recovery/active-recovery.claim') })
    Assert-Contract ($activeClaims.Count -eq 0) 'RecoveredComplete retained an active recovery claim'
    $closedClaims = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'recovery/closed') -File | Where-Object { $_.Name -match '^closed-recovery\.[0-9a-f]{32}\.g\d{10}\.[0-9a-f]{64}\.claim$' })
    Assert-Contract ($closedClaims.Count -eq 1) 'RecoveredComplete does not have exactly one attempt-specific closed claim'
    if ($RecoveryFailpoints -contains 'AfterClaimArchive') {
        $archives = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'recovery/archives') -File -Recurse | Where-Object { $_.Name -match '\.g\d{10}\.[0-9a-f]{64}\.claim$' })
        Assert-Contract ($archives.Count -ge 1) 'dead recovery claim takeover did not retain an immutable generation archive'
    }
    if ($RecoveryFailpoints -contains 'AfterClaimTakeover') {
        $archives = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'recovery/archives') -File | Sort-Object Name)
        Assert-Contract ($archives.Count -eq 2) 'claim takeover chain did not retain exactly generations 1 and 2'
        Assert-Contract ($archives[0].Name -match '\.g0000000001\.[0-9a-f]{64}\.claim$' -and $archives[1].Name -match '\.g0000000002\.[0-9a-f]{64}\.claim$') 'claim takeover archives are not exact generations 1 and 2'
        Assert-Contract ($closedClaims[0].Name -match '\.g0000000003\.[0-9a-f]{64}\.claim$') 'claim takeover did not close exact generation 3'
    }
}

function Assert-StaleClaimRecoveryRace {
    param([Parameter(Mandatory)]$Fixture, [Parameter(Mandatory)][int]$Ordinal)

    $wave = "stale-claim-race-$Ordinal"
    $normalArguments = @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "stale-race-$Ordinal", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
    $seed = Invoke-Updater -Repository $Fixture.Repository -Arguments $normalArguments -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterIntentRowRename'
    }
    Assert-Failed -Invocation $seed -Case "stale-claim race seed $Ordinal"
    $claimSeed = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave) -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterClaimCreate'
    }
    Assert-Failed -Invocation $claimSeed -Case "stale-claim race generation seed $Ordinal"

    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    $activeLeasePath = Join-Path $waveRoot 'active-ref-update.lock'
    $lease = Get-Content -LiteralPath $activeLeasePath -Raw | ConvertFrom-Json -ErrorAction Stop
    $leaseIdentity = Get-NativePathIdentity $activeLeasePath
    $refBefore = Get-WaveRef -Repository $Fixture.Repository -Wave $wave
    $reflogBefore = Get-RefLogCount -Repository $Fixture.Repository -Wave $wave
    $barrierRoot = Join-Path $Fixture.SuiteRoot "stale-claim-takeover-barrier-$Ordinal"
    $null = New-Item -ItemType Directory -Path $barrierRoot
    $barrierEnvironment = @{ DYNAMO_REMEDIATION_TEST_BARRIER = 'BeforeClaimTakeover'; DYNAMO_REMEDIATION_TEST_BARRIER_ROOT = $barrierRoot }
    $left = Start-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave) -Environment $barrierEnvironment
    $right = Start-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Recover', '-Wave', $wave) -Environment $barrierEnvironment
    $leftResult = Complete-Updater -Running $left
    $rightResult = Complete-Updater -Running $right
    Assert-Contract (@(@($leftResult, $rightResult) | Where-Object { $_.ExitCode -eq 0 }).Count -eq 1) "stale-claim race $Ordinal did not produce exactly one takeover winner"
    $final = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave)
    Assert-Contract ($final.ExitCode -eq 0) "stale-claim race $Ordinal did not converge: $($final.Output -join [Environment]::NewLine)"
    Assert-Contract ((Get-WaveRef -Repository $Fixture.Repository -Wave $wave) -ceq $refBefore) "stale-claim race $Ordinal changed the authoritative ref"
    Assert-Contract ((Get-RefLogCount -Repository $Fixture.Repository -Wave $wave) -eq $reflogBefore) "stale-claim race $Ordinal repeated the ref CAS"
    $archives = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'recovery/archives') -File)
    $closedClaims = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'recovery/closed') -File)
    Assert-Contract ($archives.Count -eq 1 -and $archives[0].Name -match '^recovery-claim\..+\.g0000000001\.[0-9a-f]{64}\.claim$') "stale-claim race $Ordinal did not preserve exactly archived generation 1"
    Assert-Contract ($closedClaims.Count -eq 1 -and $closedClaims[0].Name -match '^closed-recovery\..+\.g0000000002\.[0-9a-f]{64}\.claim$') "stale-claim race $Ordinal did not close exactly generation 2"
    $closedLeasePath = Join-Path $waveRoot "leases/closed-ref-update.$($lease.attempt_id).$($lease.lease_sha256).lock"
    Assert-Contract ((Get-NativePathIdentity $closedLeasePath) -ceq $leaseIdentity) "stale-claim race $Ordinal did not preserve lease native identity across closure rename"
    Assert-CanonicalRows -WaveRoot $waveRoot -Repository $Fixture.Repository
    Assert-StateCompletion -Fixture $Fixture -Wave $wave -Expected Recovered
}

function Assert-CorruptRefRecoveryReadOnly {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][ValidateSet('Third','Missing','Malformed','Symbolic')][string]$Kind,
        [Parameter(Mandatory)][int]$Ordinal
    )

    $wave = "corrupt-ref-$Ordinal"
    $refName = "refs/dynamo-remediation/$wave/integration"
    if ($Kind -eq 'Missing') {
        $initialize = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "corrupt-$Ordinal-base", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
        Assert-Contract ($initialize.ExitCode -eq 0) "$Kind corruption fixture initialization failed"
        $newTip = New-CommitObject -Repository $Fixture.Repository -Parent $Fixture.ExecutionBaseline -Message "$Kind corruption candidate"
        $seed = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Advance', '-Wave', $wave, '-Unit', "corrupt-$Ordinal", '-OldTip', $Fixture.ExecutionBaseline, '-NewTip', $newTip, '-MicroplanSha256', $Fixture.MicroplanSha256) -Environment @{
            DYNAMO_REMEDIATION_TEST_MODE = '1'
            DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterIntentRowRename'
        }
    }
    else {
        $seed = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "corrupt-$Ordinal", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256) -Environment @{
            DYNAMO_REMEDIATION_TEST_MODE = '1'
            DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterIntentRowRename'
        }
    }
    Assert-Failed -Invocation $seed -Case "$Kind corrupt-ref seed"
    $claimSeed = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave) -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterClaimCreate'
    }
    Assert-Failed -Invocation $claimSeed -Case "$Kind corrupt-ref stale-claim seed"

    switch ($Kind) {
        'Third' {
            $thirdTip = New-CommitObject -Repository $Fixture.Repository -Parent $Fixture.ExecutionBaseline -Message 'third-value corruption'
            $null = Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('update-ref', '--no-deref', $refName, $thirdTip, $ZeroOid)
        }
        'Missing' {
            $null = Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('update-ref', '-d', '--no-deref', $refName, $Fixture.ExecutionBaseline)
        }
        'Malformed' {
            $looseRef = Join-Path $Fixture.CommonDirectory $refName
            $null = New-Item -ItemType Directory -Path (Split-Path -Parent $looseRef) -Force
            [System.IO.File]::WriteAllText($looseRef, "not-an-object-id`n", [System.Text.UTF8Encoding]::new($false))
        }
        'Symbolic' {
            $null = Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('symbolic-ref', $refName, 'refs/heads/main')
        }
    }

    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    $failure = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Recover', '-Wave', $wave)
    Assert-Failed -Invocation $failure -Case "$Kind corrupt-ref recovery"
    $after = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    Assert-RecoveryMutationSnapshotEqual -Before $before -After $after -Case "$Kind corrupt-ref recovery"
}

function Assert-CorruptRecoveryShapeReadOnly {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][ValidateSet('Shape0NoClaim','ShapeANoClaim','ShapeBActiveClaim','ShapeCArchiveOnly')][string]$ShapeCase,
        [Parameter(Mandatory)][int]$Ordinal
    )

    $wave = "corrupt-shape-$Ordinal"
    $normalArguments = @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "corrupt-shape-$Ordinal", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
    $seedFailpoint = if ($ShapeCase -eq 'Shape0NoClaim') { 'AfterLeaseCreate' } elseif ($ShapeCase -eq 'ShapeANoClaim') { 'AfterIntentRowRename' } else { 'AfterTerminalRowRename' }
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $normalArguments -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'; DYNAMO_REMEDIATION_TEST_FAILPOINT = $seedFailpoint
    }) -Case "$ShapeCase normal seed"

    if ($ShapeCase -eq 'ShapeBActiveClaim') {
        Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave) -Environment @{
            DYNAMO_REMEDIATION_TEST_MODE = '1'; DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterClaimCreate'
        }) -Case "$ShapeCase active-claim seed"
    }
    elseif ($ShapeCase -eq 'ShapeCArchiveOnly') {
        Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave) -Environment @{
            DYNAMO_REMEDIATION_TEST_MODE = '1'; DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterLeaseRename'
        }) -Case "$ShapeCase closed-lease seed"
        Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave) -Environment @{
            DYNAMO_REMEDIATION_TEST_MODE = '1'; DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterClaimArchive'
        }) -Case "$ShapeCase archive-only seed"
    }

    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    if ($ShapeCase -in @('Shape0NoClaim','ShapeANoClaim','ShapeCArchiveOnly')) {
        Assert-Contract (-not (Test-Path -LiteralPath (Join-Path $waveRoot 'recovery/active-recovery.claim'))) "$ShapeCase unexpectedly has an active claim before corruption"
    }
    else {
        Assert-Contract (Test-Path -LiteralPath (Join-Path $waveRoot 'recovery/active-recovery.claim') -PathType Leaf) "$ShapeCase lacks its expected active claim"
    }
    if ($ShapeCase -eq 'ShapeCArchiveOnly') {
        Assert-Contract (@(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'recovery/archives') -File).Count -eq 1) "$ShapeCase lacks its exact archived claim"
    }

    $thirdTip = New-CommitObject -Repository $Fixture.Repository -Parent $Fixture.ExecutionBaseline -Message "$ShapeCase third-value corruption"
    $oldTip = if ($ShapeCase -in @('Shape0NoClaim','ShapeANoClaim')) { $ZeroOid } else { $Fixture.ExecutionBaseline }
    $null = Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('update-ref', '--no-deref', "refs/dynamo-remediation/$wave/integration", $thirdTip, $oldTip)
    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    $failure = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Recover', '-Wave', $wave)
    Assert-Failed -Invocation $failure -Case "$ShapeCase corrupt-ref recovery"
    $after = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    Assert-RecoveryMutationSnapshotEqual -Before $before -After $after -Case "$ShapeCase corrupt-ref recovery"
}

function Assert-ExactClaimTupleRejection {
    param([Parameter(Mandatory)]$Fixture, [Parameter(Mandatory)][int]$Ordinal)

    $wave = "claim-tuple-$Ordinal"
    $normalArguments = @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "claim-tuple-$Ordinal", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $normalArguments -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'; DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterIntentRowRename'
    }) -Case 'exact claim tuple normal seed'
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave) -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'; DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterRecoveryTerminalRowRename'
    }) -Case 'exact claim tuple terminal seed'
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave) -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'; DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterClaimTakeover'
    }) -Case 'exact claim tuple later-generation seed'

    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    $strictUtf8 = [System.Text.UTF8Encoding]::new($false, $true)
    $activeClaimRaw = $strictUtf8.GetString([System.IO.File]::ReadAllBytes((Join-Path $waveRoot 'recovery/active-recovery.claim')))
    $activeClaimDocument = [System.Text.Json.JsonDocument]::Parse($activeClaimRaw)
    try {
        $activeGeneration = $activeClaimDocument.RootElement.GetProperty('generation').GetInt64()
        $activeClaimSha256 = $activeClaimDocument.RootElement.GetProperty('claim_sha256').GetString()
    }
    finally { $activeClaimDocument.Dispose() }
    Assert-Contract ($activeGeneration -eq 2) 'exact claim tuple fixture did not reach active generation 2'
    Assert-Contract ($activeClaimSha256 -match '^[0-9a-f]{64}$') 'exact claim tuple fixture active claim hash is malformed'
    $terminalPath = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'journal/rows') -File | Sort-Object Name)[-1].FullName
    $terminalRawBefore = $strictUtf8.GetString([System.IO.File]::ReadAllBytes($terminalPath))
    Assert-Contract (-not $terminalRawBefore.Contains("`r") -and $terminalRawBefore.EndsWith("`n", [System.StringComparison]::Ordinal) -and -not $terminalRawBefore.Substring(0, $terminalRawBefore.Length - 1).Contains("`n")) 'exact claim tuple source terminal is not compact canonical JSON'
    $terminalDocument = [System.Text.Json.JsonDocument]::Parse($terminalRawBefore)
    try {
        $terminalGeneration = $terminalDocument.RootElement.GetProperty('recovery_generation').GetInt64()
        $terminalClaimSha256 = $terminalDocument.RootElement.GetProperty('recovery_claim_sha256').GetString()
        $terminalUtc = $terminalDocument.RootElement.GetProperty('utc').GetString()
        $terminalKeys = @($terminalDocument.RootElement.EnumerateObject() | ForEach-Object Name)
    }
    finally { $terminalDocument.Dispose() }
    Assert-Contract ($terminalGeneration -eq 1) 'exact claim tuple fixture terminal is not generation 1'
    Assert-Contract ($terminalClaimSha256 -match '^[0-9a-f]{64}$' -and $terminalClaimSha256 -cne $activeClaimSha256) 'exact claim tuple fixture lacks two distinct claim hashes'
    Assert-Contract (($terminalKeys -join ',') -ceq ($RowKeys -join ',')) 'exact claim tuple fixture source terminal schema/order is not canonical'
    Assert-Contract ($terminalUtc -match '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}Z$') 'exact claim tuple fixture source UTC is not canonical'

    $claimNeedle = '"recovery_claim_sha256":"' + $terminalClaimSha256 + '"'
    Assert-Contract ([regex]::Matches($terminalRawBefore, [regex]::Escape($claimNeedle)).Count -eq 1) 'exact claim tuple fixture claim field is not unique in raw JSON'
    $terminalWithClaimTamper = $terminalRawBefore.Replace($claimNeedle, ('"recovery_claim_sha256":"' + $activeClaimSha256 + '"'))
    Assert-Contract ($terminalWithClaimTamper -cne $terminalRawBefore) 'exact claim tuple raw claim edit made no change'
    $rowHashMatch = [regex]::Match($terminalWithClaimTamper, ',"row_sha256":"(?<hash>[0-9a-f]{64})"}\n$')
    Assert-Contract ($rowHashMatch.Success) 'exact claim tuple fixture row self-hash is not the final canonical property'
    $terminalPreimage = $terminalWithClaimTamper.Substring(0, $rowHashMatch.Index) + "}`n"
    $newRowSha256 = Get-DomainHash -Domain 'dynamo-integration-row-v1' -CanonicalJson $terminalPreimage
    $hashGroup = $rowHashMatch.Groups['hash']
    $terminalRawAfter = $terminalWithClaimTamper.Substring(0, $hashGroup.Index) + $newRowSha256 + $terminalWithClaimTamper.Substring($hashGroup.Index + $hashGroup.Length)

    $tamperedDocument = [System.Text.Json.JsonDocument]::Parse($terminalRawAfter)
    try {
        $tamperedGeneration = $tamperedDocument.RootElement.GetProperty('recovery_generation').GetInt64()
        $tamperedClaimSha256 = $tamperedDocument.RootElement.GetProperty('recovery_claim_sha256').GetString()
        $tamperedUtc = $tamperedDocument.RootElement.GetProperty('utc').GetString()
        $tamperedRowSha256 = $tamperedDocument.RootElement.GetProperty('row_sha256').GetString()
        $tamperedKeys = @($tamperedDocument.RootElement.EnumerateObject() | ForEach-Object Name)
    }
    finally { $tamperedDocument.Dispose() }
    Assert-Contract ($tamperedGeneration -eq 1 -and $tamperedClaimSha256 -ceq $activeClaimSha256 -and $tamperedRowSha256 -ceq $newRowSha256) 'exact claim tuple tamper did not preserve the intended generation/hash mismatch'
    Assert-Contract ($tamperedUtc -ceq $terminalUtc -and (($tamperedKeys -join ',') -ceq ($RowKeys -join ','))) 'exact claim tuple tamper changed UTC or schema/order'
    $tamperedHashMatch = [regex]::Match($terminalRawAfter, ',"row_sha256":"(?<hash>[0-9a-f]{64})"}\n$')
    Assert-Contract ($tamperedHashMatch.Success -and (Get-DomainHash -Domain 'dynamo-integration-row-v1' -CanonicalJson ($terminalRawAfter.Substring(0, $tamperedHashMatch.Index) + "}`n")) -ceq $newRowSha256) 'exact claim tuple tamper is not a valid self-hashed canonical row'
    [System.IO.File]::WriteAllBytes($terminalPath, $strictUtf8.GetBytes($terminalRawAfter))
    Assert-Contract ($strictUtf8.GetString([System.IO.File]::ReadAllBytes($terminalPath)) -ceq $terminalRawAfter) 'exact claim tuple tamper write/readback changed raw canonical bytes'

    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    $failure = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Recover', '-Wave', $wave)
    Assert-Failed -Invocation $failure -Case 'generation/hash-mismatched recovery terminal'
    $after = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    Assert-RecoveryMutationSnapshotEqual -Before $before -After $after -Case 'generation/hash-mismatched recovery terminal'
}

function Update-RawSelfHash {
    param(
        [Parameter(Mandatory)][string]$Raw,
        [Parameter(Mandatory)][string]$Domain,
        [Parameter(Mandatory)][string]$HashProperty
    )

    $match = [regex]::Match($Raw, (',"' + [regex]::Escape($HashProperty) + '":"(?<hash>[0-9a-f]{64})"}\n$'))
    Assert-Contract ($match.Success) "$HashProperty is not the final canonical property in injected raw JSON"
    $newHash = Get-DomainHash -Domain $Domain -CanonicalJson ($Raw.Substring(0, $match.Index) + "}`n")
    $hashGroup = $match.Groups['hash']
    $rewritten = $Raw.Substring(0, $hashGroup.Index) + $newHash + $Raw.Substring($hashGroup.Index + $hashGroup.Length)
    $verify = [regex]::Match($rewritten, (',"' + [regex]::Escape($HashProperty) + '":"(?<hash>[0-9a-f]{64})"}\n$'))
    Assert-Contract ($verify.Success -and (Get-DomainHash -Domain $Domain -CanonicalJson ($rewritten.Substring(0, $verify.Index) + "}`n")) -ceq $newHash) 'injected raw JSON self-hash prevalidation failed'
    $rewritten
}

function Assert-DuplicateJournalRowRejection {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][ValidateSet('Intent','Terminal')][string]$Kind,
        [Parameter(Mandatory)][int]$Ordinal
    )

    $wave = "duplicate-$($Kind.ToLowerInvariant())-$Ordinal"
    $arguments = @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "duplicate-$Ordinal", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
    $initialize = Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments
    Assert-Contract ($initialize.ExitCode -eq 0) "$Kind duplicate-row fixture initialization failed"
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    $rows = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'journal/rows') -File | Sort-Object Name)
    Assert-Contract ($rows.Count -eq 2) "$Kind duplicate-row fixture lacks its exact base pair"
    $tailRaw = [System.IO.File]::ReadAllText($rows[-1].FullName, [System.Text.UTF8Encoding]::new($false, $true))
    $tailDocument = [System.Text.Json.JsonDocument]::Parse($tailRaw)
    try { $tailHash = $tailDocument.RootElement.GetProperty('row_sha256').GetString() }
    finally { $tailDocument.Dispose() }
    $sourceFile = if ($Kind -eq 'Intent') { $rows[0] } else { $rows[1] }
    $sourceRaw = [System.IO.File]::ReadAllText($sourceFile.FullName, [System.Text.UTF8Encoding]::new($false, $true))
    $sourceDocument = [System.Text.Json.JsonDocument]::Parse($sourceRaw)
    try {
        $sourceSeq = $sourceDocument.RootElement.GetProperty('seq').GetInt64()
        $sourcePhase = $sourceDocument.RootElement.GetProperty('phase').GetString()
        $sourcePrevious = $sourceDocument.RootElement.GetProperty('prev_row_sha256').GetString()
        $sourceUtc = $sourceDocument.RootElement.GetProperty('utc').GetString()
    }
    finally { $sourceDocument.Dispose() }
    $duplicateRaw = $sourceRaw.Replace(('"seq":' + $sourceSeq), '"seq":3')
    $duplicateRaw = $duplicateRaw.Replace(('"prev_row_sha256":"' + $sourcePrevious + '"'), ('"prev_row_sha256":"' + $tailHash + '"'))
    Assert-Contract ($duplicateRaw -cne $sourceRaw -and $duplicateRaw.Contains(('"utc":"' + $sourceUtc + '"'), [System.StringComparison]::Ordinal)) "$Kind duplicate-row raw fixture edit failed or rewrote UTC"
    $duplicateRaw = Update-RawSelfHash -Raw $duplicateRaw -Domain 'dynamo-integration-row-v1' -HashProperty 'row_sha256'
    $slug = if ($sourcePhase -ceq 'Intent') { 'intent' } elseif ($sourcePhase -ceq 'Committed') { 'committed' } else { throw "Unexpected duplicate-row source phase: $sourcePhase" }
    $duplicatePath = Join-Path $waveRoot ("journal/rows/{0:d20}-$slug.json" -f 3)
    [System.IO.File]::WriteAllBytes($duplicatePath, [System.Text.UTF8Encoding]::new($false).GetBytes($duplicateRaw))
    $duplicateDocument = [System.Text.Json.JsonDocument]::Parse($duplicateRaw)
    try {
        Assert-Contract ($duplicateDocument.RootElement.GetProperty('seq').GetInt64() -eq 3 -and $duplicateDocument.RootElement.GetProperty('phase').GetString() -ceq $sourcePhase -and $duplicateDocument.RootElement.GetProperty('prev_row_sha256').GetString() -ceq $tailHash) "$Kind duplicate-row injection is not a valid chained row"
    }
    finally { $duplicateDocument.Dispose() }
    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    $failure = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Recover', '-Wave', $wave)
    Assert-Failed -Invocation $failure -Case "duplicate $Kind journal row"
    Assert-RecoveryMutationSnapshotEqual -Before $before -After (Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave) -Case "duplicate $Kind journal row"
}

function Assert-CanonicalEncodingRejection {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][ValidateSet('DuplicateKey','Bom','Whitespace','Timestamp')][string]$Kind,
        [Parameter(Mandatory)][int]$Ordinal
    )

    $wave = "canonical-$($Kind.ToLowerInvariant())-$Ordinal"
    $arguments = @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "canonical-$Ordinal", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
    $initialize = Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments
    Assert-Contract ($initialize.ExitCode -eq 0) "$Kind canonical-negative fixture initialization failed"
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    $terminalPath = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'journal/rows') -File | Sort-Object Name)[-1].FullName
    $utf8 = [System.Text.UTF8Encoding]::new($false, $true)
    $originalRaw = $utf8.GetString([System.IO.File]::ReadAllBytes($terminalPath))
    $mutatedBytes = switch ($Kind) {
        'DuplicateKey' { $utf8.GetBytes($originalRaw.Insert(1, '"schema_version":1,')); break }
        'Bom' { [byte[]](0xef,0xbb,0xbf) + $utf8.GetBytes($originalRaw); break }
        'Whitespace' { $utf8.GetBytes($originalRaw.Substring(0, $originalRaw.Length - 1) + " `n"); break }
        'Timestamp' {
            $document = [System.Text.Json.JsonDocument]::Parse($originalRaw)
            try { $utc = $document.RootElement.GetProperty('utc').GetString() }
            finally { $document.Dispose() }
            $noncanonicalUtc = $utc.Substring(0, 19) + 'Z'
            $timestampRaw = $originalRaw.Replace(('"utc":"' + $utc + '"'), ('"utc":"' + $noncanonicalUtc + '"'))
            Assert-Contract ($timestampRaw -cne $originalRaw) 'timestamp-negative raw edit made no change'
            $utf8.GetBytes((Update-RawSelfHash -Raw $timestampRaw -Domain 'dynamo-integration-row-v1' -HashProperty 'row_sha256'))
            break
        }
    }
    [System.IO.File]::WriteAllBytes($terminalPath, $mutatedBytes)
    Assert-Contract (([System.IO.File]::ReadAllBytes($terminalPath).Length -eq $mutatedBytes.Length)) "$Kind canonical-negative bytes were not persisted"
    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    $failure = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Recover', '-Wave', $wave)
    Assert-Failed -Invocation $failure -Case "$Kind canonical JSON"
    Assert-RecoveryMutationSnapshotEqual -Before $before -After (Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave) -Case "$Kind canonical JSON"
}

function Assert-CoherentScalarRejection {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][ValidateSet(
            'LeaseSchemaString','LeaseExpectedSeqString','LeasePidString','LeaseInvalidCalendar',
            'RowSeqString','RowRecoveryGenerationString','RowInvalidCalendar',
            'ClaimGenerationString','ClaimPidString','ClaimInvalidCalendar'
        )][string]$Kind,
        [Parameter(Mandatory)][int]$Ordinal
    )

    $wave = "scalar-$Ordinal"
    $arguments = @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "scalar-$Ordinal", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    if ($Kind.StartsWith('Lease', [System.StringComparison]::Ordinal)) {
        Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments -Environment @{ DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterLeaseCreate' }) -Case "$Kind fixture seed"
        $path = Join-Path $waveRoot 'active-ref-update.lock'
        $domain = 'dynamo-integration-lease-v1'
        $hashProperty = 'lease_sha256'
    }
    elseif ($Kind.StartsWith('Claim', [System.StringComparison]::Ordinal)) {
        Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments -Environment @{ DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterIntentRowRename' }) -Case "$Kind normal fixture seed"
        Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode','Recover','-Wave',$wave) -Environment @{ DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterClaimCreate' }) -Case "$Kind claim fixture seed"
        $path = Join-Path $waveRoot 'recovery/active-recovery.claim'
        $domain = 'dynamo-recovery-claim-v1'
        $hashProperty = 'claim_sha256'
    }
    else {
        $complete = Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments
        Assert-Contract ($complete.ExitCode -eq 0) "$Kind completed-row fixture seed failed"
        $path = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'journal/rows') -File | Sort-Object Name)[-1].FullName
        $domain = 'dynamo-integration-row-v1'
        $hashProperty = 'row_sha256'
    }

    $utf8 = [System.Text.UTF8Encoding]::new($false, $true)
    $raw = $utf8.GetString([System.IO.File]::ReadAllBytes($path))
    $document = [System.Text.Json.JsonDocument]::Parse($raw)
    try {
        $root = $document.RootElement
        $oldPid = if ($Kind -in @('LeasePidString','ClaimPidString')) { $root.GetProperty('owner').GetProperty('pid').GetInt64() } else { 0 }
        $oldTimestamp = if ($Kind -like '*InvalidCalendar') { $root.GetProperty($(if ($Kind.StartsWith('Row')) { 'utc' } else { 'created_at' })).GetString() } else { $null }
        $expectedSeq = if ($Kind -eq 'LeaseExpectedSeqString') { $root.GetProperty('expected_seq').GetInt64() } else { 0 }
        $rowSeq = if ($Kind -eq 'RowSeqString') { $root.GetProperty('seq').GetInt64() } else { 0 }
        $recoveryGeneration = if ($Kind -eq 'RowRecoveryGenerationString') { $root.GetProperty('recovery_generation').GetInt64() } else { 0 }
        $claimGeneration = if ($Kind -eq 'ClaimGenerationString') { $root.GetProperty('generation').GetInt64() } else { 0 }
    }
    finally { $document.Dispose() }

    $mutated = switch ($Kind) {
        'LeaseSchemaString' { $raw.Replace('"schema_version":1', '"schema_version":"1"'); break }
        'LeaseExpectedSeqString' { $raw.Replace(('"expected_seq":' + $expectedSeq), ('"expected_seq":"' + $expectedSeq + '"')); break }
        'LeasePidString' { $raw.Replace(('"pid":' + $oldPid), ('"pid":"' + $oldPid + '"')); break }
        'RowSeqString' { $raw.Replace(('"seq":' + $rowSeq), ('"seq":"' + $rowSeq + '"')); break }
        'RowRecoveryGenerationString' { $raw.Replace(('"recovery_generation":' + $recoveryGeneration), ('"recovery_generation":"' + $recoveryGeneration + '"')); break }
        'ClaimGenerationString' { $raw.Replace(('"generation":' + $claimGeneration), ('"generation":"' + $claimGeneration + '"')); break }
        'ClaimPidString' { $raw.Replace(('"pid":' + $oldPid), ('"pid":"' + $oldPid + '"')); break }
        default {
            $invalidTimestamp = '2026-02-30' + $oldTimestamp.Substring(10)
            $raw.Replace(('"' + $(if ($Kind.StartsWith('Row')) { 'utc' } else { 'created_at' }) + '":"' + $oldTimestamp + '"'), ('"' + $(if ($Kind.StartsWith('Row')) { 'utc' } else { 'created_at' }) + '":"' + $invalidTimestamp + '"'))
        }
    }
    Assert-Contract ($mutated -cne $raw) "$Kind coherent scalar edit made no change"
    $mutated = Update-RawSelfHash -Raw $mutated -Domain $domain -HashProperty $hashProperty
    $mutatedDocument = [System.Text.Json.JsonDocument]::Parse($mutated)
    try { $normalized = (ConvertFrom-JsonElementToMinimalText $mutatedDocument.RootElement) + "`n" }
    finally { $mutatedDocument.Dispose() }
    Assert-Contract ($normalized -ceq $mutated) "$Kind fixture is not otherwise canonical minimal JSON"
    [System.IO.File]::WriteAllBytes($path, $utf8.GetBytes($mutated))

    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    $failure = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode','Recover','-Wave',$wave)
    Assert-Failed -Invocation $failure -Case "$Kind coherent self-hashed scalar"
    Assert-RecoveryMutationSnapshotEqual -Before $before -After (Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave) -Case "$Kind coherent self-hashed scalar"
}

function Assert-OwnerIdentitySemantics {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][ValidateSet('LiveSameBirth','RemoteMachine','PidReuse')][string]$Kind,
        [Parameter(Mandatory)][int]$Ordinal
    )

    $wave = "owner-$($Kind.ToLowerInvariant())-$Ordinal"
    $arguments = @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "owner-$Ordinal", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments -Environment @{ DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterLeaseCreate' }) -Case "$Kind owner fixture seed"
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    $leasePath = Join-Path $waveRoot 'active-ref-update.lock'
    $utf8 = [System.Text.UTF8Encoding]::new($false, $true)
    $leaseRaw = $utf8.GetString([System.IO.File]::ReadAllBytes($leasePath))
    $leaseDocument = [System.Text.Json.JsonDocument]::Parse($leaseRaw)
    try {
        $owner = $leaseDocument.RootElement.GetProperty('owner')
        $oldMachine = $owner.GetProperty('machine_identity_sha256').GetString()
        $oldPid = $owner.GetProperty('pid').GetInt64()
        $oldStart = $owner.GetProperty('process_start_identity').GetString()
        $createdAt = $leaseDocument.RootElement.GetProperty('created_at').GetString()
    }
    finally { $leaseDocument.Dispose() }
    $machineMaterial = (Get-ItemPropertyValue -LiteralPath 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Cryptography' -Name MachineGuid -ErrorAction Stop).ToString().ToLowerInvariant()
    $currentMachine = Get-DomainHash -Domain 'dynamo-machine-identity-v1' -CanonicalJson $machineMaterial
    $process = [System.Diagnostics.Process]::GetProcessById($PID)
    try { $startMaterial = $process.StartTime.ToUniversalTime().ToFileTimeUtc().ToString([System.Globalization.CultureInfo]::InvariantCulture) }
    finally { $process.Dispose() }
    $currentStart = Get-DomainHash -Domain 'dynamo-process-birth-v1' -CanonicalJson $startMaterial
    $differentMachine = if ($currentMachine -cne ('f' * 64)) { 'f' * 64 } else { 'e' * 64 }
    $differentStart = if ($currentStart -cne ('f' * 64)) { 'f' * 64 } else { 'e' * 64 }
    $newMachine = if ($Kind -eq 'RemoteMachine') { $differentMachine } else { $currentMachine }
    $newStart = if ($Kind -eq 'PidReuse') { $differentStart } else { $currentStart }
    $mutatedRaw = $leaseRaw.Replace(('"machine_identity_sha256":"' + $oldMachine + '"'), ('"machine_identity_sha256":"' + $newMachine + '"'))
    $mutatedRaw = $mutatedRaw.Replace(('"pid":' + $oldPid), ('"pid":' + $PID))
    $mutatedRaw = $mutatedRaw.Replace(('"process_start_identity":"' + $oldStart + '"'), ('"process_start_identity":"' + $newStart + '"'))
    $mutatedRaw = Update-RawSelfHash -Raw $mutatedRaw -Domain 'dynamo-integration-lease-v1' -HashProperty 'lease_sha256'
    Assert-Contract ($mutatedRaw -cne $leaseRaw -and $mutatedRaw.Contains(('"created_at":"' + $createdAt + '"'), [System.StringComparison]::Ordinal)) "$Kind owner raw edit failed or rewrote created_at"
    [System.IO.File]::WriteAllBytes($leasePath, $utf8.GetBytes($mutatedRaw))
    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    $recovery = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Recover', '-Wave', $wave)
    if ($Kind -eq 'PidReuse') {
        Assert-Contract ($recovery.ExitCode -eq 0) "PID-reuse proof did not permit recovery: $($recovery.Output -join [Environment]::NewLine)"
        Assert-CanonicalRows -WaveRoot $waveRoot -Repository $Fixture.Repository
        Assert-StateCompletion -Fixture $Fixture -Wave $wave -Expected Recovered
    }
    else {
        Assert-Failed -Invocation $recovery -Case "$Kind owner recovery"
        Assert-RecoveryMutationSnapshotEqual -Before $before -After (Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave) -Case "$Kind owner recovery"
    }
}

function Assert-ProcessQueryAccessDeniedRejection {
    param([Parameter(Mandatory)]$Fixture, [Parameter(Mandatory)][int]$Ordinal)

    $wave = "owner-access-denied-$Ordinal"
    $arguments = @('-Mode','Initialize','-Wave',$wave,'-Unit',"owner-access-denied-$Ordinal",'-NewTip',$Fixture.ExecutionBaseline,'-MicroplanSha256',$Fixture.MicroplanSha256)
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments -Environment @{ DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterLeaseCreate' }) -Case 'process-query access-denied seed'
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    $lease = Get-Content -LiteralPath (Join-Path $waveRoot 'active-ref-update.lock') -Raw | ConvertFrom-Json -ErrorAction Stop
    $ownerPid = [int64]$lease.owner.pid
    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    $failure = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode','Recover','-Wave',$wave) -Environment @{
        DYNAMO_REMEDIATION_TEST_PROCESS_QUERY_FAILURE = 'AccessDenied'
        DYNAMO_REMEDIATION_TEST_PROCESS_QUERY_FAILURE_PID = $ownerPid.ToString([System.Globalization.CultureInfo]::InvariantCulture)
    }
    Assert-Failed -Invocation $failure -Case 'process-query access denied ambiguity'
    Assert-RecoveryMutationSnapshotEqual -Before $before -After (Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave) -Case 'process-query access denied ambiguity'
}

function Assert-ArchivedClaimLiveOwnerRejection {
    param([Parameter(Mandatory)]$Fixture, [Parameter(Mandatory)][int]$Ordinal)

    $wave = "archived-live-owner-$Ordinal"
    $arguments = @('-Mode','Initialize','-Wave',$wave,'-Unit',"archived-live-owner-$Ordinal",'-NewTip',$Fixture.ExecutionBaseline,'-MicroplanSha256',$Fixture.MicroplanSha256)
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments -Environment @{ DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterIntentRowRename' }) -Case 'archived live-owner normal seed'
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode','Recover','-Wave',$wave) -Environment @{ DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterClaimCreate' }) -Case 'archived live-owner claim seed'
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode','Recover','-Wave',$wave) -Environment @{ DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterClaimArchive' }) -Case 'archived live-owner archive seed'
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    $archive = @(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'recovery/archives') -File -Force)
    Assert-Contract ($archive.Count -eq 1) 'archived live-owner fixture did not retain exactly generation 1'
    $utf8 = [System.Text.UTF8Encoding]::new($false, $true)
    $raw = $utf8.GetString([System.IO.File]::ReadAllBytes($archive[0].FullName))
    $document = [System.Text.Json.JsonDocument]::Parse($raw)
    try {
        $oldPid = $document.RootElement.GetProperty('owner').GetProperty('pid').GetInt64()
        $oldStart = $document.RootElement.GetProperty('owner').GetProperty('process_start_identity').GetString()
    }
    finally { $document.Dispose() }
    $process = [System.Diagnostics.Process]::GetProcessById($PID)
    try { $startMaterial = $process.StartTime.ToUniversalTime().ToFileTimeUtc().ToString([System.Globalization.CultureInfo]::InvariantCulture) }
    finally { $process.Dispose() }
    $currentStart = Get-DomainHash -Domain 'dynamo-process-birth-v1' -CanonicalJson $startMaterial
    $mutated = $raw.Replace(('"pid":' + $oldPid), ('"pid":' + $PID)).Replace(('"process_start_identity":"' + $oldStart + '"'), ('"process_start_identity":"' + $currentStart + '"'))
    $mutated = Update-RawSelfHash -Raw $mutated -Domain 'dynamo-recovery-claim-v1' -HashProperty 'claim_sha256'
    $mutatedObject = $mutated | ConvertFrom-Json -ErrorAction Stop
    $newPath = Join-Path $archive[0].DirectoryName ("recovery-claim.{0}.g{1:d10}.{2}.claim" -f $mutatedObject.attempt_id, [int]$mutatedObject.generation, $mutatedObject.claim_sha256)
    [System.IO.File]::WriteAllBytes($archive[0].FullName, $utf8.GetBytes($mutated))
    Move-Item -LiteralPath $archive[0].FullName -Destination $newPath
    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    $failure = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode','Recover','-Wave',$wave)
    Assert-Failed -Invocation $failure -Case 'archived recovery claim with live same-birth owner'
    Assert-RecoveryMutationSnapshotEqual -Before $before -After (Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave) -Case 'archived recovery claim with live same-birth owner'
}

function Assert-ControlFileAclRejection {
    param([Parameter(Mandatory)]$Fixture, [Parameter(Mandatory)][int]$Ordinal)

    if (-not $IsWindows) { throw 'ACL contract fixture requires Windows.' }
    $wave = "acl-file-$Ordinal"
    $arguments = @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "acl-file-$Ordinal", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments -Environment @{ DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterLeaseCreate' }) -Case 'file ACL fixture seed'
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    $leasePath = Join-Path $waveRoot 'active-ref-update.lock'
    $leaseInfo = [System.IO.FileInfo]::new($leasePath)
    $acl = [System.IO.FileSystemAclExtensions]::GetAccessControl($leaseInfo)
    $users = [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-545')
    $extraRule = [System.Security.AccessControl.FileSystemAccessRule]::new($users, [System.Security.AccessControl.FileSystemRights]::Read, [System.Security.AccessControl.AccessControlType]::Allow)
    $null = $acl.AddAccessRule($extraRule)
    [System.IO.FileSystemAclExtensions]::SetAccessControl($leaseInfo, $acl)
    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    $failure = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Recover', '-Wave', $wave)
    Assert-Failed -Invocation $failure -Case 'unexpected control-file ACE'
    Assert-RecoveryMutationSnapshotEqual -Before $before -After (Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave) -Case 'unexpected control-file ACE'
}

function Assert-ControlDirectorySecurityRejection {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][ValidateSet('ExtraAce','WrongOwner','MissingSystemAce')][string]$Kind,
        [Parameter(Mandatory)][int]$Ordinal
    )

    if (-not $IsWindows) { throw 'directory ACL contract fixture requires Windows.' }
    $wave = "acl-directory-$Ordinal"
    $arguments = @('-Mode','Initialize','-Wave',$wave,'-Unit',"acl-directory-$Ordinal",'-NewTip',$Fixture.ExecutionBaseline,'-MicroplanSha256',$Fixture.MicroplanSha256)
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments -Environment @{ DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterLeaseCreate' }) -Case "$Kind directory security seed"
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    $target = Join-Path $waveRoot 'journal/tmp'
    $directoryInfo = [System.IO.DirectoryInfo]::new($target)
    $acl = [System.IO.FileSystemAclExtensions]::GetAccessControl($directoryInfo)
    $system = [System.Security.Principal.SecurityIdentifier]::new('S-1-5-18')
    $environment = @{}
    if ($Kind -eq 'WrongOwner') {
        # A standard non-elevated token cannot assign an arbitrary real owner. The updater's
        # temp-fixture-only hook makes its semantic owner check observe an exact mismatch.
        $environment['DYNAMO_REMEDIATION_TEST_OWNER_MISMATCH_PATH'] = $target
    }
    elseif ($Kind -eq 'MissingSystemAce') {
        $acl.PurgeAccessRules($system)
    }
    else {
        $users = [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-545')
        $rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
            $users,
            [System.Security.AccessControl.FileSystemRights]::FullControl,
            [System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit',
            [System.Security.AccessControl.PropagationFlags]::None,
            [System.Security.AccessControl.AccessControlType]::Allow
        )
        $null = $acl.AddAccessRule($rule)
    }
    if ($Kind -ne 'WrongOwner') { [System.IO.FileSystemAclExtensions]::SetAccessControl($directoryInfo, $acl) }
    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    $failure = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode','Recover','-Wave',$wave) -Environment $environment
    Assert-Failed -Invocation $failure -Case "$Kind control-directory security"
    Assert-RecoveryMutationSnapshotEqual -Before $before -After (Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave) -Case "$Kind control-directory security"
}

function Assert-ReparseDirectoryRejection {
    param([Parameter(Mandatory)]$Fixture, [Parameter(Mandatory)][int]$Ordinal)

    if (-not $IsWindows) { throw 'reparse contract fixture requires Windows.' }
    $wave = "reparse-directory-$Ordinal"
    $arguments = @('-Mode','Initialize','-Wave',$wave,'-Unit',"reparse-directory-$Ordinal",'-NewTip',$Fixture.ExecutionBaseline,'-MicroplanSha256',$Fixture.MicroplanSha256)
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments -Environment @{ DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterLeaseCreate' }) -Case 'reparse directory seed'
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    $target = Join-Path $waveRoot 'journal/tmp'
    $original = Join-Path $Fixture.SuiteRoot "reparse-original-$Ordinal"
    $redirect = Join-Path $Fixture.SuiteRoot "reparse-target-$Ordinal"
    Move-Item -LiteralPath $target -Destination $original
    $null = New-Item -ItemType Directory -Path $redirect
    $null = New-Item -ItemType Junction -Path $target -Target $redirect
    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    $failure = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode','Recover','-Wave',$wave)
    Assert-Failed -Invocation $failure -Case 'fixed directory reparse substitution'
    Assert-RecoveryMutationSnapshotEqual -Before $before -After (Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave) -Case 'fixed directory reparse substitution'
}

function Assert-NativeDirectoryIdentitySwapRejection {
    param([Parameter(Mandatory)]$Fixture, [Parameter(Mandatory)][int]$Ordinal)

    if (-not $IsWindows) { throw 'native identity-swap contract fixture requires Windows.' }
    $wave = "identity-swap-$Ordinal"
    $initializeArguments = @('-Mode','Initialize','-Wave',$wave,'-Unit',"identity-swap-base-$Ordinal",'-NewTip',$Fixture.ExecutionBaseline,'-MicroplanSha256',$Fixture.MicroplanSha256)
    $initialize = Invoke-Updater -Repository $Fixture.Repository -Arguments $initializeArguments
    Assert-Contract ($initialize.ExitCode -eq 0) 'identity-swap fixture initialization failed'
    $candidate = New-CommitObject -Repository $Fixture.Repository -Parent $Fixture.ExecutionBaseline -Message "identity swap candidate $Ordinal"
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    $rows = Join-Path $waveRoot 'journal/rows'
    $originalRowsIdentity = Get-NativePathIdentity $rows
    $barrierRoot = Join-Path $Fixture.SuiteRoot "identity-swap-barrier-$Ordinal"
    $null = New-Item -ItemType Directory -Path $barrierRoot
    $running = Start-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode','Advance','-Wave',$wave,'-Unit',"identity-swap-$Ordinal",'-OldTip',$Fixture.ExecutionBaseline,'-NewTip',$candidate,'-MicroplanSha256',$Fixture.MicroplanSha256) -Environment @{
        DYNAMO_REMEDIATION_TEST_BARRIER = 'AfterNormalSnapshot'
        DYNAMO_REMEDIATION_TEST_BARRIER_ROOT = $barrierRoot
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (@(Get-ChildItem -LiteralPath $barrierRoot -File -Force | Where-Object { $_.Name.StartsWith('AfterNormalSnapshot.', [System.StringComparison]::Ordinal) }).Count -lt 1) {
        if ([DateTime]::UtcNow -ge $deadline) { throw 'identity-swap updater did not reach the post-snapshot barrier' }
        Start-Sleep -Milliseconds 20
    }

    $backup = Join-Path $Fixture.SuiteRoot "identity-swap-original-rows-$Ordinal"
    $sourceAcl = [System.IO.FileSystemAclExtensions]::GetAccessControl([System.IO.DirectoryInfo]::new($rows))
    Move-Item -LiteralPath $rows -Destination $backup
    $null = New-Item -ItemType Directory -Path $rows
    [System.IO.FileSystemAclExtensions]::SetAccessControl([System.IO.DirectoryInfo]::new($rows), $sourceAcl)
    foreach ($sourceFile in Get-ChildItem -LiteralPath $backup -File -Force) {
        $destination = Join-Path $rows $sourceFile.Name
        Copy-FileExact -Source $sourceFile.FullName -Destination $destination
        $fileAcl = [System.IO.FileSystemAclExtensions]::GetAccessControl([System.IO.FileInfo]::new($sourceFile.FullName))
        [System.IO.FileSystemAclExtensions]::SetAccessControl([System.IO.FileInfo]::new($destination), $fileAcl)
    }
    Assert-Contract ((Get-NativePathIdentity $rows) -cne $originalRowsIdentity) 'identity-swap fixture did not replace the rows directory native identity'
    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    [System.IO.File]::WriteAllText((Join-Path $barrierRoot 'AfterNormalSnapshot.controller.ready'), "AfterNormalSnapshot`n", [System.Text.UTF8Encoding]::new($false))
    $failure = Complete-Updater -Running $running
    Assert-Failed -Invocation $failure -Case 'post-snapshot native directory identity swap'
    Assert-RecoveryMutationSnapshotEqual -Before $before -After (Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave) -Case 'post-snapshot native directory identity swap'
}

function Replace-ControlFileIdentityPreservingContract {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$BackupPath
    )

    if (-not $IsWindows) { throw 'native control-file identity replacement requires Windows.' }
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $oldIdentity = Get-NativePathIdentity $Path
    $oldOwner = Get-PathOwnerDescriptor $Path
    $oldAcl = Get-PathAclDescriptor $Path
    $security = [System.IO.FileSystemAclExtensions]::GetAccessControl([System.IO.FileInfo]::new($Path))
    Move-Item -LiteralPath $Path -Destination $BackupPath
    [System.IO.File]::WriteAllBytes($Path, $bytes)
    [System.IO.FileSystemAclExtensions]::SetAccessControl([System.IO.FileInfo]::new($Path), $security)
    Assert-Contract (([System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData([System.IO.File]::ReadAllBytes($Path))).ToLowerInvariant()) -ceq ([System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant())) 'control-file replacement changed bytes'
    Assert-Contract ((Get-PathOwnerDescriptor $Path) -ceq $oldOwner) 'control-file replacement changed owner'
    Assert-Contract ((Get-PathAclDescriptor $Path) -ceq $oldAcl) 'control-file replacement changed ACL'
    $newIdentity = Get-NativePathIdentity $Path
    Assert-Contract ($newIdentity -cne $oldIdentity) 'control-file replacement did not change native identity'
    $newIdentity
}

function Assert-ActiveRecordIdentitySwapRejection {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][ValidateSet('Lease','Claim')][string]$Kind,
        [Parameter(Mandatory)][int]$Ordinal
    )

    if (-not $IsWindows) { throw 'active-record identity-swap fixture requires Windows.' }
    $wave = "active-$($Kind.ToLowerInvariant())-swap-$Ordinal"
    $arguments = @('-Mode','Initialize','-Wave',$wave,'-Unit',"active-$($Kind.ToLowerInvariant())-swap-$Ordinal",'-NewTip',$Fixture.ExecutionBaseline,'-MicroplanSha256',$Fixture.MicroplanSha256)
    $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
    $barrierName = if ($Kind -eq 'Lease') { 'AfterActiveLeaseRead' } else { 'AfterActiveClaimRead' }
    $barrierRoot = Join-Path $Fixture.SuiteRoot "active-$($Kind.ToLowerInvariant())-swap-barrier-$Ordinal"
    $null = New-Item -ItemType Directory -Path $barrierRoot
    if ($Kind -eq 'Claim') {
        Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments -Environment @{ DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterLeaseCreate' }) -Case 'active-claim identity-swap seed'
        $running = Start-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode','Recover','-Wave',$wave) -Environment @{
            DYNAMO_REMEDIATION_TEST_BARRIER = $barrierName
            DYNAMO_REMEDIATION_TEST_BARRIER_ROOT = $barrierRoot
        }
        $target = Join-Path $waveRoot 'recovery/active-recovery.claim'
    }
    else {
        $running = Start-Updater -Repository $Fixture.Repository -Arguments $arguments -Environment @{
            DYNAMO_REMEDIATION_TEST_BARRIER = $barrierName
            DYNAMO_REMEDIATION_TEST_BARRIER_ROOT = $barrierRoot
        }
        $target = Join-Path $waveRoot 'active-ref-update.lock'
    }

    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (@(Get-ChildItem -LiteralPath $barrierRoot -File -Force | Where-Object { $_.Name.StartsWith("$barrierName.", [System.StringComparison]::Ordinal) }).Count -lt 1) {
        if ([DateTime]::UtcNow -ge $deadline) { throw "$Kind identity-swap updater did not reach $barrierName" }
        Start-Sleep -Milliseconds 20
    }
    Assert-Contract (Test-Path -LiteralPath $target -PathType Leaf) "$Kind identity-swap target is missing at the barrier"
    $replacementIdentity = Replace-ControlFileIdentityPreservingContract -Path $target -BackupPath (Join-Path $Fixture.SuiteRoot "active-$($Kind.ToLowerInvariant())-swap-original-$Ordinal")
    $before = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    [System.IO.File]::WriteAllText((Join-Path $barrierRoot "$barrierName.controller.ready"), "$barrierName`n", [System.Text.UTF8Encoding]::new($false))
    $failure = Complete-Updater -Running $running
    Assert-Failed -Invocation $failure -Case "active $Kind identical-record native identity swap"
    $after = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave $wave
    Assert-RecoveryMutationSnapshotEqual -Before $before -After $after -Case "active $Kind identical-record native identity swap"
    Assert-Contract ((Get-NativePathIdentity $target) -ceq $replacementIdentity) "active $Kind record changed again after swap rejection"
    Assert-Contract ($null -eq (Get-WaveRef -Repository $Fixture.Repository -Wave $wave)) "active $Kind identity swap changed the authoritative ref"
    Assert-Contract (@(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'journal/rows') -Force).Count -eq 0) "active $Kind identity swap appended a journal row"
}

function Assert-CompletionInventoryRejectsExtraDirectory {
    param([Parameter(Mandatory)]$Fixture, [Parameter(Mandatory)][int]$Ordinal)

    $wave = "completion-extra-directory-$Ordinal"
    $arguments = @('-Mode','Initialize','-Wave',$wave,'-Unit',"completion-extra-$Ordinal",'-NewTip',$Fixture.ExecutionBaseline,'-MicroplanSha256',$Fixture.MicroplanSha256)
    $complete = Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments
    Assert-Contract ($complete.ExitCode -eq 0) 'completion-extra-directory fixture initialization failed'
    $extra = Join-Path (Get-WaveStateRoot -Fixture $Fixture -Wave $wave) 'journal/rows/extra-directory'
    $null = New-Item -ItemType Directory -Path $extra
    $rejected = $false
    try { Assert-StateCompletion -Fixture $Fixture -Wave $wave -Expected Normal }
    catch { $rejected = $true }
    Assert-Contract $rejected 'completion oracle accepted an extra directory hidden from file-only enumeration'
}

function New-IntegrationFixture {
    param(
        [Parameter(Mandatory)][string]$SourceRepository,
        [Parameter(Mandatory)][string]$SuiteRoot
    )

    $repository = Join-Path $SuiteRoot 'repository'
    $linked = Join-Path $SuiteRoot 'linked-worktree'
    $evidence = Join-Path $SuiteRoot 'external-evidence'
    $null = New-Item -ItemType Directory -Path $evidence
    $clone = @(& git clone --quiet --no-local --no-hardlinks -- $SourceRepository $repository 2>&1)
    Assert-Contract ($LASTEXITCODE -eq 0) "temporary clone failed: $($clone -join [Environment]::NewLine)"
    $null = Invoke-Git -WorkingDirectory $repository -Arguments @('config', 'user.name', 'Dynamo Contract Fixture')
    $null = Invoke-Git -WorkingDirectory $repository -Arguments @('config', 'user.email', 'fixture.invalid@localhost')
    $null = Invoke-Git -WorkingDirectory $repository -Arguments @('config', 'core.autocrlf', 'false')
    $null = Invoke-Git -WorkingDirectory $repository -Arguments @('config', 'core.logAllRefUpdates', 'always')
    $null = Invoke-Git -WorkingDirectory $repository -Arguments @('worktree', 'add', '--quiet', '--detach', $linked, 'HEAD')

    foreach ($name in $PlanNames) {
        Copy-FileExact -Source (Join-Path $SourceRepository "docs/superpowers/plans/$name") -Destination (Join-Path $repository "docs/superpowers/plans/$name")
    }
    $updaterSource = Join-Path $SourceRepository 'scripts/remediation/update-integration-ref.ps1'
    if (-not (Test-Path -LiteralPath $updaterSource -PathType Leaf)) {
        throw 'integration-ref-journal-contract: expected implementation missing: scripts/remediation/update-integration-ref.ps1'
    }
    $publisherSource = Join-Path $SourceRepository 'scripts/remediation/publish-plan-set.ps1'
    if (-not (Test-Path -LiteralPath $publisherSource -PathType Leaf)) {
        throw 'integration-ref-journal-contract: expected implementation missing: scripts/remediation/publish-plan-set.ps1'
    }
    foreach ($relative in $BootstrapPaths) {
        $source = Join-Path $SourceRepository $relative
        Assert-Contract (Test-Path -LiteralPath $source -PathType Leaf) "bootstrap fixture input is missing: $relative"
    }

    $clonedHead = (Invoke-Git -WorkingDirectory $repository -Arguments @('rev-parse', 'HEAD')).Output[-1].Trim()
    $clonedParents = @(((Invoke-Git -WorkingDirectory $repository -Arguments @('show', '-s', '--format=%P', $clonedHead)).Output[-1].Trim() -split ' ') | Where-Object { $_ })
    $clonedChanged = @((Invoke-Git -WorkingDirectory $repository -Arguments @('diff-tree', '--no-commit-id', '--name-only', '-r', $clonedHead)).Output | Sort-Object -CaseSensitive)
    $expectedChanged = @($BootstrapPaths | Sort-Object -CaseSensitive)
    $alreadyBootstrap = $clonedParents.Count -eq 1 -and $clonedParents[0] -ceq $AuditBaseline -and (($clonedChanged -join "`n") -ceq ($expectedChanged -join "`n"))
    if (-not $alreadyBootstrap) {
        $null = Invoke-Git -WorkingDirectory $repository -Arguments @('reset', '--hard', $AuditBaseline)
        foreach ($relative in $BootstrapPaths) {
            Copy-FileExact -Source (Join-Path $SourceRepository $relative) -Destination (Join-Path $repository $relative)
        }
        $bootstrapAcl = [System.IO.FileSystemAclExtensions]::GetAccessControl([System.IO.FileInfo]::new((Join-Path $repository 'scripts/remediation/publish-plan-set.ps1')))
        foreach ($relative in $BootstrapPaths) {
            [System.IO.FileSystemAclExtensions]::SetAccessControl([System.IO.FileInfo]::new((Join-Path $repository $relative)), $bootstrapAcl)
        }
        $null = Invoke-Git -WorkingDirectory $repository -Arguments (@('add', '--') + $BootstrapPaths)
        $null = Invoke-Git -WorkingDirectory $repository -Arguments @('commit', '--quiet', '-m', 'fixture: install bootstrap contract inputs')
    }
    $executionBaseline = (Invoke-Git -WorkingDirectory $repository -Arguments @('rev-parse', 'HEAD')).Output[-1].Trim()
    $parents = @(((Invoke-Git -WorkingDirectory $repository -Arguments @('show', '-s', '--format=%P', $executionBaseline)).Output[-1].Trim() -split ' ') | Where-Object { $_ })
    $changed = @((Invoke-Git -WorkingDirectory $repository -Arguments @('diff-tree', '--no-commit-id', '--name-only', '-r', $executionBaseline)).Output | Sort-Object -CaseSensitive)
    Assert-Contract ($parents.Count -eq 1 -and $parents[0] -ceq $AuditBaseline) 'fixture execution commit does not have the audit baseline as sole parent'
    Assert-Contract (($changed -join "`n") -ceq ($expectedChanged -join "`n")) 'fixture execution commit does not change exactly the four bootstrap files'
    $null = Invoke-Git -WorkingDirectory $linked -Arguments @('reset', '--hard', $executionBaseline)
    $audit = Invoke-Git -WorkingDirectory $repository -Arguments @('cat-file', '-e', "$AuditBaseline^{commit}") -AllowFailure
    Assert-Contract ($audit.ExitCode -eq 0) 'fixed audit baseline is absent from fixture clone'
    $handoff = Invoke-Publisher -Repository $repository -EvidenceRoot $evidence
    $common = (Invoke-Git -WorkingDirectory $repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()
    $bindingPath = Join-Path $common 'dynamo-remediation/plan-set-binding-v1.json'
    Assert-Contract (Test-Path -LiteralPath $bindingPath -PathType Leaf) 'fixture publisher did not create the fixed binding leaf'
    $binding = Get-Content -LiteralPath $bindingPath -Raw | ConvertFrom-Json -ErrorAction Stop
    Assert-Contract ($binding.execution_baseline -ceq $executionBaseline) 'fixture binding execution baseline mismatch'
    Assert-Contract ($binding.plan_set_sha256 -match '^[0-9a-f]{64}$') 'fixture binding plan-set hash is malformed'
    $linkedCommon = (Invoke-Git -WorkingDirectory $linked -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()
    Assert-Contract ([System.IO.Path]::GetFullPath($common) -ceq [System.IO.Path]::GetFullPath($linkedCommon)) 'linked worktrees do not share one physical common directory'
    [pscustomobject]@{
        SuiteRoot = $SuiteRoot
        Repository = $repository
        LinkedWorktree = $linked
        EvidenceRoot = $evidence
        ExecutionBaseline = $executionBaseline
        PlanSetSha256 = $binding.plan_set_sha256
        CommonDirectory = [System.IO.Path]::GetFullPath($common)
        MicroplanSha256 = (Get-FileHash -LiteralPath (Join-Path $repository 'docs/superpowers/plans/2026-07-13-wave0-bootstrap.md') -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}

function Assert-IntegrationContract {
    param([Parameter(Mandatory)]$Fixture)

    $knownRow = '{"schema_version":1,"seq":0,"phase":"Intent","wave":"wave0","unit":"fixture","attempt_id":"00000000000000000000000000000001","old_tip":"0000000000000000000000000000000000000000","new_tip":"1111111111111111111111111111111111111111","microplan_sha256":"2222222222222222222222222222222222222222222222222222222222222222","lease_sha256":"3333333333333333333333333333333333333333333333333333333333333333","recovery_generation":0,"recovery_claim_sha256":"0000000000000000000000000000000000000000000000000000000000000000","utc":"2026-07-13T00:00:00.0000000Z","prev_row_sha256":"0000000000000000000000000000000000000000000000000000000000000000"}'
    Assert-Contract ((Get-DomainHash -Domain 'dynamo-integration-row-v1' -CanonicalJson ($knownRow + "`n")) -ceq 'b7dc3a6a36cdcec949bdab9daa8e7e48b28d332b00e04e058bf3266d5fa3affe') 'canonical integration-row known vector mismatch'
    $updater = Join-Path $Fixture.Repository 'scripts/remediation/update-integration-ref.ps1'
    $tokens = $null
    $errors = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile($updater, [ref]$tokens, [ref]$errors)
    Assert-Contract ($errors.Count -eq 0) 'integration updater does not parse'

    $forbiddenBefore = Get-TreeFingerprint -Root (Join-Path $Fixture.CommonDirectory 'dynamo-remediation/integration-state-v1')
    foreach ($arguments in @(
        @('-Mode', 'Recover', '-Wave', 'wave0', '-StateRoot', 'x'),
        @('-Mode', 'Recover', '-Wave', 'wave0', '-Unit', 'x'),
        @('-Mode', 'Recover', '-Wave', 'wave0', '-OldTip', $ZeroOid),
        @('-Mode', 'Initialize', '-Wave', 'wave0', '-Unit', 'bootstrap', '-OldTip', $ZeroOid, '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256),
        @('-Mode', 'Advance', '-Wave', 'wave0', '-Unit', 'x', '-OldTip', $Fixture.ExecutionBaseline, '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256, '-BindingPath', 'x'),
        @('-Mode', 'Initialize', '-Wave', '../wave', '-Unit', 'x', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256),
        @('-Mode', 'Initialize', '-Wave', 'Wave0', '-Unit', 'x', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256),
        @('-Mode', 'Initialize', '-Wave', 'con', '-Unit', 'x', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256),
        @('-Mode', 'Initialize', '-Wave', 'wave0', '-Unit', '../unit', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256),
        @('-Mode', 'Initialize', '-Wave', 'wave0', '-Unit', 'Unit', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
    )) {
        Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments) -Case "forbidden parameter surface: $($arguments -join ' ')"
        Assert-Contract (($forbiddenBefore -join "`n") -ceq ((Get-TreeFingerprint -Root (Join-Path $Fixture.CommonDirectory 'dynamo-remediation/integration-state-v1')) -join "`n")) 'forbidden parameter input mutated integration state'
    }

    $failpointGuardBefore = Get-TreeFingerprint -Root (Join-Path $Fixture.CommonDirectory 'dynamo-remediation/integration-state-v1')
    $unguardedFailpoint = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Initialize', '-Wave', 'wave0', '-Unit', 'failpoint-guard', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256) -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '0'
        DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterRefCas'
    }
    Assert-Failed -Invocation $unguardedFailpoint -Case 'valid failpoint without explicit test mode'
    $unknownFailpoint = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Initialize', '-Wave', 'failpoint-unknown', '-Unit', 'failpoint-unknown', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256) -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterUnknownBoundary'
    }
    Assert-Failed -Invocation $unknownFailpoint -Case 'unknown failpoint'
    Assert-Contract (($failpointGuardBefore -join "`n") -ceq ((Get-TreeFingerprint -Root (Join-Path $Fixture.CommonDirectory 'dynamo-remediation/integration-state-v1')) -join "`n")) 'invalid failpoint configuration mutated integration state'
    Assert-Contract ($null -eq (Get-WaveRef -Repository $Fixture.Repository -Wave 'wave0')) 'unguarded failpoint invocation changed the wave0 ref'
    Assert-Contract ($null -eq (Get-WaveRef -Repository $Fixture.Repository -Wave 'failpoint-unknown')) 'unknown failpoint invocation changed its ref'

    $productionWave = 'production-denied'
    $productionRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $productionWave
    $productionInitialize = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Initialize', '-Wave', $productionWave, '-Unit', $productionWave, '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256) -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '0'
    }
    Assert-Failed -Invocation $productionInitialize -Case 'production initialize outside wave0'
    Assert-Contract (-not (Test-Path -LiteralPath $productionRoot)) 'production non-wave0 Initialize created control state before rejection'
    Assert-Contract ($null -eq (Get-WaveRef -Repository $Fixture.Repository -Wave $productionWave)) 'production non-wave0 Initialize changed its ref'

    $malformedRefPath = Join-Path $Fixture.CommonDirectory 'refs/dynamo-remediation/malformed-ref/integration'
    $null = New-Item -ItemType Directory -Path (Split-Path -Parent $malformedRefPath) -Force
    [System.IO.File]::WriteAllText($malformedRefPath, "not-an-object-id`n", [System.Text.UTF8Encoding]::new($false))
    try {
        $malformedRef = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Initialize', '-Wave', 'malformed-ref', '-Unit', 'malformed-ref', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
        Assert-Failed -Invocation $malformedRef -Case 'malformed direct ref'
        Assert-Contract ((Get-Content -LiteralPath $malformedRefPath -Raw) -ceq "not-an-object-id`n") 'malformed direct ref was rewritten or removed'
    }
    finally {
        Remove-Item -LiteralPath $malformedRefPath -Force
    }

    $orphanAdvanceWave = 'advance-without-state'
    $orphanAdvanceRef = "refs/dynamo-remediation/$orphanAdvanceWave/integration"
    $orphanAdvanceTip = New-CommitObject -Repository $Fixture.Repository -Parent $Fixture.ExecutionBaseline -Message 'advance without initialized state'
    $null = Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('update-ref', '--no-deref', $orphanAdvanceRef, $Fixture.ExecutionBaseline, $ZeroOid)
    try {
        $orphanAdvanceRefBefore = @(Get-TreeFingerprint -Root (Join-Path $Fixture.CommonDirectory "refs/dynamo-remediation/$orphanAdvanceWave"))
        $orphanAdvance = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Advance', '-Wave', $orphanAdvanceWave, '-Unit', $orphanAdvanceWave, '-OldTip', $Fixture.ExecutionBaseline, '-NewTip', $orphanAdvanceTip, '-MicroplanSha256', $Fixture.MicroplanSha256)
        Assert-Failed -Invocation $orphanAdvance -Case 'Advance without initialized wave state'
        Assert-Contract (-not (Test-Path -LiteralPath (Get-WaveStateRoot -Fixture $Fixture -Wave $orphanAdvanceWave))) 'Advance without initialized state created a wave directory'
        Assert-Contract (($orphanAdvanceRefBefore -join "`n") -ceq ((Get-TreeFingerprint -Root (Join-Path $Fixture.CommonDirectory "refs/dynamo-remediation/$orphanAdvanceWave")) -join "`n")) 'Advance without initialized state changed its direct ref tree'
    }
    finally {
        $null = Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('update-ref', '-d', $orphanAdvanceRef, $Fixture.ExecutionBaseline) -AllowFailure
    }

    $initialize = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Initialize', '-Wave', 'wave0', '-Unit', 'bootstrap', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
    Assert-Contract ($initialize.ExitCode -eq 0) "normal initialization failed: $($initialize.Output -join [Environment]::NewLine)"
    Assert-Contract ((Get-WaveRef -Repository $Fixture.LinkedWorktree -Wave 'wave0') -ceq $Fixture.ExecutionBaseline) 'linked worktree did not observe initialized ref'
    $wave0Root = Get-WaveStateRoot -Fixture $Fixture -Wave 'wave0'
    Assert-CanonicalRows -WaveRoot $wave0Root -Repository $Fixture.Repository
    Assert-StateCompletion -Fixture $Fixture -Wave 'wave0' -Expected Normal

    $normalCompleteBefore = Get-TreeFingerprint -Root $wave0Root
    $normalComplete = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Recover', '-Wave', 'wave0')
    Assert-Contract ($normalComplete.ExitCode -eq 0) "NormalComplete recovery read failed: $($normalComplete.Output -join [Environment]::NewLine)"
    $normalCompleteAfter = Get-TreeFingerprint -Root $wave0Root
    Assert-Contract (($normalCompleteBefore -join "`n") -ceq ($normalCompleteAfter -join "`n")) 'NormalComplete recovery was not read-only'

    $advanceTip = New-CommitObject -Repository $Fixture.Repository -Parent $Fixture.ExecutionBaseline -Message 'advance tip'
    $advance = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Advance', '-Wave', 'wave0', '-Unit', 'advance-one', '-OldTip', $Fixture.ExecutionBaseline, '-NewTip', $advanceTip, '-MicroplanSha256', $Fixture.MicroplanSha256)
    Assert-Contract ($advance.ExitCode -eq 0) "normal advance failed: $($advance.Output -join [Environment]::NewLine)"
    Assert-Contract ((Get-WaveRef -Repository $Fixture.Repository -Wave 'wave0') -ceq $advanceTip) 'primary worktree did not observe linked-worktree advance'
    Assert-CanonicalRows -WaveRoot $wave0Root -Repository $Fixture.Repository
    Assert-StateCompletion -Fixture $Fixture -Wave 'wave0' -Expected Normal

    $wave0RefName = 'refs/dynamo-remediation/wave0/integration'
    $externalDriftTip = New-CommitObject -Repository $Fixture.Repository -Parent $advanceTip -Message 'external integration ref drift'
    $postDriftCandidate = New-CommitObject -Repository $Fixture.Repository -Parent $externalDriftTip -Message 'candidate after external drift'
    $null = Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('update-ref', '--no-deref', $wave0RefName, $externalDriftTip, $advanceTip)
    try {
        $driftBefore = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave 'wave0'
        $driftAdvance = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Advance', '-Wave', 'wave0', '-Unit', 'external-drift', '-OldTip', $externalDriftTip, '-NewTip', $postDriftCandidate, '-MicroplanSha256', $Fixture.MicroplanSha256)
        Assert-Failed -Invocation $driftAdvance -Case 'Advance after external direct-ref drift'
        Assert-RecoveryMutationSnapshotEqual -Before $driftBefore -After (Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave 'wave0') -Case 'Advance after external direct-ref drift'
    }
    finally {
        $null = Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('update-ref', '--no-deref', $wave0RefName, $advanceTip, $externalDriftTip) -AllowFailure
    }

    $null = Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('update-ref', '-d', $wave0RefName, $advanceTip)
    try {
        $deletedRefBefore = Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave 'wave0'
        $reinitializeDeletedRef = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Initialize', '-Wave', 'wave0', '-Unit', 'reinitialize-deleted-ref', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
        Assert-Failed -Invocation $reinitializeDeletedRef -Case 'Initialize after completed ref deletion'
        Assert-RecoveryMutationSnapshotEqual -Before $deletedRefBefore -After (Get-RecoveryMutationSnapshot -Fixture $Fixture -Wave 'wave0') -Case 'Initialize after completed ref deletion'
    }
    finally {
        $null = Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('update-ref', '--no-deref', $wave0RefName, $advanceTip, $ZeroOid) -AllowFailure
    }

    $decoy = Join-Path $Fixture.EvidenceRoot 'decoy-state-root'
    $null = New-Item -ItemType Directory -Path $decoy
    [System.IO.File]::WriteAllText((Join-Path $decoy 'sentinel'), 'unchanged', [System.Text.UTF8Encoding]::new($false))
    $redirectEnvironment = @{
        DYNAMO_REMEDIATION_STATE_ROOT = $decoy
        DYNAMO_REMEDIATION_BINDING_PATH = (Join-Path $decoy 'copied-binding.json')
        DYNAMO_REMEDIATION_JOURNAL_ROOT = $decoy
        GIT_DIR = $decoy
        GIT_WORK_TREE = $decoy
    }
    Copy-Item -LiteralPath (Join-Path $Fixture.CommonDirectory 'dynamo-remediation/plan-set-binding-v1.json') -Destination $redirectEnvironment.DYNAMO_REMEDIATION_BINDING_PATH
    $decoyBefore = Get-TreeFingerprint -Root $decoy
    $redirected = Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Initialize', '-Wave', 'redirect-check', '-Unit', 'redirect-check', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256) -Environment $redirectEnvironment
    Assert-Contract ($redirected.ExitCode -eq 0) "environment redirection check failed: $($redirected.Output -join [Environment]::NewLine)"
    Assert-Contract ((Get-Content -LiteralPath (Join-Path $decoy 'sentinel') -Raw) -ceq 'unchanged') 'environment redirect mutated the decoy sentinel'
    Assert-Contract (($decoyBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $decoy) -join "`n")) 'environment redirect mutated the decoy tree'
    Assert-Contract (Test-Path -LiteralPath (Get-WaveStateRoot -Fixture $Fixture -Wave 'redirect-check')) 'redirected call did not use the fixed common-dir state root'

    for ($iteration = 0; $iteration -lt 20; $iteration++) {
        $wave = "race-$iteration"
        $init = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', 'race-base', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
        Assert-Contract ($init.ExitCode -eq 0) "race fixture initialization $iteration failed"
        $leftTip = New-CommitObject -Repository $Fixture.Repository -Parent $Fixture.ExecutionBaseline -Message "race left $iteration"
        $rightTip = New-CommitObject -Repository $Fixture.Repository -Parent $Fixture.ExecutionBaseline -Message "race right $iteration"
        $barrierRoot = Join-Path $Fixture.SuiteRoot "normal-race-barrier-$iteration"
        $null = New-Item -ItemType Directory -Path $barrierRoot
        $barrierEnvironment = @{ DYNAMO_REMEDIATION_TEST_BARRIER = 'BeforeLeaseCreate'; DYNAMO_REMEDIATION_TEST_BARRIER_ROOT = $barrierRoot }
        $left = Start-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Advance', '-Wave', $wave, '-Unit', "left-$iteration", '-OldTip', $Fixture.ExecutionBaseline, '-NewTip', $leftTip, '-MicroplanSha256', $Fixture.MicroplanSha256) -Environment $barrierEnvironment
        $right = Start-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Advance', '-Wave', $wave, '-Unit', "right-$iteration", '-OldTip', $Fixture.ExecutionBaseline, '-NewTip', $rightTip, '-MicroplanSha256', $Fixture.MicroplanSha256) -Environment $barrierEnvironment
        $leftResult = Complete-Updater -Running $left
        $rightResult = Complete-Updater -Running $right
        $successCount = @(@($leftResult, $rightResult) | Where-Object { $_.ExitCode -eq 0 }).Count
        Assert-Contract ($successCount -eq 1) "race $iteration did not produce exactly one winner"
        $winnerTip = Get-WaveRef -Repository $Fixture.Repository -Wave $wave
        Assert-Contract ($winnerTip -in @($leftTip, $rightTip)) "race $iteration ref does not equal either candidate"
        Assert-CanonicalRows -WaveRoot (Get-WaveStateRoot -Fixture $Fixture -Wave $wave) -Repository $Fixture.Repository
        Assert-StateCompletion -Fixture $Fixture -Wave $wave -Expected Normal
    }

    for ($iteration = 0; $iteration -lt 5; $iteration++) {
        $wave = "recovery-race-$iteration"
        $arguments = @('-Mode', 'Initialize', '-Wave', $wave, '-Unit', "recovery-race-$iteration", '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)
        $seed = Invoke-Updater -Repository $Fixture.Repository -Arguments $arguments -Environment @{
            DYNAMO_REMEDIATION_TEST_MODE = '1'
            DYNAMO_REMEDIATION_TEST_FAILPOINT = 'AfterIntentTempFsync'
        }
        Assert-Failed -Invocation $seed -Case "recovery race seed $iteration"
        $barrierRoot = Join-Path $Fixture.SuiteRoot "recovery-race-barrier-$iteration"
        $null = New-Item -ItemType Directory -Path $barrierRoot
        $barrierEnvironment = @{ DYNAMO_REMEDIATION_TEST_BARRIER = 'BeforeClaimCreate'; DYNAMO_REMEDIATION_TEST_BARRIER_ROOT = $barrierRoot }
        $left = Start-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave) -Environment $barrierEnvironment
        $right = Start-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Recover', '-Wave', $wave) -Environment $barrierEnvironment
        $leftResult = Complete-Updater -Running $left
        $rightResult = Complete-Updater -Running $right
        Assert-Contract (@(@($leftResult, $rightResult) | Where-Object { $_.ExitCode -eq 0 }).Count -eq 1) "recovery race $iteration did not produce exactly one deterministic claim winner"
        $finalRead = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Recover', '-Wave', $wave)
        Assert-Contract ($finalRead.ExitCode -eq 0) "recovery race $iteration did not converge: $($finalRead.Output -join [Environment]::NewLine)"
        $waveRoot = Get-WaveStateRoot -Fixture $Fixture -Wave $wave
        Assert-CanonicalRows -WaveRoot $waveRoot -Repository $Fixture.Repository
        Assert-StateCompletion -Fixture $Fixture -Wave $wave -Expected Recovered
        Assert-Contract (@(Get-ChildItem -LiteralPath (Join-Path $waveRoot 'journal/orphans') -File -Force).Count -eq 1) "recovery race $iteration did not preserve exactly one immutable temp orphan"
    }

    $normalFailpoints = @(
        'AfterLeaseCreate',
        'AfterIntentTempFsync',
        'AfterIntentRowRename',
        'AfterRefCas',
        'AfterTerminalTempFsync',
        'AfterTerminalRowRename',
        'AfterLeaseRename'
    )
    for ($index = 0; $index -lt $normalFailpoints.Count; $index++) {
        Assert-NormalFailpointRecovery -Fixture $Fixture -Failpoint $normalFailpoints[$index] -Ordinal $index
    }
    Assert-RecoveryFailpointChain -Fixture $Fixture -RecoveryFailpoints @('AfterClaimCreate') -Ordinal 0
    Assert-RecoveryFailpointChain -Fixture $Fixture -RecoveryFailpoints @('AfterClaimCreate', 'AfterClaimArchive') -Ordinal 1
    Assert-RecoveryFailpointChain -Fixture $Fixture -RecoveryFailpoints @('AfterRecoveryTerminalRowRename') -Ordinal 2
    Assert-RecoveryFailpointChain -Fixture $Fixture -RecoveryFailpoints @('AfterClaimRename') -Ordinal 3
    Assert-RecoveryFailpointChain -Fixture $Fixture -RecoveryFailpoints @('AfterLeaseRename', 'AfterClaimArchive') -Ordinal 4 -SeedFailpoint 'AfterTerminalRowRename'
    Assert-RecoveryFailpointChain -Fixture $Fixture -RecoveryFailpoints @('AfterClaimCreate', 'AfterClaimTakeover') -Ordinal 5
    Assert-RecoveryFailpointChain -Fixture $Fixture -RecoveryFailpoints @('AfterIntentTempFsync') -Ordinal 6 -SeedFailpoint 'AfterLeaseCreate'
    Assert-RecoveryFailpointChain -Fixture $Fixture -RecoveryFailpoints @('AfterIntentRowRename') -Ordinal 7 -SeedFailpoint 'AfterLeaseCreate'
    Assert-RecoveryFailpointChain -Fixture $Fixture -RecoveryFailpoints @('AfterTerminalTempFsync') -Ordinal 8 -SeedFailpoint 'AfterLeaseCreate'

    for ($iteration = 0; $iteration -lt 3; $iteration++) {
        Assert-StaleClaimRecoveryRace -Fixture $Fixture -Ordinal $iteration
    }
    $corruptKinds = @('Third', 'Missing', 'Malformed', 'Symbolic')
    for ($index = 0; $index -lt $corruptKinds.Count; $index++) {
        Assert-CorruptRefRecoveryReadOnly -Fixture $Fixture -Kind $corruptKinds[$index] -Ordinal $index
    }
    $corruptShapes = @('Shape0NoClaim', 'ShapeANoClaim', 'ShapeBActiveClaim', 'ShapeCArchiveOnly')
    for ($index = 0; $index -lt $corruptShapes.Count; $index++) {
        Assert-CorruptRecoveryShapeReadOnly -Fixture $Fixture -ShapeCase $corruptShapes[$index] -Ordinal $index
    }
    Assert-ExactClaimTupleRejection -Fixture $Fixture -Ordinal 0
    Assert-DuplicateJournalRowRejection -Fixture $Fixture -Kind Intent -Ordinal 0
    Assert-DuplicateJournalRowRejection -Fixture $Fixture -Kind Terminal -Ordinal 1
    $canonicalNegativeKinds = @('DuplicateKey','Bom','Whitespace','Timestamp')
    for ($index = 0; $index -lt $canonicalNegativeKinds.Count; $index++) {
        Assert-CanonicalEncodingRejection -Fixture $Fixture -Kind $canonicalNegativeKinds[$index] -Ordinal $index
    }
    $scalarNegativeKinds = @(
        'LeaseSchemaString','LeaseExpectedSeqString','LeasePidString','LeaseInvalidCalendar',
        'RowSeqString','RowRecoveryGenerationString','RowInvalidCalendar',
        'ClaimGenerationString','ClaimPidString','ClaimInvalidCalendar'
    )
    for ($index = 0; $index -lt $scalarNegativeKinds.Count; $index++) {
        Assert-CoherentScalarRejection -Fixture $Fixture -Kind $scalarNegativeKinds[$index] -Ordinal $index
    }
    Assert-OwnerIdentitySemantics -Fixture $Fixture -Kind LiveSameBirth -Ordinal 0
    Assert-OwnerIdentitySemantics -Fixture $Fixture -Kind RemoteMachine -Ordinal 1
    Assert-OwnerIdentitySemantics -Fixture $Fixture -Kind PidReuse -Ordinal 2
    Assert-ProcessQueryAccessDeniedRejection -Fixture $Fixture -Ordinal 0
    Assert-ArchivedClaimLiveOwnerRejection -Fixture $Fixture -Ordinal 0
    Assert-ControlFileAclRejection -Fixture $Fixture -Ordinal 0
    Assert-ControlDirectorySecurityRejection -Fixture $Fixture -Kind ExtraAce -Ordinal 0
    Assert-ControlDirectorySecurityRejection -Fixture $Fixture -Kind WrongOwner -Ordinal 1
    Assert-ControlDirectorySecurityRejection -Fixture $Fixture -Kind MissingSystemAce -Ordinal 2
    Assert-ReparseDirectoryRejection -Fixture $Fixture -Ordinal 0
    Assert-NativeDirectoryIdentitySwapRejection -Fixture $Fixture -Ordinal 0
    Assert-ActiveRecordIdentitySwapRejection -Fixture $Fixture -Kind Lease -Ordinal 0
    Assert-ActiveRecordIdentitySwapRejection -Fixture $Fixture -Kind Claim -Ordinal 1
    Assert-CompletionInventoryRejectsExtraDirectory -Fixture $Fixture -Ordinal 0

    $staleTip = New-CommitObject -Repository $Fixture.Repository -Parent $Fixture.ExecutionBaseline -Message 'stale tip'
    $staleBefore = Get-TreeFingerprint -Root $wave0Root
    $staleRefBefore = Get-WaveRef -Repository $Fixture.Repository -Wave 'wave0'
    $stale = Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Advance', '-Wave', 'wave0', '-Unit', 'stale-old-tip', '-OldTip', $Fixture.ExecutionBaseline, '-NewTip', $staleTip, '-MicroplanSha256', $Fixture.MicroplanSha256)
    Assert-Failed -Invocation $stale -Case 'stale old-tip CAS'
    Assert-Contract ((Get-WaveRef -Repository $Fixture.Repository -Wave 'wave0') -ceq $advanceTip) 'stale old-tip call changed the ref'
    Assert-Contract ((Get-WaveRef -Repository $Fixture.Repository -Wave 'wave0') -ceq $staleRefBefore) 'stale old-tip call changed the direct ref snapshot'
    Assert-Contract (($staleBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $wave0Root) -join "`n")) 'stale old-tip call mutated wave state before rejection'

    $bindingPath = Join-Path $Fixture.CommonDirectory 'dynamo-remediation/plan-set-binding-v1.json'
    $bindingBytes = [System.IO.File]::ReadAllBytes($bindingPath)
    $bindingText = [System.Text.UTF8Encoding]::new($false, $true).GetString($bindingBytes)
    $originalBinding = $bindingText | ConvertFrom-Json -ErrorAction Stop
    $manifestPath = [string]$originalBinding.manifest_native_path
    $tamperedBindingText = $bindingText.Replace('"schema_version":1', '"schema_version":2')
    Assert-Contract ($tamperedBindingText -cne $bindingText) 'binding tamper fixture did not change bytes'
    [System.IO.File]::WriteAllText($bindingPath, $tamperedBindingText, [System.Text.UTF8Encoding]::new($false))
    $bindingNegativeBefore = Get-SecurityNegativeSnapshot -Fixture $Fixture -ManifestPath $manifestPath
    $integrityRoot = Get-WaveStateRoot -Fixture $Fixture -Wave 'binding-integrity'
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.Repository -Arguments @('-Mode', 'Initialize', '-Wave', 'binding-integrity', '-Unit', 'binding-integrity', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)) -Case 'edited fixed binding'
    Assert-Contract (-not (Test-Path -LiteralPath $integrityRoot)) 'edited binding created integration state before rejection'
    Assert-SecurityNegativeSnapshotEqual -Before $bindingNegativeBefore -After (Get-SecurityNegativeSnapshot -Fixture $Fixture -ManifestPath $manifestPath) -Case 'edited fixed binding'
    [System.IO.File]::WriteAllBytes($bindingPath, $bindingBytes)

    $binding = Get-Content -LiteralPath $bindingPath -Raw | ConvertFrom-Json -ErrorAction Stop
    $manifestPath = [string]$binding.manifest_native_path
    $manifestBytes = [System.IO.File]::ReadAllBytes($manifestPath)
    [System.IO.File]::WriteAllBytes($manifestPath, $manifestBytes + [byte[]](0x20))
    $manifestNegativeBefore = Get-SecurityNegativeSnapshot -Fixture $Fixture -ManifestPath $manifestPath
    $manifestIntegrityRoot = Get-WaveStateRoot -Fixture $Fixture -Wave 'manifest-integrity'
    Assert-Failed -Invocation (Invoke-Updater -Repository $Fixture.LinkedWorktree -Arguments @('-Mode', 'Initialize', '-Wave', 'manifest-integrity', '-Unit', 'manifest-integrity', '-NewTip', $Fixture.ExecutionBaseline, '-MicroplanSha256', $Fixture.MicroplanSha256)) -Case 'edited bound manifest'
    Assert-Contract (-not (Test-Path -LiteralPath $manifestIntegrityRoot)) 'edited manifest created integration state before rejection'
    Assert-SecurityNegativeSnapshotEqual -Before $manifestNegativeBefore -After (Get-SecurityNegativeSnapshot -Fixture $Fixture -ManifestPath $manifestPath) -Case 'edited bound manifest'
    [System.IO.File]::WriteAllBytes($manifestPath, $manifestBytes)
}

if ($PSVersionTable.PSVersion -lt [version]'7.4') {
    throw 'integration-ref-journal-contract: PowerShell 7.4 or newer is required'
}

$sourceRepository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$before = Get-RepositorySnapshot -Repository $sourceRepository
$suiteRoot = Join-Path ([System.IO.Path]::GetTempPath()) "dynamo-integration-ref-contract-$PID-$([guid]::NewGuid().ToString('N'))"
$marker = Join-Path $suiteRoot '.dynamo-contract-owned'
$fixture = $null
$testError = $null
$suiteIdentity = $null
$markerHash = $null

try {
    $null = New-Item -ItemType Directory -Path $suiteRoot
    [System.IO.File]::WriteAllText($marker, 'integration-ref-journal-contract', [System.Text.UTF8Encoding]::new($false))
    $suiteIdentity = Get-OwnedRootIdentity -Root $suiteRoot
    $markerHash = (Get-FileHash -LiteralPath $marker -Algorithm SHA256).Hash.ToLowerInvariant()
    $fixture = New-IntegrationFixture -SourceRepository $sourceRepository -SuiteRoot $suiteRoot
    Assert-IntegrationContract -Fixture $fixture
}
catch {
    $testError = $_
}
finally {
    try {
        foreach ($child in @($script:UpdaterChildren)) {
            try {
                if (-not $child.HasExited) { $child.Kill($true) }
                $child.WaitForExit()
            }
            catch {
                if ($null -eq $testError) { $testError = $_ }
            }
            finally { $child.Dispose() }
        }
        $script:UpdaterChildren.Clear()
        $fullSuiteRoot = [System.IO.Path]::GetFullPath($suiteRoot)
        $fullTempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
        Assert-Contract ($fullSuiteRoot.StartsWith($fullTempRoot, [System.StringComparison]::OrdinalIgnoreCase)) 'cleanup root escaped the OS temp directory'
        Assert-Contract (Test-Path -LiteralPath $marker -PathType Leaf) 'cleanup ownership marker is missing'
        Assert-Contract ((Get-OwnedRootIdentity -Root $fullSuiteRoot) -ceq $suiteIdentity) 'cleanup suite-root native identity changed'
        Assert-Contract ((Get-FileHash -LiteralPath $marker -Algorithm SHA256).Hash.ToLowerInvariant() -ceq $markerHash) 'cleanup ownership marker bytes changed'
        $cleanupRepository = if ($null -ne $fixture) { $fixture.Repository } else { Join-Path $suiteRoot 'repository' }
        $cleanupLinked = if ($null -ne $fixture) { $fixture.LinkedWorktree } else { Join-Path $suiteRoot 'linked-worktree' }
        if (Test-Path -LiteralPath $cleanupRepository -PathType Container) {
            $null = Invoke-Git -WorkingDirectory $cleanupRepository -Arguments @('worktree', 'remove', '--force', $cleanupLinked) -AllowFailure
            $null = Invoke-Git -WorkingDirectory $cleanupRepository -Arguments @('worktree', 'prune', '--expire', 'now') -AllowFailure
        }
        Remove-Item -LiteralPath $fullSuiteRoot -Recurse -Force
    }
    catch {
        if ($null -eq $testError) { $testError = $_ }
    }
    try {
        Assert-RepositoryUnchanged -Before $before -After (Get-RepositorySnapshot -Repository $sourceRepository)
    }
    catch {
        if ($null -eq $testError) { $testError = $_ }
    }
}

if ($null -ne $testError) {
    throw $testError
}

Write-Output '{"contract":"integration-ref-journal","status":"pass"}'
