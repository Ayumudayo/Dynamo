$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$AuditBaseline = '03ec755eb109975ecc8911f26cc75ee482f32a7a'
$PlanNames = @(
    '2026-07-12-security-performance-dashboard-remediation-program.md'
    '2026-07-12-security-remediation.md'
    '2026-07-12-performance-remediation.md'
    '2026-07-12-dashboard-ux-remediation.md'
    '2026-07-13-wave0-bootstrap.md'
)
$BootstrapPaths = @(
    'scripts/remediation/control-schema-v2.json'
    'scripts/remediation/modules/canonical-json.ps1'
    'scripts/remediation/modules/path-security.ps1'
    'scripts/remediation/publish-plan-set.ps1'
    'scripts/remediation/update-integration-ref.ps1'
    'tests/scripts/plan-set-publisher-contract.ps1'
    'tests/scripts/integration-ref-journal-contract.ps1'
)
$script:PublisherChildren = [System.Collections.Generic.List[object]]::new()

function Assert-Contract {
    param(
        [Parameter(Mandatory)]
        [bool]$Condition,

        [Parameter(Mandatory)]
        [string]$Message
    )

    if (-not $Condition) {
        throw "plan-set-publisher-contract: $Message"
    }
}

function Invoke-Git {
    param(
        [Parameter(Mandatory)]
        [string]$WorkingDirectory,

        [Parameter(Mandatory)]
        [string[]]$Arguments,

        [switch]$AllowFailure
    )

    $output = @(& git --no-optional-locks -C $WorkingDirectory @Arguments 2>&1)
    $exitCode = $LASTEXITCODE
    if (-not $AllowFailure -and $exitCode -ne 0) {
        throw "git -C '$WorkingDirectory' $($Arguments -join ' ') failed ($exitCode): $($output -join [Environment]::NewLine)"
    }

    [pscustomobject]@{
        ExitCode = $exitCode
        Output = $output
    }
}

function Assert-PublisherBatchBlobDuplex {
    param(
        [Parameter(Mandatory)][string]$PublisherPath,
        [Parameter(Mandatory)][string]$CaseRoot
    )
    $repository = Join-Path $CaseRoot 'batch-repository'
    $bulkRoot = Join-Path $repository 'bulk'
    $null = New-Item -ItemType Directory -Path $bulkRoot -Force
    $null = Invoke-Git -WorkingDirectory $repository -Arguments @('init', '--quiet')
    $null = Invoke-Git -WorkingDirectory $repository -Arguments @('config', 'core.autocrlf', 'false')
    [int]$blobCount = 1800
    [int]$blobBytes = 32768
    [byte[]]$body = [byte[]]::new($blobBytes)
    [Array]::Fill[byte]($body, 0x61)
    for ($index = 0; $index -lt $blobCount; $index++) {
        [byte[]]$prefix = [BitConverter]::GetBytes([int]$index)
        [Array]::Copy($prefix, 0, $body, 0, $prefix.Length)
        [System.IO.File]::WriteAllBytes((Join-Path $bulkRoot ('blob-{0:d4}.bin' -f $index)), $body)
    }
    $null = Invoke-Git -WorkingDirectory $repository -Arguments @('add', '--', 'bulk')
    $entryLines = [System.Collections.Generic.List[string]]::new()
    foreach ($line in @((Invoke-Git -WorkingDirectory $repository -Arguments @('ls-files', '--stage', '--', 'bulk')).Output)) {
        $match = [regex]::Match([string]$line, '^100644 (?<oid>[0-9a-f]{40}) 0\t(?<path>bulk/.+)$')
        Assert-Contract $match.Success 'large/many blob fixture produced a malformed index entry'
        $entryLines.Add("$($match.Groups['oid'].Value)|$($match.Groups['path'].Value)")
    }
    Assert-Contract ($entryLines.Count -eq $blobCount -and @($entryLines | ForEach-Object { $_.Substring(0, 40) } | Sort-Object -Unique -CaseSensitive).Count -eq $blobCount) 'large/many blob fixture OIDs are not unique and complete'
    $entriesPath = Join-Path $CaseRoot 'batch-entries.txt'
    [System.IO.File]::WriteAllLines($entriesPath, $entryLines, [System.Text.UTF8Encoding]::new($false))

    $tokens = $null
    $parseErrors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($PublisherPath, [ref]$tokens, [ref]$parseErrors)
    Assert-Contract ($parseErrors.Count -eq 0) 'publisher does not parse for the batch-blob probe'
    $wanted = @('Read-AsciiLine', 'Read-ExactBytes', 'Get-GitBatchBlobs')
    $definitions = @($ast.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -in $wanted
    }, $true) | Sort-Object { $_.Extent.StartOffset })
    Assert-Contract ($definitions.Count -eq $wanted.Count -and @($definitions.Name | Sort-Object -Unique -CaseSensitive).Count -eq $wanted.Count) 'could not extract the exact publisher batch functions'
    $harnessPath = Join-Path $CaseRoot 'batch-harness.ps1'
    $harnessHeader = @'
param(
    [Parameter(Mandatory)][string]$Repository,
    [Parameter(Mandatory)][string]$EntriesPath,
    [Parameter(Mandatory)][int]$ExpectedCount
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
'@
    $harnessBody = @'
$script:RepositoryRoot = [System.IO.Path]::GetFullPath($Repository)
$entries = @([System.IO.File]::ReadAllLines($EntriesPath) | ForEach-Object {
    $parts = $_.Split('|', 2)
    if ($parts.Count -ne 2 -or $parts[0] -notmatch '^[0-9a-f]{40}$') { throw 'malformed batch probe entry' }
    [pscustomobject]@{ Oid = $parts[0]; Path = $parts[1] }
})
if ($entries.Count -ne $ExpectedCount) { throw 'batch probe entry count mismatch' }
$blobs = Get-GitBatchBlobs -Entries $entries
if ($blobs.Count -ne $ExpectedCount) { throw 'batch probe blob count mismatch' }
foreach ($entry in $entries) {
    $expected = [System.IO.File]::ReadAllBytes((Join-Path $script:RepositoryRoot $entry.Path))
    if (-not $blobs.ContainsKey($entry.Oid) -or -not [System.Linq.Enumerable]::SequenceEqual[byte]($expected, $blobs[$entry.Oid])) {
        throw "batch probe blob mismatch: $($entry.Oid)"
    }
}
[Console]::Out.WriteLine('BATCH_DUPLEX_PASS')
'@
    $harness = $harnessHeader + "`n" + (($definitions | ForEach-Object { $_.Extent.Text }) -join "`n`n") + "`n" + $harnessBody
    [System.IO.File]::WriteAllText($harnessPath, $harness, [System.Text.UTF8Encoding]::new($false))

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = 'pwsh'
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    foreach ($argument in @('-NoProfile', '-File', $harnessPath, '-Repository', $repository, '-EntriesPath', $entriesPath, '-ExpectedCount', [string]$blobCount)) { $psi.ArgumentList.Add($argument) }
    $process = [System.Diagnostics.Process]::Start($psi)
    try {
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(60000)) {
            $process.Kill($true)
            $process.WaitForExit()
            throw 'publisher batch-blob probe deadlocked on many large responses'
        }
        $stdout = $stdoutTask.GetAwaiter().GetResult().Trim()
        $stderr = $stderrTask.GetAwaiter().GetResult().Trim()
        Assert-Contract ($process.ExitCode -eq 0 -and $stdout -ceq 'BATCH_DUPLEX_PASS') "publisher batch-blob probe failed: $stderr"
    }
    finally {
        if (-not $process.HasExited) { $process.Kill($true); $process.WaitForExit() }
        $process.Dispose()
    }
}

function Get-RepositorySnapshot {
    param([Parameter(Mandatory)][string]$Repository)

    $head = (Invoke-Git -WorkingDirectory $Repository -Arguments @('rev-parse', 'HEAD')).Output[-1].Trim()
    $status = @((Invoke-Git -WorkingDirectory $Repository -Arguments @('status', '--porcelain=v1', '--untracked-files=all')).Output)
    $common = (Invoke-Git -WorkingDirectory $Repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()
    $controlRoot = Join-Path $common 'dynamo-remediation'
    $refs = @((Invoke-Git -WorkingDirectory $Repository -Arguments @('for-each-ref', '--format=%(refname)%00%(objectname)', 'refs/dynamo-remediation/')).Output)
    $refLogs = Get-TreeFingerprint -Root (Join-Path $common 'logs/refs/dynamo-remediation')
    $control = if (Test-Path -LiteralPath $controlRoot) {
        @(
            Get-ChildItem -LiteralPath $controlRoot -Force -Recurse |
                Sort-Object FullName |
                ForEach-Object {
                    if ($_.PSIsContainer) {
                        "D|$($_.FullName)"
                    }
                    else {
                        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
                        "F|$($_.FullName)|$($_.Length)|$hash"
                    }
                }
        )
    }
    else {
        @('<absent>')
    }

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
    param(
        [Parameter(Mandatory)]$Before,
        [Parameter(Mandatory)]$After
    )

    Assert-Contract ($Before.Head -ceq $After.Head) 'the real repository HEAD changed'
    Assert-Contract ((@($Before.Status) -join "`n") -ceq (@($After.Status) -join "`n")) 'the real repository status changed'
    Assert-Contract ($Before.CommonDirectory -ceq $After.CommonDirectory) 'the real Git common directory changed'
    Assert-Contract ((@($Before.Control) -join "`n") -ceq (@($After.Control) -join "`n")) 'the real Git common-dir control state changed'
    Assert-Contract ((@($Before.Refs) -join "`n") -ceq (@($After.Refs) -join "`n")) 'the real remediation refs changed'
    Assert-Contract ((@($Before.RefLogs) -join "`n") -ceq (@($After.RefLogs) -join "`n")) 'the real remediation reflogs changed'
}

function Copy-FileExact {
    param(
        [Parameter(Mandatory)][string]$Source,
        [Parameter(Mandatory)][string]$Destination
    )

    $parent = Split-Path -Parent $Destination
    $null = New-Item -ItemType Directory -Path $parent -Force
    [System.IO.File]::WriteAllBytes($Destination, [System.IO.File]::ReadAllBytes($Source))
}

function Add-ContractAclDrift {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][ValidateSet('File','Directory')][string]$Kind
    )
    $item = Get-Item -LiteralPath $Path -Force
    $acl = [System.IO.FileSystemAclExtensions]::GetAccessControl($item)
    $sections = [System.Security.AccessControl.AccessControlSections]'Access, Owner, Group'
    $sddl = $acl.GetSecurityDescriptorSddlForm($sections)
    $guest = [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-546')
    $rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
        $guest,
        [System.Security.AccessControl.FileSystemRights]::Read,
        [System.Security.AccessControl.InheritanceFlags]::None,
        [System.Security.AccessControl.PropagationFlags]::None,
        [System.Security.AccessControl.AccessControlType]::Allow
    )
    $null = $acl.AddAccessRule($rule)
    [System.IO.FileSystemAclExtensions]::SetAccessControl($item, $acl)
    [pscustomobject]@{ Path = $Path; Kind = $Kind; Sddl = $sddl; Sections = $sections }
}

function Restore-ContractAcl {
    param([Parameter(Mandatory)]$State)
    $security = if ($State.Kind -ceq 'Directory') { [System.Security.AccessControl.DirectorySecurity]::new() } else { [System.Security.AccessControl.FileSecurity]::new() }
    $security.SetSecurityDescriptorSddlForm($State.Sddl, $State.Sections)
    $item = Get-Item -LiteralPath $State.Path -Force
    [System.IO.FileSystemAclExtensions]::SetAccessControl($item, $security)
}

function Set-ContractDirectoryAclFromSddl {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Sddl
    )
    $sections = [System.Security.AccessControl.AccessControlSections]'Access, Owner, Group'
    $security = [System.Security.AccessControl.DirectorySecurity]::new()
    $security.SetSecurityDescriptorSddlForm($Sddl, $sections)
    [System.IO.FileSystemAclExtensions]::SetAccessControl((Get-Item -LiteralPath $Path -Force), $security)
}

function Assert-PathUnderRoot {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string]$Message
    )

    $fullPath = [System.IO.Path]::GetFullPath($Path)
    $fullRoot = [System.IO.Path]::GetFullPath($Root).TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    Assert-Contract ($fullPath.StartsWith($fullRoot, [System.StringComparison]::OrdinalIgnoreCase)) $Message
}

function Get-DomainHash {
    param(
        [Parameter(Mandatory)][string]$Domain,
        [Parameter(Mandatory)][string]$CanonicalJson
    )

    $prefix = [System.Text.UTF8Encoding]::new($false).GetBytes("$Domain`0")
    $body = [System.Text.UTF8Encoding]::new($false).GetBytes($CanonicalJson)
    $bytes = [byte[]]::new($prefix.Length + $body.Length)
    [System.Buffer]::BlockCopy($prefix, 0, $bytes, 0, $prefix.Length)
    [System.Buffer]::BlockCopy($body, 0, $bytes, $prefix.Length, $body.Length)
    [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
}

function Get-TreeFingerprint {
    param([Parameter(Mandatory)][string]$Root)

    if (-not (Test-Path -LiteralPath $Root)) { return @('<absent>') }
    @(
        Get-ChildItem -LiteralPath $Root -Force -Recurse |
            Sort-Object FullName |
            ForEach-Object {
                $relative = [System.IO.Path]::GetRelativePath($Root, $_.FullName).Replace('\', '/')
                if ($_.PSIsContainer) { "D|$relative" }
                else {
                    $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
                    "F|$relative|$($_.Length)|$hash"
                }
            }
    )
}

function Get-OwnedRootIdentity {
    param([Parameter(Mandatory)][string]$Root)

    $item = Get-Item -LiteralPath $Root -Force
    Assert-Contract (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) 'suite root is a reparse point'
    $full = [System.IO.Path]::GetFullPath($item.FullName)
    if ($IsWindows) {
        $native = @(& fsutil file queryfileid $full 2>&1)
        Assert-Contract ($LASTEXITCODE -eq 0 -and $native.Count -eq 1) 'could not read suite-root native file identity'
        return "$full|$($native[0].Trim())"
    }
    $native = @(& stat -c '%d:%i' -- $full 2>&1)
    Assert-Contract ($LASTEXITCODE -eq 0 -and $native.Count -eq 1) 'could not read suite-root native inode identity'
    "$full|$($native[0].Trim())"
}

function Get-ContractFileIdentity {
    param([Parameter(Mandatory)][string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    if ($IsWindows) {
        $nativePath = if ($full.StartsWith('\\', [System.StringComparison]::Ordinal)) { '\\?\UNC\' + $full.Substring(2) } else { '\\?\' + $full }
        $native = @(& fsutil file queryfileid $nativePath 2>&1)
        Assert-Contract ($LASTEXITCODE -eq 0 -and $native.Count -eq 1) "could not read native file identity: $full"
        return $native[0].Trim()
    }
    $native = @(& stat -c '%d:%i' -- $full 2>&1)
    Assert-Contract ($LASTEXITCODE -eq 0 -and $native.Count -eq 1) "could not read native file identity: $full"
    $native[0].Trim()
}

function Get-ContractPathState {
    param(
        [Parameter(Mandatory)][string]$Path,
        [string[]]$ExcludeRelativePrefix = @()
    )
    $full = [System.IO.Path]::GetFullPath($Path)
    if (-not (Test-Path -LiteralPath $full)) { return @('<absent>') }
    $pending = [System.Collections.Generic.Queue[System.IO.FileSystemInfo]]::new()
    $pending.Enqueue((Get-Item -LiteralPath $full -Force))
    $rows = [System.Collections.Generic.List[string]]::new()
    while ($pending.Count -gt 0) {
        $item = $pending.Dequeue()
        $relative = if ($item.FullName -ceq $full) { '.' } else { [System.IO.Path]::GetRelativePath($full, $item.FullName).Replace('\', '/') }
        if (@($ExcludeRelativePrefix | Where-Object { $relative -ceq $_ -or $relative.StartsWith("$_/", [System.StringComparison]::Ordinal) }).Count -gt 0) { continue }
        $acl = [System.IO.FileSystemAclExtensions]::GetAccessControl($item)
        $sections = [System.Security.AccessControl.AccessControlSections]'Access, Owner, Group'
        $sddl = $acl.GetSecurityDescriptorSddlForm($sections)
        $identity = Get-ContractFileIdentity -Path $item.FullName
        if ($item.PSIsContainer) {
            $rows.Add("D|$relative|$([int64]$item.Attributes)|$identity|$sddl")
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) {
                foreach ($child in @(Get-ChildItem -LiteralPath $item.FullName -Force | Sort-Object Name -CaseSensitive)) { $pending.Enqueue($child) }
            }
        }
        else {
            $hash = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            $rows.Add("F|$relative|$([int64]$item.Attributes)|$identity|$sddl|$($item.Length)|$hash")
        }
    }
    @($rows | Sort-Object -CaseSensitive)
}

function Get-PublisherMutationSnapshot {
    param(
        [Parameter(Mandatory)][string]$Repository,
        [Parameter(Mandatory)][string]$EvidenceRoot
    )
    $common = (Invoke-Git -WorkingDirectory $Repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()
    $gitDirectory = (Invoke-Git -WorkingDirectory $Repository -Arguments @('rev-parse', '--path-format=absolute', '--absolute-git-dir')).Output[-1].Trim()
    $evidencePath = if ([System.IO.Path]::IsPathFullyQualified($EvidenceRoot)) { $EvidenceRoot } else { Join-Path $Repository $EvidenceRoot }
    $repositoryState = Get-RepositorySnapshot -Repository $Repository
    $rows = [System.Collections.Generic.List[string]]::new()
    $rows.Add("HEAD|$([string]$repositoryState.Head)")
    foreach ($value in @($repositoryState.Status)) { $rows.Add("STATUS|$([string]$value)") }
    foreach ($value in @((Invoke-Git -WorkingDirectory $Repository -Arguments @('for-each-ref', '--format=%(refname)%00%(objectname)')).Output)) { $rows.Add("ALLREF|$([string]$value)") }
    foreach ($value in @((Invoke-Git -WorkingDirectory $Repository -Arguments @('ls-files', '--stage', '--')).Output)) { $rows.Add("INDEXOID|$([string]$value)") }
    foreach ($value in @((Invoke-Git -WorkingDirectory $Repository -Arguments @('ls-files', '-v', '--')).Output)) { $rows.Add("INDEXFLAG|$([string]$value)") }
    foreach ($relative in @((Invoke-Git -WorkingDirectory $Repository -Arguments @('-c', 'core.quotePath=false', 'ls-files', '--')).Output)) {
        $worktreePath = Join-Path $Repository $relative
        if (Test-Path -LiteralPath $worktreePath -PathType Leaf) {
            $item = Get-Item -LiteralPath $worktreePath -Force
            $hash = (Get-FileHash -LiteralPath $worktreePath -Algorithm SHA256).Hash.ToLowerInvariant()
            $rows.Add("WORKTREE|$relative|$([int64]$item.Attributes)|$($item.Length)|$hash")
        }
        else { $rows.Add("WORKTREE|$relative|<missing-or-nonleaf>") }
    }
    foreach ($state in @(
        @{ Label = 'GIT_CONFIG'; Path = (Join-Path $common 'config') },
        @{ Label = 'GIT_INFO'; Path = (Join-Path $common 'info') },
        @{ Label = 'GIT_PACKED_REFS'; Path = (Join-Path $common 'packed-refs') },
        @{ Label = 'COMMON_REFS'; Path = (Join-Path $common 'refs') },
        @{ Label = 'COMMON_REFLOGS'; Path = (Join-Path $common 'logs/refs') },
        @{ Label = 'GIT_INDEX'; Path = (Join-Path $gitDirectory 'index') },
        @{ Label = 'GIT_HEAD'; Path = (Join-Path $gitDirectory 'HEAD') },
        @{ Label = 'WORKTREE_REFS'; Path = (Join-Path $gitDirectory 'refs') },
        @{ Label = 'WORKTREE_REFLOGS'; Path = (Join-Path $gitDirectory 'logs/refs') }
    )) {
        foreach ($value in @(Get-ContractPathState -Path $state.Path)) { $rows.Add("$($state.Label)|$([string]$value)") }
    }
    foreach ($value in @(Get-ContractPathState -Path (Join-Path $common 'dynamo-remediation'))) { $rows.Add("CONTROL|$([string]$value)") }
    foreach ($value in @(Get-ContractPathState -Path $evidencePath -ExcludeRelativePrefix @('.publisher-test-barriers'))) { $rows.Add("EVIDENCE|$([string]$value)") }
    [string]::Join("`n", $rows)
}

function Invoke-PublisherRejectedWithoutMutation {
    param(
        [Parameter(Mandatory)][string]$Repository,
        [Parameter(Mandatory)][string]$EvidenceRoot,
        [Parameter(Mandatory)][string]$Case,
        [hashtable]$Environment = @{}
    )
    $before = Get-PublisherMutationSnapshot -Repository $Repository -EvidenceRoot $EvidenceRoot
    $invocation = Complete-Publisher -Running (Start-Publisher -Repository $Repository -EvidenceRoot $EvidenceRoot -Environment $Environment)
    Assert-FailedWithoutOutput -Invocation $invocation -Case $Case
    $after = Get-PublisherMutationSnapshot -Repository $Repository -EvidenceRoot $EvidenceRoot
    Assert-Contract ($before -ceq $after) "$Case mutated repository/control/evidence state after rejection"
    $invocation
}

function Complete-RunningPublisherRejectedWithoutMutation {
    param(
        [Parameter(Mandatory)]$Running,
        [Parameter(Mandatory)][string]$Repository,
        [Parameter(Mandatory)][string]$EvidenceRoot,
        [Parameter(Mandatory)][string]$ReleasePath,
        [Parameter(Mandatory)][string]$Case
    )
    $before = Get-PublisherMutationSnapshot -Repository $Repository -EvidenceRoot $EvidenceRoot
    [System.IO.File]::WriteAllText($ReleasePath, 'release', [System.Text.UTF8Encoding]::new($false))
    $invocation = Complete-Publisher -Running $Running
    Assert-FailedWithoutOutput -Invocation $invocation -Case $Case
    $after = Get-PublisherMutationSnapshot -Repository $Repository -EvidenceRoot $EvidenceRoot
    Assert-Contract ($before -ceq $after) "$Case mutated state outside the excluded test barrier after rejection"
    $invocation
}

function Assert-ContractAclTreeSemantics {
    param([Parameter(Mandatory)][string]$Root)
    Assert-ContractAclSemantics -Path $Root -Kind Directory
    foreach ($item in @(Get-ChildItem -LiteralPath $Root -Force -Recurse | Sort-Object FullName -CaseSensitive)) {
        Assert-ContractAclSemantics -Path $item.FullName -Kind $(if ($item.PSIsContainer) { 'Directory' } else { 'File' })
    }
}

function Assert-ContractAclSemantics {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][ValidateSet('File','Directory')][string]$Kind
    )
    $item = Get-Item -LiteralPath $Path -Force
    Assert-Contract (($Kind -ceq 'Directory') -eq [bool]$item.PSIsContainer) "ACL oracle type mismatch: $Path"
    $acl = [System.IO.FileSystemAclExtensions]::GetAccessControl($item)
    $current = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $system = 'S-1-5-18'
    Assert-Contract ($acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ceq $current) "ACL oracle owner mismatch: $Path"
    if ($Kind -ceq 'Directory') { Assert-Contract ([bool]$acl.AreAccessRulesProtected) "ACL oracle found inherited directory ACL: $Path" }
    $rules = @($acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
    Assert-Contract ($rules.Count -eq 2) "ACL oracle expected exactly two ACEs: $Path"
    $tuples = @($rules | ForEach-Object {
        "$($_.IdentityReference.Value)|$($_.AccessControlType)|$([int64]$_.FileSystemRights)|$([int64]$_.InheritanceFlags)|$([int64]$_.PropagationFlags)|$([bool]$_.IsInherited)"
    } | Sort-Object -CaseSensitive)
    $full = [int64][System.Security.AccessControl.FileSystemRights]::FullControl
    $inherit = [int64][System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
    $expected = if ($Kind -ceq 'Directory') {
        @("$current|Allow|$full|$inherit|0|False", "$system|Allow|$full|$inherit|0|False") | Sort-Object -CaseSensitive
    }
    else {
        @("$current|Allow|$full|0|0|True", "$system|Allow|$full|0|0|True") | Sort-Object -CaseSensitive
    }
    Assert-Contract (($tuples -join "`n") -ceq ($expected -join "`n")) "ACL oracle semantic ACE mismatch: $Path"
}

function Assert-SelfOmittingHash {
    param(
        [Parameter(Mandatory)][string]$Raw,
        [Parameter(Mandatory)][string]$Property,
        [Parameter(Mandatory)][string]$Domain,
        [Parameter(Mandatory)][string]$Expected,
        [Parameter(Mandatory)][string]$Case
    )

    Assert-Contract ($Raw.EndsWith("`n", [System.StringComparison]::Ordinal) -and -not $Raw.Substring(0, $Raw.Length - 1).Contains("`n") -and -not $Raw.Contains("`r")) "$Case must be compact UTF-8 with exactly one terminal LF"
    $canonical = $Raw.Substring(0, $Raw.Length - 1)
    $pattern = [string]::Concat(',"', [regex]::Escape($Property), '":"(?<hash>[0-9a-f]{64})"}$')
    $match = [regex]::Match($canonical, $pattern)
    Assert-Contract ($match.Success) "$Case self-hash is not the final canonical property"
    Assert-Contract ($match.Groups['hash'].Value -ceq $Expected) "$Case self-hash value mismatch"
    $preimage = $canonical.Substring(0, $match.Index) + "}`n"
    Assert-Contract ((Get-DomainHash -Domain $Domain -CanonicalJson $preimage) -ceq $Expected) "$Case domain-separated self-hash mismatch"
}

function Set-SelfHashedStringValue {
    param(
        [Parameter(Mandatory)][string]$Raw,
        [Parameter(Mandatory)][string]$Property,
        [Parameter(Mandatory)][string]$Value,
        [Parameter(Mandatory)][string]$HashProperty,
        [Parameter(Mandatory)][string]$Domain
    )

    $propertyPattern = [string]::Concat('"', [regex]::Escape($Property), '":"[^"\\]*"')
    $propertyMatches = [regex]::Matches($Raw, $propertyPattern)
    Assert-Contract ($propertyMatches.Count -eq 1) "could not identify exactly one $Property string property"
    $replacement = '"' + $Property + '":"' + $Value + '"'
    $updated = $Raw.Substring(0, $propertyMatches[0].Index) + $replacement + $Raw.Substring($propertyMatches[0].Index + $propertyMatches[0].Length)
    $tailPattern = [string]::Concat(',"', [regex]::Escape($HashProperty), '":"(?<hash>[0-9a-f]{64})"}\n$')
    $tail = [regex]::Match($updated, $tailPattern)
    Assert-Contract $tail.Success "could not identify terminal $HashProperty property"
    $preimage = $updated.Substring(0, $tail.Index) + "}`n"
    $hash = Get-DomainHash -Domain $Domain -CanonicalJson $preimage
    $hashGroup = $tail.Groups['hash']
    $updated.Substring(0, $hashGroup.Index) + $hash + $updated.Substring($hashGroup.Index + $hashGroup.Length)
}

function Rehash-SelfHashedJson {
    param(
        [Parameter(Mandatory)][string]$Raw,
        [Parameter(Mandatory)][string]$HashProperty,
        [Parameter(Mandatory)][string]$Domain
    )
    $tailPattern = [string]::Concat(',"', [regex]::Escape($HashProperty), '":"(?<hash>[0-9a-f]{64})"}\n$')
    $tail = [regex]::Match($Raw, $tailPattern)
    Assert-Contract $tail.Success "could not identify terminal $HashProperty for coherent rehash"
    $preimage = $Raw.Substring(0, $tail.Index) + "}`n"
    $hash = Get-DomainHash -Domain $Domain -CanonicalJson $preimage
    $group = $tail.Groups['hash']
    $Raw.Substring(0, $group.Index) + $hash + $Raw.Substring($group.Index + $group.Length)
}

function Convert-JsonIntegerTokenToString {
    param(
        [Parameter(Mandatory)][string]$Raw,
        [Parameter(Mandatory)][string]$Property,
        [int]$Occurrence = 0
    )
    $pattern = [string]::Concat('"', [regex]::Escape($Property), '":(?<number>-?(?:0|[1-9][0-9]*))')
    $matches = [regex]::Matches($Raw, $pattern)
    Assert-Contract ($Occurrence -ge 0 -and $Occurrence -lt $matches.Count) "could not identify integer token occurrence $Occurrence for $Property"
    $match = $matches[$Occurrence]
    $replacement = '"' + $Property + '":"' + $match.Groups['number'].Value + '"'
    $Raw.Substring(0, $match.Index) + $replacement + $Raw.Substring($match.Index + $match.Length)
}

function Replace-ExactRawToken {
    param(
        [Parameter(Mandatory)][string]$Raw,
        [Parameter(Mandatory)][string]$Old,
        [Parameter(Mandatory)][string]$New,
        [int]$ExpectedCount = 1
    )
    $count = [regex]::Matches($Raw, [regex]::Escape($Old)).Count
    Assert-Contract ($count -eq $ExpectedCount) "coherent rewrite expected $ExpectedCount occurrences of $Old but found $count"
    $Raw.Replace($Old, $New, [System.StringComparison]::Ordinal)
}

function Get-RawUtf8Sha256([string] $Raw) {
    [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData([System.Text.UTF8Encoding]::new($false).GetBytes($Raw))).ToLowerInvariant()
}

function Get-TerminalSelfHash([string] $Raw, [string] $HashProperty) {
    $match = [regex]::Match($Raw, [string]::Concat(',"', [regex]::Escape($HashProperty), '":"(?<hash>[0-9a-f]{64})"}\n$'))
    Assert-Contract $match.Success "could not read terminal $HashProperty"
    $match.Groups['hash'].Value
}

function Assert-CoherentNumericStringRejections {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][string]$ManifestPath,
        [Parameter(Mandatory)][string]$ManifestText,
        [Parameter(Mandatory)][string]$BindingPath,
        [Parameter(Mandatory)][string]$BindingText,
        [Parameter(Mandatory)][string]$PreparedPath,
        [Parameter(Mandatory)][string]$PreparedText,
        [Parameter(Mandatory)][string]$CommittedPath,
        [Parameter(Mandatory)][string]$CommittedText,
        [Parameter(Mandatory)][string]$LeasePath,
        [Parameter(Mandatory)][string]$LeaseText
    )
    $utf8 = [System.Text.UTF8Encoding]::new($false)
    $preparedObject = $PreparedText | ConvertFrom-Json -ErrorAction Stop
    $bindingObject = $BindingText | ConvertFrom-Json -ErrorAction Stop
    $leaseObject = $LeaseText | ConvertFrom-Json -ErrorAction Stop
    $oldManifestHash = Get-RawUtf8Sha256 $ManifestText
    $oldManifestBytes = $utf8.GetByteCount($ManifestText)
    $oldPreparedHash = [string]$preparedObject.row_sha256
    $oldBindingHash = [string]$bindingObject.binding_sha256
    $oldLeaseHash = [string]$leaseObject.lease_sha256
    $manifestObject = $ManifestText | ConvertFrom-Json -ErrorAction Stop
    $targets = [System.Collections.Generic.List[object]]::new()
    $targets.Add([pscustomobject]@{ Name = 'manifest-schema-version'; Document = 'manifest'; Property = 'schema_version'; Occurrence = 0 })
    for ($index = 0; $index -lt @($manifestObject.payloads).Count; $index++) {
        $targets.Add([pscustomobject]@{ Name = "manifest-payload-bytes-$index"; Document = 'manifest'; Property = 'bytes'; Occurrence = $index })
    }
    for ($index = 0; $index -lt @($manifestObject.controls).Count; $index++) {
        $targets.Add([pscustomobject]@{ Name = "manifest-control-bytes-$index"; Document = 'manifest'; Property = 'bytes'; Occurrence = @($manifestObject.payloads).Count + $index })
    }
    for ($index = 0; $index -lt @($manifestObject.gitignore_evidence).Count; $index++) {
        $targets.Add([pscustomobject]@{ Name = "manifest-rule-line-$index"; Document = 'manifest'; Property = 'rule_line'; Occurrence = $index })
    }
    foreach ($document in @('prepared','committed')) {
        foreach ($property in @('schema_version','seq','generation','manifest_bytes')) {
            $targets.Add([pscustomobject]@{ Name = "$document-$($property.Replace('_','-'))"; Document = $document; Property = $property; Occurrence = 0 })
        }
    }
    foreach ($property in @('schema_version','manifest_bytes')) {
        $targets.Add([pscustomobject]@{ Name = "binding-$($property.Replace('_','-'))"; Document = 'binding'; Property = $property; Occurrence = 0 })
    }
    foreach ($property in @('schema_version','generation','pid')) {
        $targets.Add([pscustomobject]@{ Name = "lease-$($property.Replace('_','-'))"; Document = 'lease'; Property = $property; Occurrence = 0 })
    }
    $expectedTargetCount = 14 + @($manifestObject.payloads).Count + @($manifestObject.controls).Count + @($manifestObject.gitignore_evidence).Count
    Assert-Contract ($targets.Count -eq $expectedTargetCount -and @($targets.Name | Sort-Object -Unique -CaseSensitive).Count -eq $targets.Count) 'persisted numeric-field target matrix is incomplete or ambiguous'

    foreach ($target in $targets) {
        $manifest = $ManifestText
        $lease = $LeaseText
        $prepared = $PreparedText
        $binding = $BindingText
        $committed = $CommittedText
        switch ($target.Document) {
            'manifest' { $manifest = Convert-JsonIntegerTokenToString -Raw $manifest -Property $target.Property -Occurrence $target.Occurrence }
            'prepared' { $prepared = Convert-JsonIntegerTokenToString -Raw $prepared -Property $target.Property -Occurrence $target.Occurrence }
            'committed' { $committed = Convert-JsonIntegerTokenToString -Raw $committed -Property $target.Property -Occurrence $target.Occurrence }
            'binding' { $binding = Convert-JsonIntegerTokenToString -Raw $binding -Property $target.Property -Occurrence $target.Occurrence }
            'lease' { $lease = Convert-JsonIntegerTokenToString -Raw $lease -Property $target.Property -Occurrence $target.Occurrence }
        }

        $lease = Rehash-SelfHashedJson -Raw $lease -HashProperty 'lease_sha256' -Domain 'dynamo-publication-lease-v1'
        $newLeaseHash = Get-TerminalSelfHash -Raw $lease -HashProperty 'lease_sha256'
        $newManifestHash = Get-RawUtf8Sha256 $manifest
        $newManifestBytes = $utf8.GetByteCount($manifest)

        $prepared = Replace-ExactRawToken -Raw $prepared -Old ('"manifest_sha256":"' + $oldManifestHash + '"') -New ('"manifest_sha256":"' + $newManifestHash + '"')
        if (-not ($target.Document -ceq 'prepared' -and $target.Property -ceq 'manifest_bytes')) {
            $prepared = Replace-ExactRawToken -Raw $prepared -Old ('"manifest_bytes":' + $oldManifestBytes) -New ('"manifest_bytes":' + $newManifestBytes)
        }
        $prepared = Replace-ExactRawToken -Raw $prepared -Old ('"lease_sha256":"' + $oldLeaseHash + '"') -New ('"lease_sha256":"' + $newLeaseHash + '"')
        $prepared = Rehash-SelfHashedJson -Raw $prepared -HashProperty 'row_sha256' -Domain 'dynamo-publication-row-v1'
        $newPreparedHash = Get-TerminalSelfHash -Raw $prepared -HashProperty 'row_sha256'

        $binding = Replace-ExactRawToken -Raw $binding -Old ('"manifest_sha256":"' + $oldManifestHash + '"') -New ('"manifest_sha256":"' + $newManifestHash + '"')
        if (-not ($target.Document -ceq 'binding' -and $target.Property -ceq 'manifest_bytes')) {
            $binding = Replace-ExactRawToken -Raw $binding -Old ('"manifest_bytes":' + $oldManifestBytes) -New ('"manifest_bytes":' + $newManifestBytes)
        }
        $binding = Replace-ExactRawToken -Raw $binding -Old ('"bundle_prepared_row_sha256":"' + $oldPreparedHash + '"') -New ('"bundle_prepared_row_sha256":"' + $newPreparedHash + '"')
        $binding = Rehash-SelfHashedJson -Raw $binding -HashProperty 'binding_sha256' -Domain 'dynamo-plan-set-binding-v1'
        $newBindingHash = Get-TerminalSelfHash -Raw $binding -HashProperty 'binding_sha256'

        $committed = Replace-ExactRawToken -Raw $committed -Old ('"manifest_sha256":"' + $oldManifestHash + '"') -New ('"manifest_sha256":"' + $newManifestHash + '"')
        if (-not ($target.Document -ceq 'committed' -and $target.Property -ceq 'manifest_bytes')) {
            $committed = Replace-ExactRawToken -Raw $committed -Old ('"manifest_bytes":' + $oldManifestBytes) -New ('"manifest_bytes":' + $newManifestBytes)
        }
        $committed = Replace-ExactRawToken -Raw $committed -Old ('"lease_sha256":"' + $oldLeaseHash + '"') -New ('"lease_sha256":"' + $newLeaseHash + '"')
        $committed = Replace-ExactRawToken -Raw $committed -Old $oldPreparedHash -New $newPreparedHash -ExpectedCount 2
        $committed = Replace-ExactRawToken -Raw $committed -Old ('"binding_sha256":"' + $oldBindingHash + '"') -New ('"binding_sha256":"' + $newBindingHash + '"')
        $committed = Rehash-SelfHashedJson -Raw $committed -HashProperty 'row_sha256' -Domain 'dynamo-publication-row-v1'

        $newLeasePath = if ($newLeaseHash -ceq $oldLeaseHash) { $LeasePath } else {
            Join-Path (Split-Path -Parent $LeasePath) ("closed-publication.{0}.g{1:d10}.{2}.lock" -f $leaseObject.attempt_id, [int64]$leaseObject.generation, $newLeaseHash)
        }
        try {
            [System.IO.File]::WriteAllText($ManifestPath, $manifest, $utf8)
            [System.IO.File]::WriteAllText($PreparedPath, $prepared, $utf8)
            [System.IO.File]::WriteAllText($BindingPath, $binding, $utf8)
            [System.IO.File]::WriteAllText($CommittedPath, $committed, $utf8)
            if ($newLeasePath -cne $LeasePath) { Move-Item -LiteralPath $LeasePath -Destination $newLeasePath }
            [System.IO.File]::WriteAllText($newLeasePath, $lease, $utf8)
            $rejected = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case "coherent numeric-string graph $($target.Name)"
            Assert-Contract ((@($rejected.Stderr) -join "`n") -match 'must be an actual JSON integer') "coherent numeric-string graph $($target.Name) did not prove token-type rejection"
        }
        finally {
            if ($newLeasePath -cne $LeasePath -and (Test-Path -LiteralPath $newLeasePath)) { Move-Item -LiteralPath $newLeasePath -Destination $LeasePath }
            [System.IO.File]::WriteAllText($ManifestPath, $ManifestText, $utf8)
            [System.IO.File]::WriteAllText($PreparedPath, $PreparedText, $utf8)
            [System.IO.File]::WriteAllText($BindingPath, $BindingText, $utf8)
            [System.IO.File]::WriteAllText($CommittedPath, $CommittedText, $utf8)
            [System.IO.File]::WriteAllText($LeasePath, $LeaseText, $utf8)
        }
    }
}

function Invoke-Publisher {
    param(
        [Parameter(Mandatory)][string]$Repository,
        [Parameter(Mandatory)][string]$EvidenceRoot
    )

    Complete-Publisher -Running (Start-Publisher -Repository $Repository -EvidenceRoot $EvidenceRoot)
}

function Start-Publisher {
    param(
        [Parameter(Mandatory)][string]$Repository,
        [Parameter(Mandatory)][string]$EvidenceRoot,
        [hashtable]$Environment = @{}
    )

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = (Get-Command pwsh -ErrorAction Stop).Source
    $psi.WorkingDirectory = $Repository
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    foreach ($name in @('GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES', 'GIT_INDEX_FILE', 'GIT_NAMESPACE', 'GIT_CEILING_DIRECTORIES', 'GIT_DISCOVERY_ACROSS_FILESYSTEM', 'GIT_CONFIG', 'GIT_CONFIG_GLOBAL', 'GIT_CONFIG_SYSTEM', 'GIT_CONFIG_COUNT', 'GIT_REPLACE_REF_BASE', 'GIT_NO_REPLACE_OBJECTS')) {
        $null = $psi.Environment.Remove($name)
    }
    $psi.Environment['DYNAMO_REMEDIATION_EVIDENCE_ROOT'] = $EvidenceRoot
    foreach ($key in $Environment.Keys) { $psi.Environment[$key] = [string]$Environment[$key] }
    foreach ($argument in @('-NoProfile', '-File', (Join-Path $Repository 'scripts/remediation/publish-plan-set.ps1'))) {
        $null = $psi.ArgumentList.Add($argument)
    }
    $process = [System.Diagnostics.Process]::Start($psi)
    $running = [pscustomobject]@{
        Process = $process
        StandardOutput = $process.StandardOutput.ReadToEndAsync()
        StandardError = $process.StandardError.ReadToEndAsync()
    }
    $script:PublisherChildren.Add($running)
    $running
}

function Stop-PublisherChildren {
    foreach ($running in @($script:PublisherChildren)) {
        try {
            if (-not $running.Process.HasExited) {
                $running.Process.Kill($true)
                $running.Process.WaitForExit()
            }
        } catch {}
        try { $null = $running.StandardOutput.GetAwaiter().GetResult() } catch {}
        try { $null = $running.StandardError.GetAwaiter().GetResult() } catch {}
    }
    $live = @($script:PublisherChildren | Where-Object { -not $_.Process.HasExited })
    Assert-Contract ($live.Count -eq 0) 'publisher child cleanup left a live process'
}

function Complete-Publisher {
    param([Parameter(Mandatory)]$Running)

    if (-not $Running.Process.WaitForExit(60000)) {
        $Running.Process.Kill($true)
        $Running.Process.WaitForExit()
        $null = $Running.StandardOutput.GetAwaiter().GetResult()
        $null = $Running.StandardError.GetAwaiter().GetResult()
        throw 'plan-set-publisher-contract: publisher child exceeded 60-second timeout and was killed'
    }
    $stdout = $Running.StandardOutput.GetAwaiter().GetResult()
    $stderr = $Running.StandardError.GetAwaiter().GetResult()
    $stdoutLines = @($stdout -split '\r?\n') | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    $stderrLines = @($stderr -split '\r?\n') | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    [pscustomobject]@{
        ExitCode = $Running.Process.ExitCode
        Stdout = @($stdoutLines)
        Stderr = @($stderrLines)
        Output = @($stdoutLines) + @($stderrLines)
    }
}

function Wait-PublisherBarrier {
    param(
        [Parameter(Mandatory)]$Running,
        [Parameter(Mandatory)][string]$EnteredPath,
        [Parameter(Mandatory)][string]$Case
    )
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not (Test-Path -LiteralPath $EnteredPath -PathType Leaf) -and -not $Running.Process.HasExited -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 25 }
    if (-not (Test-Path -LiteralPath $EnteredPath -PathType Leaf)) {
        $diagnostic = if ($Running.Process.HasExited) { "exit=$($Running.Process.ExitCode)" } else { 'child-still-running' }
        Assert-Contract $false "$Case barrier was not reached: $diagnostic"
    }
}

function Convert-Handoff {
    param([Parameter(Mandatory)]$Invocation)

    Assert-Contract ($Invocation.ExitCode -eq 0) "publisher failed: $($Invocation.Output -join [Environment]::NewLine)"
    $lines = @($Invocation.Stdout | ForEach-Object { "$_" } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    Assert-Contract ($lines.Count -eq 1) 'publisher must emit exactly one nonempty JSON handoff line'
    Assert-Contract (@($Invocation.Stderr).Count -eq 0) 'successful publisher emitted stderr'
    try {
        $handoff = $lines[0] | ConvertFrom-Json -ErrorAction Stop
    }
    catch {
        throw "plan-set-publisher-contract: publisher output is not one JSON handoff: $($_.Exception.Message)"
    }

    $handoff
}

function Assert-FailedWithoutOutput {
    param(
        [Parameter(Mandatory)]$Invocation,
        [Parameter(Mandatory)][string]$Case
    )

    Assert-Contract ($Invocation.ExitCode -ne 0) "$Case unexpectedly succeeded"
    Assert-Contract (@($Invocation.Stdout).Count -eq 0) "$Case emitted stdout before failing"
    $joined = @($Invocation.Output | ForEach-Object { "$_" }) -join "`n"
    Assert-Contract ($joined -notmatch '(?i)(mongodb(?:\+srv)?://|authorization:\s*bearer|client_secret|access_token|refresh_token|cookie:)') "$Case leaked credential-shaped output"
}

function Assert-InjectedPublisherFailpoint {
    param(
        [Parameter(Mandatory)]$Invocation,
        [Parameter(Mandatory)][string]$Failpoint
    )
    Assert-FailedWithoutOutput -Invocation $Invocation -Case "publisher failpoint $Failpoint"
    Assert-Contract ($Invocation.ExitCode -eq 97) "publisher failpoint $Failpoint exited at an unexpected boundary/code: $($Invocation.ExitCode)"
    Assert-Contract (@($Invocation.Stderr).Count -eq 1 -and [string]$Invocation.Stderr[0] -ceq "Injected publisher failpoint: $Failpoint") "publisher failpoint $Failpoint did not prove exact boundary injection"
}

function Assert-PublisherFailpointBoundary {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][string]$Failpoint
    )
    $common = (Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()
    $executionStateRoot = Join-Path $common "dynamo-remediation/publication-state-v1/$($Fixture.ExecutionBaseline)"
    $planRoots = @(Get-ChildItem -LiteralPath $executionStateRoot -Directory -Force -ErrorAction SilentlyContinue)
    Assert-Contract ($planRoots.Count -eq 1 -and $planRoots[0].Name -match '^[0-9a-f]{64}$') "publisher failpoint $Failpoint did not leave exactly one deterministic plan-set state root"
    $publicationRoot = $planRoots[0].FullName
    $activeCount = @(Get-ChildItem -LiteralPath $publicationRoot -File -Force -ErrorAction SilentlyContinue | Where-Object Name -CEQ 'active-publication.lock').Count
    $closedCount = @(Get-ChildItem -LiteralPath (Join-Path $publicationRoot 'leases') -File -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '^closed-publication\.' }).Count
    $rowCount = @(Get-ChildItem -LiteralPath (Join-Path $publicationRoot 'journal/rows') -File -Force -ErrorAction SilentlyContinue).Count
    $rowTempCount = @(Get-ChildItem -LiteralPath (Join-Path $publicationRoot 'journal/tmp') -File -Force -ErrorAction SilentlyContinue).Count
    $bindingTempCount = @(Get-ChildItem -LiteralPath (Join-Path $publicationRoot 'binding-tmp') -File -Force -ErrorAction SilentlyContinue).Count
    $bindingExists = Test-Path -LiteralPath (Join-Path $common 'dynamo-remediation/plan-set-binding-v1.json') -PathType Leaf
    $bundleExecutionRoot = Join-Path $Fixture.EvidenceRoot "Dynamo/plan-set/$($Fixture.ExecutionBaseline)"
    $bundleCount = @(Get-ChildItem -LiteralPath $bundleExecutionRoot -Directory -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '^[0-9a-f]{64}$' }).Count

    $expected = switch ($Failpoint) {
        'after-lease-create' { @(1,0,0,0,0,$false,0) }
        'after-bundle-publish' { @(1,0,0,0,0,$false,1) }
        'after-row-temp-write' { @(1,0,0,1,0,$false,1) }
        'after-bundle-prepared' { @(1,0,1,0,0,$false,1) }
        'after-binding-temp-write' { @(1,0,1,0,1,$false,1) }
        'after-binding-create' { @(1,0,1,0,0,$true,1) }
        'after-binding-committed' { @(1,0,2,0,0,$true,1) }
        'after-lease-close' { @(0,1,2,0,0,$true,1) }
        default { throw "Unknown publisher boundary assertion: $Failpoint" }
    }
    $actual = @($activeCount,$closedCount,$rowCount,$rowTempCount,$bindingTempCount,[bool]$bindingExists,$bundleCount)
    Assert-Contract (($actual -join ',') -ceq ($expected -join ',')) "publisher failpoint $Failpoint state boundary mismatch; actual=$($actual -join ',') expected=$($expected -join ',')"
}

function New-PublisherFixture {
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

    $worktree = Invoke-Git -WorkingDirectory $repository -Arguments @('worktree', 'add', '--quiet', '--detach', $linked, 'HEAD')
    Assert-Contract ($worktree.ExitCode -eq 0) 'linked worktree creation failed'

    foreach ($name in $PlanNames) {
        $source = Join-Path $SourceRepository "docs/superpowers/plans/$name"
        Assert-Contract (Test-Path -LiteralPath $source -PathType Leaf) "reviewed plan source is missing: $name"
        Copy-FileExact -Source $source -Destination (Join-Path $repository "docs/superpowers/plans/$name")
    }

    $publisherSource = Join-Path $SourceRepository 'scripts/remediation/publish-plan-set.ps1'
    if (-not (Test-Path -LiteralPath $publisherSource -PathType Leaf)) {
        throw "plan-set-publisher-contract: expected implementation missing: scripts/remediation/publish-plan-set.ps1"
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
        $commit = Invoke-Git -WorkingDirectory $repository -Arguments @('commit', '--quiet', '-m', 'fixture: install bootstrap contract inputs')
        Assert-Contract ($commit.ExitCode -eq 0) 'fixture bootstrap commit failed'
    }
    $executionBaseline = (Invoke-Git -WorkingDirectory $repository -Arguments @('rev-parse', 'HEAD')).Output[-1].Trim()
    $null = Invoke-Git -WorkingDirectory $linked -Arguments @('reset', '--hard', $executionBaseline)
    foreach ($name in $PlanNames) {
        Copy-FileExact -Source (Join-Path $repository "docs/superpowers/plans/$name") -Destination (Join-Path $linked "docs/superpowers/plans/$name")
    }
    Assert-Contract ($executionBaseline -match '^[0-9a-f]{40}$') 'fixture execution baseline is malformed'
    $parents = @(((Invoke-Git -WorkingDirectory $repository -Arguments @('show', '-s', '--format=%P', $executionBaseline)).Output[-1].Trim() -split ' ') | Where-Object { $_ })
    Assert-Contract ($parents.Count -eq 1 -and $parents[0] -ceq $AuditBaseline) 'fixture execution commit must have the audit baseline as its sole parent'
    $changed = @((Invoke-Git -WorkingDirectory $repository -Arguments @('diff-tree', '--no-commit-id', '--name-only', '-r', $executionBaseline)).Output | Sort-Object -CaseSensitive)
    Assert-Contract (($changed -join "`n") -ceq ($expectedChanged -join "`n")) 'fixture execution commit does not change exactly the four bootstrap files'
    $audit = Invoke-Git -WorkingDirectory $repository -Arguments @('cat-file', '-e', "$AuditBaseline^{commit}") -AllowFailure
    Assert-Contract ($audit.ExitCode -eq 0) 'fixed audit baseline is absent from fixture clone'
    $fixtureStatus = @((Invoke-Git -WorkingDirectory $repository -Arguments @('status', '--porcelain=v1', '--untracked-files=all')).Output)
    Assert-Contract ($fixtureStatus.Count -eq 0) 'fixture must be clean before publisher invocation'

    [pscustomobject]@{
        Repository = $repository
        LinkedWorktree = $linked
        EvidenceRoot = $evidence
        ExecutionBaseline = $executionBaseline
    }
}

function Assert-PublisherFailpointRecovery {
    param(
        [Parameter(Mandatory)][string]$SourceRepository,
        [Parameter(Mandatory)][string]$CaseRoot,
        [Parameter(Mandatory)][string]$Failpoint,
        [switch]$CheckGuard
    )

    $null = New-Item -ItemType Directory -Path $CaseRoot -Force
    $fixture = New-PublisherFixture -SourceRepository $SourceRepository -SuiteRoot $CaseRoot
    if ($CheckGuard) {
        $longEvidence = Join-Path $CaseRoot (('long-evidence-a-' + ('x' * 72)) + [System.IO.Path]::DirectorySeparatorChar + ('long-evidence-b-' + ('y' * 72)) + [System.IO.Path]::DirectorySeparatorChar + ('long-evidence-c-' + ('z' * 72)))
        [System.IO.Directory]::CreateDirectory($longEvidence) | Out-Null
        $fixture.EvidenceRoot = $longEvidence
        $longPathProbe = Join-Path $longEvidence ('Dynamo/plan-set/' + ('f' * 40) + '/' + ('a' * 64) + '/plan-set-manifest-v1.json')
        Assert-Contract ($longPathProbe.Length -gt 260) 'publisher long-path fixture does not exceed the legacy Windows limit'
    }
    $controlRoot = Join-Path $fixture.Repository '.git/dynamo-remediation'
    if ($CheckGuard) {
        $guardBefore = Get-TreeFingerprint -Root $controlRoot
        $guardEvidenceBefore = Get-TreeFingerprint -Root $fixture.EvidenceRoot
        $unguarded = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Case "unguarded publisher failpoint $Failpoint" -Environment @{
            DYNAMO_REMEDIATION_TEST_FAILPOINT = $Failpoint
        }
        Assert-Contract (($guardBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $controlRoot) -join "`n")) 'unguarded publisher failpoint mutated control state'
        Assert-Contract (($guardEvidenceBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $fixture.EvidenceRoot) -join "`n")) 'unguarded publisher failpoint mutated external evidence'
        $unknown = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Case 'unknown guarded publisher failpoint' -Environment @{
            DYNAMO_REMEDIATION_TEST_MODE = '1'
            DYNAMO_REMEDIATION_TEST_FAILPOINT = 'not-a-publisher-boundary'
        }
        Assert-Contract (($guardBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $controlRoot) -join "`n")) 'unknown publisher failpoint mutated control state'
        Assert-Contract (($guardEvidenceBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $fixture.EvidenceRoot) -join "`n")) 'unknown publisher failpoint mutated external evidence'
    }
    $failed = Complete-Publisher -Running (Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_TEST_FAILPOINT = $Failpoint
    })
    Assert-InjectedPublisherFailpoint -Invocation $failed -Failpoint $Failpoint
    Assert-PublisherFailpointBoundary -Fixture $fixture -Failpoint $Failpoint
    $recovered = Convert-Handoff -Invocation (Invoke-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot)
    Assert-Contract ($recovered.publication_tail_sha256 -match '^[0-9a-f]{64}$') "publisher recovery after $Failpoint did not converge"
    $publicationRoot = Join-Path $fixture.Repository ".git/dynamo-remediation/publication-state-v1/$($fixture.ExecutionBaseline)/$($recovered.plan_set_sha256)"
    Assert-Contract (-not (Test-Path -LiteralPath (Join-Path $publicationRoot 'active-publication.lock'))) "publisher recovery after $Failpoint left an active lease"
    $closed = @(Get-ChildItem -LiteralPath (Join-Path $publicationRoot 'leases') -File | Where-Object { $_.Name -match '^closed-publication\.[0-9a-f]{32}\.g\d{10}\.[0-9a-f]{64}\.lock$' })
    Assert-Contract ($closed.Count -eq 1) "publisher recovery after $Failpoint did not reach PublicationComplete"
    $closedText = Get-Content -LiteralPath $closed[0].FullName -Raw
    $closedObject = $closedText | ConvertFrom-Json -ErrorAction Stop
    $archivesRoot = Join-Path $publicationRoot 'leases/archives'
    $archives = @(Get-ChildItem -LiteralPath $archivesRoot -File | Sort-Object Name)
    Assert-Contract ($archives.Count -eq ([long]$closedObject.generation - 1)) "publisher recovery after $Failpoint produced a noncontiguous archive count"
    $prior = '0' * 64
    [long]$expectedGeneration = 1
    foreach ($archive in $archives) {
        $archiveText = Get-Content -LiteralPath $archive.FullName -Raw
        $archiveObject = $archiveText | ConvertFrom-Json -ErrorAction Stop
        Assert-SelfOmittingHash -Raw $archiveText -Property 'lease_sha256' -Domain 'dynamo-publication-lease-v1' -Expected $archiveObject.lease_sha256 -Case "publisher recovery archive g$expectedGeneration"
        Assert-Contract ([long]$archiveObject.generation -eq $expectedGeneration -and $archiveObject.prior_lease_sha256 -ceq $prior -and $archiveObject.attempt_id -ceq $closedObject.attempt_id) "publisher recovery after $Failpoint produced a broken archive link"
        Assert-Contract ($archive.Name -ceq ("publication-lease.{0}.g{1:d10}.{2}.lock" -f $archiveObject.attempt_id, [long]$archiveObject.generation, $archiveObject.lease_sha256)) "publisher recovery after $Failpoint produced a mismatched archive filename"
        $prior = $archiveObject.lease_sha256
        $expectedGeneration++
    }
    Assert-Contract ($closedObject.prior_lease_sha256 -ceq $prior) "publisher recovery after $Failpoint closed a lease outside the archive chain"
    $rowOrphans = @(Get-ChildItem -LiteralPath (Join-Path $publicationRoot 'journal/orphans') -File)
    $bindingOrphans = @(Get-ChildItem -LiteralPath (Join-Path $publicationRoot 'binding-orphans') -File)
    Assert-Contract ($rowOrphans.Count -eq $(if ($Failpoint -ceq 'after-row-temp-write') { 1 } else { 0 })) "publisher recovery after $Failpoint produced an unexpected row-orphan set"
    Assert-Contract ($bindingOrphans.Count -eq $(if ($Failpoint -ceq 'after-binding-temp-write') { 1 } else { 0 })) "publisher recovery after $Failpoint produced an unexpected binding-orphan set"
    Assert-ContractAclTreeSemantics -Root (Join-Path $fixture.Repository '.git/dynamo-remediation')
    Assert-ContractAclTreeSemantics -Root (Join-Path $fixture.EvidenceRoot 'Dynamo')
    if ($CheckGuard -and $archives.Count -gt 0) {
        $archiveBytes = [System.IO.File]::ReadAllBytes($archives[0].FullName)
        [System.IO.File]::WriteAllBytes($archives[0].FullName, $archiveBytes + [byte]10)
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Case 'tampered publication archive'
        [System.IO.File]::WriteAllBytes($archives[0].FullName, $archiveBytes)
        $gapPath = Join-Path $CaseRoot 'temporarily-removed-publication-archive.lock'
        Move-Item -LiteralPath $archives[0].FullName -Destination $gapPath
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Case 'gapped publication archive chain'
        Move-Item -LiteralPath $gapPath -Destination $archives[0].FullName
    }
}

function Assert-ExactPreexistingBundleRejected {
    param(
        [Parameter(Mandatory)][string]$SourceRepository,
        [Parameter(Mandatory)][string]$CaseRoot
    )
    $null = New-Item -ItemType Directory -Path $CaseRoot -Force
    $fixture = New-PublisherFixture -SourceRepository $SourceRepository -SuiteRoot $CaseRoot
    $published = Convert-Handoff -Invocation (Invoke-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot)
    $sourceBundle = Join-Path $fixture.EvidenceRoot "Dynamo/plan-set/$($fixture.ExecutionBaseline)/$($published.plan_set_sha256)"
    $copiedEvidence = Join-Path $CaseRoot 'copied-exact-evidence'
    $null = New-Item -ItemType Directory -Path $copiedEvidence
    Copy-Item -LiteralPath (Join-Path $fixture.EvidenceRoot 'Dynamo') -Destination $copiedEvidence -Recurse
    $copiedBundle = Join-Path $copiedEvidence "Dynamo/plan-set/$($fixture.ExecutionBaseline)/$($published.plan_set_sha256)"
    $sourceFingerprint = (Get-TreeFingerprint -Root $sourceBundle) -join "`n"
    $copiedFingerprint = (Get-TreeFingerprint -Root $copiedBundle) -join "`n"
    Assert-Contract ($sourceFingerprint -ceq $copiedFingerprint) 'exact pre-existing bundle fixture copy changed content'
    $common = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()
    $controlRoot = Join-Path $common 'dynamo-remediation'
    Assert-PathUnderRoot -Path $controlRoot -Root $CaseRoot -Message 'exact-bundle fixture control root escaped its owned case root'
    Remove-Item -LiteralPath $controlRoot -Recurse -Force
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $copiedEvidence -Case 'exact bundle without recovery provenance'
    Assert-Contract (-not (Test-Path -LiteralPath $controlRoot)) 'unproven exact bundle created common-dir publication state'
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $copiedEvidence -Case 'repeated exact bundle without recovery provenance'
    Assert-Contract (-not (Test-Path -LiteralPath (Join-Path $controlRoot 'plan-set-binding-v1.json'))) 'unproven exact bundle created a binding'
    $publicationRoot = Join-Path $controlRoot "publication-state-v1/$($fixture.ExecutionBaseline)/$($published.plan_set_sha256)"
    Assert-Contract (@(Get-ChildItem -LiteralPath (Join-Path $publicationRoot 'journal/rows') -File -ErrorAction SilentlyContinue).Count -eq 0) 'unproven exact bundle created a publication row'
}

function Assert-PublisherGitObjectHardening {
    param(
        [Parameter(Mandatory)][string]$SourceRepository,
        [Parameter(Mandatory)][string]$CaseRoot
    )

    $null = New-Item -ItemType Directory -Path $CaseRoot -Force
    $fixture = New-PublisherFixture -SourceRepository $SourceRepository -SuiteRoot $CaseRoot
    $publisherPath = Join-Path $fixture.Repository 'scripts/remediation/publish-plan-set.ps1'
    $publisherBytes = [System.IO.File]::ReadAllBytes($publisherPath)
    $attributesPath = Join-Path $fixture.Repository '.git/info/attributes'
    $attributesExisted = Test-Path -LiteralPath $attributesPath -PathType Leaf
    $attributesBytes = if ($attributesExisted) { [System.IO.File]::ReadAllBytes($attributesPath) } else { $null }
    try {
        $prefix = if ($attributesExisted -and $attributesBytes.Length -gt 0 -and $attributesBytes[-1] -ne 10) { "`n" } else { '' }
        [System.IO.File]::WriteAllText($attributesPath, "$prefix/scripts/remediation/publish-plan-set.ps1 filter=contractmask`n", [System.Text.UTF8Encoding]::new($false))
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('config', 'filter.contractmask.clean', 'git show HEAD:scripts/remediation/publish-plan-set.ps1')
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('config', 'filter.contractmask.required', 'true')
        [System.IO.File]::WriteAllBytes($publisherPath, $publisherBytes + [System.Text.UTF8Encoding]::new($false).GetBytes("`n# filter-aware-hash-bypass-fixture`n"))
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('update-index', '--assume-unchanged', '--', 'scripts/remediation/publish-plan-set.ps1')

        $filteredHash = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('hash-object', '--path=scripts/remediation/publish-plan-set.ps1', '--', $publisherPath)).Output[-1].Trim()
        $headHash = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('rev-parse', 'HEAD:scripts/remediation/publish-plan-set.ps1')).Output[-1].Trim()
        Assert-Contract ($filteredHash -ceq $headHash) 'clean-filter fixture did not mask the non-HEAD publisher bytes'
        Assert-Contract (@((Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('status', '--porcelain=v1', '--untracked-files=all')).Output).Count -eq 0) 'clean-filter fixture was not Git-clean'
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Case 'clean-filter masked publisher bytes'
        Assert-Contract (-not (Test-Path -LiteralPath (Join-Path $fixture.Repository '.git/dynamo-remediation'))) 'clean-filter masked bytes mutated common-dir publication state'
    }
    finally {
        [System.IO.File]::WriteAllBytes($publisherPath, $publisherBytes)
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('update-index', '--no-assume-unchanged', '--', 'scripts/remediation/publish-plan-set.ps1') -AllowFailure
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('config', '--unset-all', 'filter.contractmask.clean') -AllowFailure
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('config', '--unset-all', 'filter.contractmask.required') -AllowFailure
        if ($attributesExisted) { [System.IO.File]::WriteAllBytes($attributesPath, $attributesBytes) }
        elseif (Test-Path -LiteralPath $attributesPath) { Remove-Item -LiteralPath $attributesPath -Force }
    }

    Assert-Contract (@((Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('status', '--porcelain=v1', '--untracked-files=all')).Output).Count -eq 0) 'clean-filter fixture cleanup left the repository dirty'
    $tree = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('rev-parse', "$($fixture.ExecutionBaseline)^{tree}")).Output[-1].Trim()
    $replacement = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('commit-tree', $tree, '-m', 'fixture replacement root')).Output[-1].Trim()
    $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('replace', $fixture.ExecutionBaseline, $replacement)
    try {
        $visibleParents = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('show', '-s', '--format=%P', 'HEAD')).Output[-1].Trim()
        Assert-Contract ([string]::IsNullOrEmpty($visibleParents)) 'replace-ref fixture did not alter ordinary Git commit semantics'
        $published = Convert-Handoff -Invocation (Invoke-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot)
        Assert-Contract ($published.execution_baseline -ceq $fixture.ExecutionBaseline) 'publisher did not ignore Git replacement-object semantics'
    }
    finally {
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('replace', '-d', $fixture.ExecutionBaseline) -AllowFailure
    }
}

function Assert-PublisherRepositoryIntegrityHardening {
    param(
        [Parameter(Mandatory)][string]$SourceRepository,
        [Parameter(Mandatory)][string]$CaseRoot
    )
    $null = New-Item -ItemType Directory -Path $CaseRoot -Force
    $fixture = New-PublisherFixture -SourceRepository $SourceRepository -SuiteRoot $CaseRoot
    $relative = 'Cargo.toml'
    $trackedPath = Join-Path $fixture.Repository $relative
    $trackedBytes = [System.IO.File]::ReadAllBytes($trackedPath)
    $tamper = [System.Text.UTF8Encoding]::new($false).GetBytes("`n# unrelated-tracked-integrity-bypass`n")

    try {
        [System.IO.File]::WriteAllBytes($trackedPath, $trackedBytes + $tamper)
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('update-index', '--assume-unchanged', '--', $relative)
        Assert-Contract (@((Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('status', '--porcelain=v1', '--untracked-files=all')).Output).Count -eq 0) 'assume-unchanged fixture did not conceal unrelated tracked bytes'
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Case 'assume-unchanged unrelated tracked bytes'
    }
    finally {
        [System.IO.File]::WriteAllBytes($trackedPath, $trackedBytes)
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('update-index', '--no-assume-unchanged', '--', $relative) -AllowFailure
    }

    try {
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('update-index', '--skip-worktree', '--', $relative)
        [System.IO.File]::WriteAllBytes($trackedPath, $trackedBytes + $tamper)
        Assert-Contract (@((Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('status', '--porcelain=v1', '--untracked-files=all')).Output).Count -eq 0) 'skip-worktree fixture did not conceal unrelated tracked bytes'
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Case 'skip-worktree unrelated tracked bytes'
    }
    finally {
        [System.IO.File]::WriteAllBytes($trackedPath, $trackedBytes)
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('update-index', '--no-skip-worktree', '--', $relative) -AllowFailure
    }

    $attributesPath = Join-Path $fixture.Repository '.git/info/attributes'
    $attributesExisted = Test-Path -LiteralPath $attributesPath -PathType Leaf
    $attributesBytes = if ($attributesExisted) { [System.IO.File]::ReadAllBytes($attributesPath) } else { $null }
    try {
        [System.IO.File]::WriteAllText($attributesPath, "/$relative filter=wholeworktreemask`n", [System.Text.UTF8Encoding]::new($false))
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('config', 'filter.wholeworktreemask.clean', "git show HEAD:$relative")
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('config', 'filter.wholeworktreemask.required', 'true')
        [System.IO.File]::WriteAllBytes($trackedPath, $trackedBytes + $tamper)
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('add', '--', $relative)
        $filteredHash = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('hash-object', "--path=$relative", '--', $trackedPath)).Output[-1].Trim()
        $headHash = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('rev-parse', "HEAD:$relative")).Output[-1].Trim()
        Assert-Contract ($filteredHash -ceq $headHash) 'whole-worktree clean-filter fixture did not map tampered bytes to HEAD'
        Assert-Contract (@((Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('status', '--porcelain=v1', '--untracked-files=all')).Output).Count -eq 0) 'whole-worktree clean-filter fixture was not Git-clean after index stat refresh'
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Case 'clean-filter unrelated tracked bytes'
    }
    finally {
        [System.IO.File]::WriteAllBytes($trackedPath, $trackedBytes)
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('add', '--', $relative) -AllowFailure
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('config', '--unset-all', 'filter.wholeworktreemask.clean') -AllowFailure
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('config', '--unset-all', 'filter.wholeworktreemask.required') -AllowFailure
        if ($attributesExisted) { [System.IO.File]::WriteAllBytes($attributesPath, $attributesBytes) }
        elseif (Test-Path -LiteralPath $attributesPath) { Remove-Item -LiteralPath $attributesPath -Force }
    }
    Assert-Contract (@((Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('status', '--porcelain=v1', '--untracked-files=all')).Output).Count -eq 0) 'whole-worktree concealment fixtures did not restore clean state'

    $graftsPath = Join-Path $fixture.Repository '.git/info/grafts'
    $graftsExisted = Test-Path -LiteralPath $graftsPath -PathType Leaf
    $graftsBytes = if ($graftsExisted) { [System.IO.File]::ReadAllBytes($graftsPath) } else { $null }
    $tree = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('rev-parse', "$($fixture.ExecutionBaseline)^{tree}")).Output[-1].Trim()
    $invalidHead = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('commit-tree', $tree, '-p', $fixture.ExecutionBaseline, '-m', 'fixture raw-parent violation')).Output[-1].Trim()
    $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('reset', '--hard', $invalidHead)
    try {
        [System.IO.File]::WriteAllText($graftsPath, "$invalidHead $AuditBaseline`n", [System.Text.UTF8Encoding]::new($false))
        $visibleParents = @(& git -C $fixture.Repository show -s '--format=%P' HEAD 2>$null)
        Assert-Contract ($LASTEXITCODE -eq 0 -and $visibleParents[-1].Trim() -ceq $AuditBaseline) 'legacy graft fixture did not forge the ordinary visible parent'
        $visibleChanged = @(& git -C $fixture.Repository diff-tree --no-commit-id --name-only -r HEAD 2>$null | Sort-Object -CaseSensitive)
        Assert-Contract ($LASTEXITCODE -eq 0 -and (($visibleChanged -join "`n") -ceq (($BootstrapPaths | Sort-Object -CaseSensitive) -join "`n"))) 'legacy graft fixture did not forge the ordinary bootstrap diff'
        $rawCommit = @(& git -C $fixture.Repository cat-file commit $invalidHead 2>$null)
        Assert-Contract ($LASTEXITCODE -eq 0 -and @($rawCommit | Where-Object { $_ -ceq "parent $($fixture.ExecutionBaseline)" }).Count -eq 1) 'legacy graft fixture raw parent is not the invalid execution descendant'
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Case 'legacy graft forged execution parent'
    }
    finally {
        if ($graftsExisted) { [System.IO.File]::WriteAllBytes($graftsPath, $graftsBytes) }
        elseif (Test-Path -LiteralPath $graftsPath) { Remove-Item -LiteralPath $graftsPath -Force }
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('reset', '--hard', $fixture.ExecutionBaseline)
    }
    Assert-Contract (@((Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('status', '--porcelain=v1', '--untracked-files=all')).Output).Count -eq 0) 'legacy graft fixture cleanup left the repository dirty'

    $literalPath = Join-Path $CaseRoot 'malformed-order-commit.raw'
    $tree = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('rev-parse', "$($fixture.ExecutionBaseline)^{tree}")).Output[-1].Trim()
    $malformedCommitText = "tree $tree`nauthor Contract Fixture <contract@example.invalid> 1700000000 +0000`nparent $AuditBaseline`ncommitter Contract Fixture <contract@example.invalid> 1700000000 +0000`n`nmalformed parent order`n"
    [System.IO.File]::WriteAllText($literalPath, $malformedCommitText, [System.Text.UTF8Encoding]::new($false))
    $literalResult = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('hash-object', '-t', 'commit', '-w', '--literally', '--', $literalPath)
    $malformedHead = $literalResult.Output[-1].Trim()
    Assert-Contract ($malformedHead -match '^[0-9a-f]{40}$') 'could not write malformed-order literal commit object'
    $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('update-ref', 'HEAD', $malformedHead, $fixture.ExecutionBaseline)
    try {
        $rawCommit = @(& git -C $fixture.Repository cat-file commit $malformedHead 2>$null)
        Assert-Contract ($LASTEXITCODE -eq 0 -and ($rawCommit -join "`n").Contains("author Contract Fixture <contract@example.invalid> 1700000000 +0000`nparent $AuditBaseline", [System.StringComparison]::Ordinal)) 'literal malformed-order commit did not preserve the parent after author'
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Case 'literal commit parent after author'
    }
    finally {
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('update-ref', 'HEAD', $fixture.ExecutionBaseline, $malformedHead) -AllowFailure
        if (Test-Path -LiteralPath $literalPath) { Remove-Item -LiteralPath $literalPath -Force }
    }
    Assert-Contract (@((Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('status', '--porcelain=v1', '--untracked-files=all')).Output).Count -eq 0) 'malformed-order raw commit fixture cleanup left the repository dirty'
}

function Assert-PublisherPartialSkeletonConvergence {
    param(
        [Parameter(Mandatory)][string]$SourceRepository,
        [Parameter(Mandatory)][string]$CaseRoot
    )
    $null = New-Item -ItemType Directory -Path $CaseRoot -Force
    $fixture = New-PublisherFixture -SourceRepository $SourceRepository -SuiteRoot $CaseRoot
    $failed = Complete-Publisher -Running (Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_TEST_FAILPOINT = 'after-lease-create'
    })
    Assert-InjectedPublisherFailpoint -Invocation $failed -Failpoint 'after-lease-create'
    $common = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()
    $controlRoot = Join-Path $common 'dynamo-remediation'
    $executionRoot = Join-Path $controlRoot "publication-state-v1/$($fixture.ExecutionBaseline)"
    $planRoots = @(Get-ChildItem -LiteralPath $executionRoot -Directory -Force)
    Assert-Contract ($planRoots.Count -eq 1 -and $planRoots[0].Name -match '^[0-9a-f]{64}$') 'partial-skeleton fixture could not resolve its plan-set root'
    $planHash = $planRoots[0].Name
    $templateAcl = [System.IO.FileSystemAclExtensions]::GetAccessControl((Get-Item -LiteralPath $controlRoot -Force))
    $templateSections = [System.Security.AccessControl.AccessControlSections]'Access, Owner, Group'
    $templateSddl = $templateAcl.GetSecurityDescriptorSddlForm($templateSections)
    $orderedRelativeDirectories = @(
        '.',
        'publication-state-v1',
        "publication-state-v1/$($fixture.ExecutionBaseline)",
        "publication-state-v1/$($fixture.ExecutionBaseline)/$planHash",
        "publication-state-v1/$($fixture.ExecutionBaseline)/$planHash/leases",
        "publication-state-v1/$($fixture.ExecutionBaseline)/$planHash/leases/archives",
        "publication-state-v1/$($fixture.ExecutionBaseline)/$planHash/journal",
        "publication-state-v1/$($fixture.ExecutionBaseline)/$planHash/journal/rows",
        "publication-state-v1/$($fixture.ExecutionBaseline)/$planHash/journal/tmp",
        "publication-state-v1/$($fixture.ExecutionBaseline)/$planHash/journal/orphans",
        "publication-state-v1/$($fixture.ExecutionBaseline)/$planHash/binding-tmp",
        "publication-state-v1/$($fixture.ExecutionBaseline)/$planHash/binding-orphans"
    )
    $dynamoEvidence = Join-Path $fixture.EvidenceRoot 'Dynamo'
    Assert-PathUnderRoot -Path $controlRoot -Root $CaseRoot -Message 'partial-skeleton control root escaped its owned case root'
    Assert-PathUnderRoot -Path $dynamoEvidence -Root $CaseRoot -Message 'partial-skeleton evidence root escaped its owned case root'
    for ($depth = 1; $depth -le $orderedRelativeDirectories.Count; $depth++) {
        if (Test-Path -LiteralPath $controlRoot) { Remove-Item -LiteralPath $controlRoot -Recurse -Force }
        if (Test-Path -LiteralPath $dynamoEvidence) { Remove-Item -LiteralPath $dynamoEvidence -Recurse -Force }
        for ($index = 0; $index -lt $depth; $index++) {
            $path = if ($orderedRelativeDirectories[$index] -ceq '.') { $controlRoot } else { Join-Path $controlRoot $orderedRelativeDirectories[$index] }
            $null = New-Item -ItemType Directory -Path $path -Force
            Set-ContractDirectoryAclFromSddl -Path $path -Sddl $templateSddl
            Assert-ContractAclSemantics -Path $path -Kind Directory
        }
        $published = Convert-Handoff -Invocation (Invoke-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot)
        Assert-Contract ($published.publication_tail_sha256 -match '^[0-9a-f]{64}$') "partial protected skeleton depth $depth did not converge"
    }

    foreach ($invalidCase in @('wrong-case','unknown-directory','unexpected-file','reparse-directory','acl-drift')) {
        if (Test-Path -LiteralPath $controlRoot) { Remove-Item -LiteralPath $controlRoot -Recurse -Force }
        if (Test-Path -LiteralPath $dynamoEvidence) { Remove-Item -LiteralPath $dynamoEvidence -Recurse -Force }
        foreach ($relative in $orderedRelativeDirectories[0..3]) {
            $path = if ($relative -ceq '.') { $controlRoot } else { Join-Path $controlRoot $relative }
            $null = New-Item -ItemType Directory -Path $path -Force
            Set-ContractDirectoryAclFromSddl -Path $path -Sddl $templateSddl
        }
        $publicationRoot = Join-Path $controlRoot $orderedRelativeDirectories[3]
        $cleanupAcl = $null
        switch ($invalidCase) {
            'wrong-case' {
                $invalid = Join-Path $publicationRoot 'Journal'
                $null = New-Item -ItemType Directory -Path $invalid
                Set-ContractDirectoryAclFromSddl -Path $invalid -Sddl $templateSddl
            }
            'unknown-directory' {
                $invalid = Join-Path $publicationRoot 'unexpected'
                $null = New-Item -ItemType Directory -Path $invalid
                Set-ContractDirectoryAclFromSddl -Path $invalid -Sddl $templateSddl
            }
            'unexpected-file' { [System.IO.File]::WriteAllText((Join-Path $publicationRoot 'unexpected.file'), 'invalid', [System.Text.UTF8Encoding]::new($false)) }
            'reparse-directory' {
                $target = Join-Path $CaseRoot 'skeleton-reparse-target'
                $null = New-Item -ItemType Directory -Path $target -Force
                $null = New-Item -ItemType Junction -Path (Join-Path $publicationRoot 'journal') -Target $target
            }
            'acl-drift' { $cleanupAcl = Add-ContractAclDrift -Path $publicationRoot -Kind Directory }
        }
        try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Case "invalid partial skeleton $invalidCase" }
        finally { if ($null -ne $cleanupAcl) { Restore-ContractAcl $cleanupAcl } }
    }

    foreach ($evidenceCase in @('wrong-case','reparse','acl-drift')) {
        if (Test-Path -LiteralPath $controlRoot) { Remove-Item -LiteralPath $controlRoot -Recurse -Force }
        if (Test-Path -LiteralPath $dynamoEvidence) { Remove-Item -LiteralPath $dynamoEvidence -Recurse -Force }
        $cleanupAcl = $null
        switch ($evidenceCase) {
            'wrong-case' {
                $wrongCase = Join-Path $fixture.EvidenceRoot 'dynamo'
                $null = New-Item -ItemType Directory -Path $wrongCase
                Set-ContractDirectoryAclFromSddl -Path $wrongCase -Sddl $templateSddl
            }
            'reparse' {
                $target = Join-Path $CaseRoot 'evidence-reparse-target'
                $null = New-Item -ItemType Directory -Path $target -Force
                $null = New-Item -ItemType Junction -Path $dynamoEvidence -Target $target
            }
            'acl-drift' {
                $null = New-Item -ItemType Directory -Path $dynamoEvidence
                Set-ContractDirectoryAclFromSddl -Path $dynamoEvidence -Sddl $templateSddl
                $cleanupAcl = Add-ContractAclDrift -Path $dynamoEvidence -Kind Directory
            }
        }
        try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Case "invalid partial evidence ancestor $evidenceCase" }
        finally { if ($null -ne $cleanupAcl) { Restore-ContractAcl $cleanupAcl } }
    }
}

function New-PublisherCrashFixture {
    param(
        [Parameter(Mandatory)][string]$SourceRepository,
        [Parameter(Mandatory)][string]$CaseRoot,
        [Parameter(Mandatory)][string]$Failpoint
    )
    $null = New-Item -ItemType Directory -Path $CaseRoot -Force
    $fixture = New-PublisherFixture -SourceRepository $SourceRepository -SuiteRoot $CaseRoot
    $failed = Complete-Publisher -Running (Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_TEST_FAILPOINT = $Failpoint
    })
    Assert-InjectedPublisherFailpoint -Invocation $failed -Failpoint $Failpoint
    $fixture
}

function Get-PublisherFixtureStatePaths {
    param([Parameter(Mandatory)]$Fixture)
    $common = (Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()
    $control = Join-Path $common 'dynamo-remediation'
    $executionRoot = Join-Path $control "publication-state-v1/$($Fixture.ExecutionBaseline)"
    $planRoots = @(Get-ChildItem -LiteralPath $executionRoot -Directory -Force)
    Assert-Contract ($planRoots.Count -eq 1 -and $planRoots[0].Name -match '^[0-9a-f]{64}$') 'could not resolve exactly one publisher fixture state root'
    [pscustomobject]@{
        Common = $common
        Control = $control
        Publication = $planRoots[0].FullName
        PlanHash = $planRoots[0].Name
        Bundle = Join-Path $Fixture.EvidenceRoot "Dynamo/plan-set/$($Fixture.ExecutionBaseline)/$($planRoots[0].Name)"
        Binding = Join-Path $control 'plan-set-binding-v1.json'
        Prepared = Join-Path $planRoots[0].FullName 'journal/rows/00000000000000000001-bundle-prepared.json'
        Committed = Join-Path $planRoots[0].FullName 'journal/rows/00000000000000000002-binding-committed.json'
        Active = Join-Path $planRoots[0].FullName 'active-publication.lock'
    }
}

function Assert-PublisherPreterminalAdmissionHardening {
    param(
        [Parameter(Mandatory)][string]$SourceRepository,
        [Parameter(Mandatory)][string]$CaseRoot
    )
    $null = New-Item -ItemType Directory -Path $CaseRoot -Force

    $completeRoot = Join-Path $CaseRoot 'binding-without-state'
    $null = New-Item -ItemType Directory -Path $completeRoot -Force
    $complete = New-PublisherFixture -SourceRepository $SourceRepository -SuiteRoot $completeRoot
    $null = Convert-Handoff -Invocation (Invoke-Publisher -Repository $complete.Repository -EvidenceRoot $complete.EvidenceRoot)
    $completePaths = Get-PublisherFixtureStatePaths $complete
    $publicationHold = Join-Path $completeRoot 'held-publication-state'
    Move-Item -LiteralPath $completePaths.Publication -Destination $publicationHold
    try {
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $complete.Repository -EvidenceRoot $complete.EvidenceRoot -Case 'final bundle plus binding without publication state'
        $bundleHold = Join-Path $completeRoot 'held-final-bundle'
        Move-Item -LiteralPath $completePaths.Bundle -Destination $bundleHold
        try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $complete.Repository -EvidenceRoot $complete.EvidenceRoot -Case 'binding without publication state or final bundle' }
        finally { Move-Item -LiteralPath $bundleHold -Destination $completePaths.Bundle }
    }
    finally { Move-Item -LiteralPath $publicationHold -Destination $completePaths.Publication }

    $bindingCrash = New-PublisherCrashFixture -SourceRepository $SourceRepository -CaseRoot (Join-Path $CaseRoot 'binding-crash') -Failpoint 'after-binding-create'
    $bindingPaths = Get-PublisherFixtureStatePaths $bindingCrash
    $bindingBytes = [System.IO.File]::ReadAllBytes($bindingPaths.Binding)
    [System.IO.File]::WriteAllBytes($bindingPaths.Binding, $bindingBytes + [byte]10)
    try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $bindingCrash.Repository -EvidenceRoot $bindingCrash.EvidenceRoot -Case 'prepared row plus corrupt fixed binding' }
    finally { [System.IO.File]::WriteAllBytes($bindingPaths.Binding, $bindingBytes) }
    $preparedHold = Join-Path $CaseRoot 'held-binding-prepared-row'
    Move-Item -LiteralPath $bindingPaths.Prepared -Destination $preparedHold
    try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $bindingCrash.Repository -EvidenceRoot $bindingCrash.EvidenceRoot -Case 'binding without BundlePrepared' }
    finally { Move-Item -LiteralPath $preparedHold -Destination $bindingPaths.Prepared }

    $bundleCrash = New-PublisherCrashFixture -SourceRepository $SourceRepository -CaseRoot (Join-Path $CaseRoot 'bundle-crash') -Failpoint 'after-bundle-publish'
    $bundlePaths = Get-PublisherFixtureStatePaths $bundleCrash
    $manifestPath = Join-Path $bundlePaths.Bundle 'plan-set-manifest-v1.json'
    $manifestBytes = [System.IO.File]::ReadAllBytes($manifestPath)
    [System.IO.File]::WriteAllBytes($manifestPath, $manifestBytes + [byte]10)
    try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $bundleCrash.Repository -EvidenceRoot $bundleCrash.EvidenceRoot -Case 'active lease plus corrupt existing manifest' }
    finally { [System.IO.File]::WriteAllBytes($manifestPath, $manifestBytes) }
    $payloadPath = @(Get-ChildItem -LiteralPath $bundlePaths.Bundle -File -Recurse | Where-Object Name -cne 'plan-set-manifest-v1.json')[0].FullName
    $payloadBytes = [System.IO.File]::ReadAllBytes($payloadPath)
    [System.IO.File]::WriteAllBytes($payloadPath, $payloadBytes + [byte]10)
    try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $bundleCrash.Repository -EvidenceRoot $bundleCrash.EvidenceRoot -Case 'active lease plus corrupt existing payload' }
    finally { [System.IO.File]::WriteAllBytes($payloadPath, $payloadBytes) }

    $preparedCrash = New-PublisherCrashFixture -SourceRepository $SourceRepository -CaseRoot (Join-Path $CaseRoot 'prepared-crash') -Failpoint 'after-bundle-prepared'
    $preparedPaths = Get-PublisherFixtureStatePaths $preparedCrash
    $activeHold = Join-Path $CaseRoot 'held-prepared-active-publication.lock'
    Move-Item -LiteralPath $preparedPaths.Active -Destination $activeHold
    try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $preparedCrash.Repository -EvidenceRoot $preparedCrash.EvidenceRoot -Case 'BundlePrepared without active or archive lease' }
    finally { Move-Item -LiteralPath $activeHold -Destination $preparedPaths.Active }
    $activeObject = (Get-Content -LiteralPath $preparedPaths.Active -Raw) | ConvertFrom-Json -ErrorAction Stop
    $closedPath = Join-Path (Join-Path $preparedPaths.Publication 'leases') ("closed-publication.{0}.g{1:d10}.{2}.lock" -f $activeObject.attempt_id, [int64]$activeObject.generation, $activeObject.lease_sha256)
    Move-Item -LiteralPath $preparedPaths.Active -Destination $closedPath
    try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $preparedCrash.Repository -EvidenceRoot $preparedCrash.EvidenceRoot -Case 'preterminal closed lease' }
    finally { Move-Item -LiteralPath $closedPath -Destination $preparedPaths.Active }
    [System.IO.File]::WriteAllText((Join-Path $preparedPaths.Publication 'unknown-child'), 'invalid', [System.Text.UTF8Encoding]::new($false))
    try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $preparedCrash.Repository -EvidenceRoot $preparedCrash.EvidenceRoot -Case 'unknown preterminal child' }
    finally { Remove-Item -LiteralPath (Join-Path $preparedPaths.Publication 'unknown-child') -Force }

    $rowTempCrash = New-PublisherCrashFixture -SourceRepository $SourceRepository -CaseRoot (Join-Path $CaseRoot 'row-temp-crash') -Failpoint 'after-row-temp-write'
    $rowTempPaths = Get-PublisherFixtureStatePaths $rowTempCrash
    $rowTemps = @(Get-ChildItem -LiteralPath (Join-Path $rowTempPaths.Publication 'journal/tmp') -File)
    Assert-Contract ($rowTemps.Count -eq 1) 'row-temp collision fixture did not expose exactly one temp'
    $rowRawHash = (Get-FileHash -LiteralPath $rowTemps[0].FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    $rowCollision = Join-Path (Join-Path $rowTempPaths.Publication 'journal/orphans') "$($rowTemps[0].BaseName).$rowRawHash.orphan"
    Copy-Item -LiteralPath $rowTemps[0].FullName -Destination $rowCollision
    try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $rowTempCrash.Repository -EvidenceRoot $rowTempCrash.EvidenceRoot -Case 'row temp deterministic orphan collision' }
    finally { Remove-Item -LiteralPath $rowCollision -Force }
    $rowActiveHold = Join-Path $CaseRoot 'held-row-temp-active.lock'
    Move-Item -LiteralPath $rowTempPaths.Active -Destination $rowActiveHold
    try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $rowTempCrash.Repository -EvidenceRoot $rowTempCrash.EvidenceRoot -Case 'row temp without active/archive chain' }
    finally { Move-Item -LiteralPath $rowActiveHold -Destination $rowTempPaths.Active }

    $bindingTempCrash = New-PublisherCrashFixture -SourceRepository $SourceRepository -CaseRoot (Join-Path $CaseRoot 'binding-temp-crash') -Failpoint 'after-binding-temp-write'
    $bindingTempPaths = Get-PublisherFixtureStatePaths $bindingTempCrash
    $bindingTemps = @(Get-ChildItem -LiteralPath (Join-Path $bindingTempPaths.Publication 'binding-tmp') -File)
    Assert-Contract ($bindingTemps.Count -eq 1) 'binding-temp collision fixture did not expose exactly one temp'
    $bindingRawHash = (Get-FileHash -LiteralPath $bindingTemps[0].FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    $bindingCollision = Join-Path (Join-Path $bindingTempPaths.Publication 'binding-orphans') "$($bindingTemps[0].BaseName).$bindingRawHash.orphan"
    Copy-Item -LiteralPath $bindingTemps[0].FullName -Destination $bindingCollision
    try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $bindingTempCrash.Repository -EvidenceRoot $bindingTempCrash.EvidenceRoot -Case 'binding temp deterministic orphan collision' }
    finally { Remove-Item -LiteralPath $bindingCollision -Force }

    $terminalCrash = New-PublisherCrashFixture -SourceRepository $SourceRepository -CaseRoot (Join-Path $CaseRoot 'terminal-active-crash') -Failpoint 'after-binding-committed'
    $terminalPaths = Get-PublisherFixtureStatePaths $terminalCrash
    $terminalPreparedHold = Join-Path $CaseRoot 'held-terminal-prepared-row'
    Move-Item -LiteralPath $terminalPaths.Prepared -Destination $terminalPreparedHold
    try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $terminalCrash.Repository -EvidenceRoot $terminalCrash.EvidenceRoot -Case 'BindingCommitted without BundlePrepared' }
    finally { Move-Item -LiteralPath $terminalPreparedHold -Destination $terminalPaths.Prepared }
    $terminalActive = (Get-Content -LiteralPath $terminalPaths.Active -Raw) | ConvertFrom-Json -ErrorAction Stop
    $extraClosed = Join-Path (Join-Path $terminalPaths.Publication 'leases') ("closed-publication.{0}.g{1:d10}.{2}.lock" -f $terminalActive.attempt_id, [int64]$terminalActive.generation, $terminalActive.lease_sha256)
    Copy-Item -LiteralPath $terminalPaths.Active -Destination $extraClosed
    try { $null = Invoke-PublisherRejectedWithoutMutation -Repository $terminalCrash.Repository -EvidenceRoot $terminalCrash.EvidenceRoot -Case 'terminal active lease plus extra closed lease' }
    finally { Remove-Item -LiteralPath $extraClosed -Force }
}

function Assert-PublisherChainedFailpointRecovery {
    param(
        [Parameter(Mandatory)][string]$SourceRepository,
        [Parameter(Mandatory)][string]$CaseRoot
    )
    $null = New-Item -ItemType Directory -Path $CaseRoot -Force

    $row2Fixture = New-PublisherCrashFixture -SourceRepository $SourceRepository -CaseRoot (Join-Path $CaseRoot 'binding-committed-temp') -Failpoint 'after-binding-create'
    $row2Failure = Complete-Publisher -Running (Start-Publisher -Repository $row2Fixture.Repository -EvidenceRoot $row2Fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_TEST_FAILPOINT = 'after-row-temp-write'
    })
    Assert-InjectedPublisherFailpoint -Invocation $row2Failure -Failpoint 'after-row-temp-write'
    $row2Paths = Get-PublisherFixtureStatePaths $row2Fixture
    $row2Temps = @(Get-ChildItem -LiteralPath (Join-Path $row2Paths.Publication 'journal/tmp') -File)
    Assert-Contract ($row2Temps.Count -eq 1 -and $row2Temps[0].Name -match '\.00000000000000000002\.binding-committed\.') 'seq2 failpoint did not leave the expected BindingCommitted temp'
    $row2Handoff = Convert-Handoff -Invocation (Invoke-Publisher -Repository $row2Fixture.Repository -EvidenceRoot $row2Fixture.EvidenceRoot)
    Assert-Contract ($row2Handoff.publication_tail_sha256 -match '^[0-9a-f]{64}$') 'seq2 row-temp recovery did not converge'
    $row2Paths = Get-PublisherFixtureStatePaths $row2Fixture
    $row2Orphans = @(Get-ChildItem -LiteralPath (Join-Path $row2Paths.Publication 'journal/orphans') -File)
    Assert-Contract ($row2Orphans.Count -eq 1 -and $row2Orphans[0].Name -match '\.00000000000000000002\.binding-committed\.') 'seq2 row-temp recovery did not preserve one exact orphan'
    $row2Archives = @(Get-ChildItem -LiteralPath (Join-Path $row2Paths.Publication 'leases/archives') -File)
    Assert-Contract ($row2Archives.Count -ge 2) 'seq2 row-temp recovery did not preserve its contiguous takeover generations'

    $leaseFixture = New-PublisherCrashFixture -SourceRepository $SourceRepository -CaseRoot (Join-Path $CaseRoot 'prepared-lease-generation') -Failpoint 'after-binding-create'
    $leaseFailure = Complete-Publisher -Running (Start-Publisher -Repository $leaseFixture.Repository -EvidenceRoot $leaseFixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_TEST_FAILPOINT = 'after-lease-create'
    })
    Assert-InjectedPublisherFailpoint -Invocation $leaseFailure -Failpoint 'after-lease-create'
    $leaseHandoff = Convert-Handoff -Invocation (Invoke-Publisher -Repository $leaseFixture.Repository -EvidenceRoot $leaseFixture.EvidenceRoot)
    Assert-Contract ($leaseHandoff.publication_tail_sha256 -match '^[0-9a-f]{64}$') 'prepared-row lease-generation recovery did not converge'
    $leasePaths = Get-PublisherFixtureStatePaths $leaseFixture
    $archives = @(Get-ChildItem -LiteralPath (Join-Path $leasePaths.Publication 'leases/archives') -File | Sort-Object Name)
    Assert-Contract ($archives.Count -ge 2) 'prepared-row lease-generation recovery did not create the expected archive chain'
    $prior = '0' * 64
    foreach ($archive in $archives) {
        $value = (Get-Content -LiteralPath $archive.FullName -Raw) | ConvertFrom-Json -ErrorAction Stop
        Assert-Contract ($value.prior_lease_sha256 -ceq $prior) 'prepared-row lease-generation archive prior link is broken'
        if ([int64]$value.generation -gt 1) { Assert-Contract ($value.expected_tail_sha256 -match '^[0-9a-f]{64}$' -and $value.expected_tail_sha256 -cne ('0' * 64)) 'recovery generation did not bind the prepared tail' }
        $prior = $value.lease_sha256
    }
}

function Assert-PublisherDriftBarriers {
    param(
        [Parameter(Mandatory)][string]$SourceRepository,
        [Parameter(Mandatory)][string]$CaseRoot
    )
    $null = New-Item -ItemType Directory -Path $CaseRoot -Force
    $fixture = New-PublisherFixture -SourceRepository $SourceRepository -SuiteRoot $CaseRoot
    $barrierDirectory = Join-Path $fixture.EvidenceRoot '.publisher-test-barriers'
    $common = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()

    $preLeaseEntered = Join-Path $barrierDirectory 'before-publication-lease.entered'
    $preLeaseRelease = Join-Path $barrierDirectory 'before-publication-lease.release'
    $preLeasePayloadPath = Join-Path $fixture.Repository 'docs/superpowers/plans/2026-07-13-wave0-bootstrap.md'
    $preLeasePayloadBytes = [System.IO.File]::ReadAllBytes($preLeasePayloadPath)
    $preLeaseSource = Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'before-publication-lease'
    }
    Wait-PublisherBarrier -Running $preLeaseSource -EnteredPath $preLeaseEntered -Case 'pre-lease source drift'
    [System.IO.File]::WriteAllBytes($preLeasePayloadPath, $preLeasePayloadBytes + [System.Text.UTF8Encoding]::new($false).GetBytes("`n# pre-lease-source-drift`n"))
    $null = Complete-RunningPublisherRejectedWithoutMutation -Running $preLeaseSource -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -ReleasePath $preLeaseRelease -Case 'source drift while waiting for publication lease admission'
    [System.IO.File]::WriteAllBytes($preLeasePayloadPath, $preLeasePayloadBytes)

    foreach ($leaf in @($preLeaseEntered, $preLeaseRelease)) { if (Test-Path -LiteralPath $leaf) { Remove-Item -LiteralPath $leaf -Force } }
    $preLeaseTree = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('rev-parse', "$($fixture.ExecutionBaseline)^{tree}")).Output[-1].Trim()
    $preLeaseHead = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('commit-tree', $preLeaseTree, '-p', $AuditBaseline, '-m', 'fixture pre-lease head drift')).Output[-1].Trim()
    $preLeaseHeadRun = Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'before-publication-lease'
    }
    Wait-PublisherBarrier -Running $preLeaseHeadRun -EnteredPath $preLeaseEntered -Case 'pre-lease HEAD drift'
    $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('update-ref', 'HEAD', $preLeaseHead, $fixture.ExecutionBaseline)
    try {
        $null = Complete-RunningPublisherRejectedWithoutMutation -Running $preLeaseHeadRun -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -ReleasePath $preLeaseRelease -Case 'HEAD drift while waiting for publication lease admission'
    }
    finally { $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('update-ref', 'HEAD', $fixture.ExecutionBaseline, $preLeaseHead) -AllowFailure }

    foreach ($leaf in @($preLeaseEntered, $preLeaseRelease)) { if (Test-Path -LiteralPath $leaf) { Remove-Item -LiteralPath $leaf -Force } }
    $preLeaseAclRun = Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'before-publication-lease'
    }
    Wait-PublisherBarrier -Running $preLeaseAclRun -EnteredPath $preLeaseEntered -Case 'pre-lease ACL drift'
    $preLeaseAcl = Add-ContractAclDrift -Path $fixture.EvidenceRoot -Kind Directory
    try {
        $null = Complete-RunningPublisherRejectedWithoutMutation -Running $preLeaseAclRun -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -ReleasePath $preLeaseRelease -Case 'ACL drift while waiting for publication lease admission'
    }
    finally { Restore-ContractAcl $preLeaseAcl }

    foreach ($leaf in @($preLeaseEntered, $preLeaseRelease)) { if (Test-Path -LiteralPath $leaf) { Remove-Item -LiteralPath $leaf -Force } }
    $preLeaseControlAclRun = Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'before-publication-lease'
    }
    Wait-PublisherBarrier -Running $preLeaseControlAclRun -EnteredPath $preLeaseEntered -Case 'pre-lease control-root ACL drift'
    $preLeaseControlAcl = Add-ContractAclDrift -Path (Join-Path $common 'dynamo-remediation') -Kind Directory
    try {
        $null = Complete-RunningPublisherRejectedWithoutMutation -Running $preLeaseControlAclRun -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -ReleasePath $preLeaseRelease -Case 'control-root ACL drift while waiting for publication lease admission'
    }
    finally { Restore-ContractAcl $preLeaseControlAcl }
    Assert-Contract (@(Get-ChildItem -LiteralPath (Join-Path $common 'dynamo-remediation') -Filter 'active-publication.lock' -File -Recurse -ErrorAction SilentlyContinue).Count -eq 0) 'pre-lease admission drift created or archived a lease'

    $afterSourceRoot = Join-Path $CaseRoot 'after-source-cases'
    $null = New-Item -ItemType Directory -Path $afterSourceRoot -Force
    $fixture = New-PublisherFixture -SourceRepository $SourceRepository -SuiteRoot $afterSourceRoot
    $barrierDirectory = Join-Path $fixture.EvidenceRoot '.publisher-test-barriers'
    $common = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()

    $sourceAclEntered = Join-Path $barrierDirectory 'after-source-snapshot.entered'
    $sourceAclRelease = Join-Path $barrierDirectory 'after-source-snapshot.release'
    $sourceAclRun = Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'after-source-snapshot'
    }
    Wait-PublisherBarrier -Running $sourceAclRun -EnteredPath $sourceAclEntered -Case 'after-source evidence-root ACL drift'
    $sourceAclState = Add-ContractAclDrift -Path $fixture.EvidenceRoot -Kind Directory
    try {
        $null = Complete-RunningPublisherRejectedWithoutMutation -Running $sourceAclRun -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -ReleasePath $sourceAclRelease -Case 'evidence-root ACL drift after source snapshot'
    }
    finally { Restore-ContractAcl $sourceAclState }
    foreach ($leaf in @($sourceAclEntered, $sourceAclRelease)) { if (Test-Path -LiteralPath $leaf) { Remove-Item -LiteralPath $leaf -Force } }

    $controlPath = Join-Path $fixture.Repository 'scripts/remediation/update-integration-ref.ps1'
    $controlBytes = [System.IO.File]::ReadAllBytes($controlPath)
    $entered = Join-Path $barrierDirectory 'after-source-snapshot.entered'
    $release = Join-Path $barrierDirectory 'after-source-snapshot.release'
    $controlDrift = Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'after-source-snapshot'
    }
    Wait-PublisherBarrier -Running $controlDrift -EnteredPath $entered -Case 'tracked-control drift'
    [System.IO.File]::WriteAllBytes($controlPath, $controlBytes + [System.Text.UTF8Encoding]::new($false).GetBytes("`n# tracked-control-drift`n"))
    $null = Complete-RunningPublisherRejectedWithoutMutation -Running $controlDrift -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -ReleasePath $release -Case 'tracked control drift after source snapshot'
    [System.IO.File]::WriteAllBytes($controlPath, $controlBytes)
    Assert-Contract (-not (Test-Path -LiteralPath (Join-Path $common 'dynamo-remediation'))) 'tracked-control drift mutated common-dir publication state'

    foreach ($leaf in @($entered, $release)) { if (Test-Path -LiteralPath $leaf) { Remove-Item -LiteralPath $leaf -Force } }
    $identityBefore = Get-ContractFileIdentity -Path $controlPath
    $identitySwap = Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'after-source-snapshot'
    }
    Wait-PublisherBarrier -Running $identitySwap -EnteredPath $entered -Case 'byte-identical control identity swap'
    $replacementPath = "$controlPath.contract-replacement"
    [System.IO.File]::WriteAllBytes($replacementPath, $controlBytes)
    [System.IO.File]::Move($replacementPath, $controlPath, $true)
    Assert-Contract ((Get-ContractFileIdentity -Path $controlPath) -cne $identityBefore) 'byte-identical control replacement did not change native file identity'
    $null = Complete-RunningPublisherRejectedWithoutMutation -Running $identitySwap -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -ReleasePath $release -Case 'byte-identical source identity drift'
    Assert-Contract (-not (Test-Path -LiteralPath (Join-Path $common 'dynamo-remediation'))) 'byte-identical source identity drift mutated common-dir publication state'

    foreach ($leaf in @($entered, $release)) { if (Test-Path -LiteralPath $leaf) { Remove-Item -LiteralPath $leaf -Force } }
    $tree = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('rev-parse', "$($fixture.ExecutionBaseline)^{tree}")).Output[-1].Trim()
    $sameTreeHead = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('commit-tree', $tree, '-p', $AuditBaseline, '-m', 'fixture same-tree anchor drift')).Output[-1].Trim()
    Assert-Contract ($sameTreeHead -match '^[0-9a-f]{40}$' -and $sameTreeHead -cne $fixture.ExecutionBaseline) 'could not create same-tree HEAD drift fixture commit'
    $headDrift = Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'after-source-snapshot'
    }
    Wait-PublisherBarrier -Running $headDrift -EnteredPath $entered -Case 'same-tree HEAD anchor drift'
    $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('update-ref', 'HEAD', $sameTreeHead, $fixture.ExecutionBaseline)
    try {
        $null = Complete-RunningPublisherRejectedWithoutMutation -Running $headDrift -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -ReleasePath $release -Case 'same-tree HEAD anchor drift'
    }
    finally {
        $null = Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('update-ref', 'HEAD', $fixture.ExecutionBaseline, $sameTreeHead)
    }
    Assert-Contract (-not (Test-Path -LiteralPath (Join-Path $common 'dynamo-remediation'))) 'same-tree HEAD drift mutated common-dir publication state'

    foreach ($leaf in @($entered, $release)) { if (Test-Path -LiteralPath $leaf) { Remove-Item -LiteralPath $leaf -Force } }
    $payloadPath = Join-Path $fixture.Repository 'docs/superpowers/plans/2026-07-13-wave0-bootstrap.md'
    $payloadBytes = [System.IO.File]::ReadAllBytes($payloadPath)
    $copyEntered = Join-Path $barrierDirectory 'after-payload-copy.entered'
    $copyRelease = Join-Path $barrierDirectory 'after-payload-copy.release'
    $payloadDrift = Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'after-payload-copy'
    }
    Wait-PublisherBarrier -Running $payloadDrift -EnteredPath $copyEntered -Case 'payload-copy drift'
    [System.IO.File]::WriteAllBytes($payloadPath, $payloadBytes + [System.Text.UTF8Encoding]::new($false).GetBytes("`npayload-copy-drift`n"))
    $null = Complete-RunningPublisherRejectedWithoutMutation -Running $payloadDrift -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -ReleasePath $copyRelease -Case 'payload drift after staging copy'
    [System.IO.File]::WriteAllBytes($payloadPath, $payloadBytes)
    $executionRoot = Join-Path $fixture.EvidenceRoot "Dynamo/plan-set/$($fixture.ExecutionBaseline)"
    $publishedFinals = @(if (Test-Path -LiteralPath $executionRoot) { Get-ChildItem -LiteralPath $executionRoot -Directory | Where-Object { $_.Name -match '^[0-9a-f]{64}$' } })
    Assert-Contract ($publishedFinals.Count -eq 0) 'payload-copy drift promoted an unverified final bundle'
    $recovered = Convert-Handoff -Invocation (Invoke-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot)
    Assert-Contract ($recovered.publication_tail_sha256 -match '^[0-9a-f]{64}$') 'payload-copy drift recovery did not converge'
}

function Assert-PublisherMutationBoundaryTamper {
    param(
        [Parameter(Mandatory)][string]$SourceRepository,
        [Parameter(Mandatory)][string]$CaseRoot
    )
    $null = New-Item -ItemType Directory -Path $CaseRoot -Force
    foreach ($mode in @('tamper','delete')) {
        $modeRoot = Join-Path $CaseRoot "after-payload-$mode"
        $null = New-Item -ItemType Directory -Path $modeRoot -Force
        $fixture = New-PublisherFixture -SourceRepository $SourceRepository -SuiteRoot $modeRoot
        $barrierRoot = Join-Path $fixture.EvidenceRoot '.publisher-test-barriers'
        $entered = Join-Path $barrierRoot 'after-payload-copy.entered'
        $release = Join-Path $barrierRoot 'after-payload-copy.release'
        $running = Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
            DYNAMO_REMEDIATION_TEST_MODE = '1'
            DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'after-payload-copy'
        }
        Wait-PublisherBarrier -Running $running -EnteredPath $entered -Case "after-payload active lease $mode"
        $common = (Invoke-Git -WorkingDirectory $fixture.Repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()
        $activeFiles = @(Get-ChildItem -LiteralPath (Join-Path $common 'dynamo-remediation') -Filter 'active-publication.lock' -File -Recurse)
        Assert-Contract ($activeFiles.Count -eq 1) "after-payload $mode fixture did not expose exactly one active lease"
        $activePath = $activeFiles[0].FullName
        $activeBytes = [System.IO.File]::ReadAllBytes($activePath)
        if ($mode -ceq 'tamper') { [System.IO.File]::WriteAllBytes($activePath, $activeBytes + [byte]10) }
        else { Remove-Item -LiteralPath $activePath -Force }
        try {
            $null = Complete-RunningPublisherRejectedWithoutMutation -Running $running -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -ReleasePath $release -Case "active lease $mode during after-payload-copy barrier"
        }
        finally {
            [System.IO.File]::WriteAllBytes($activePath, $activeBytes)
        }
    }

    foreach ($target in @('staged-file','ancestor-acl')) {
        $targetRoot = Join-Path $CaseRoot "after-payload-$target"
        $null = New-Item -ItemType Directory -Path $targetRoot -Force
        $fixture = New-PublisherFixture -SourceRepository $SourceRepository -SuiteRoot $targetRoot
        $barrierRoot = Join-Path $fixture.EvidenceRoot '.publisher-test-barriers'
        $entered = Join-Path $barrierRoot 'after-payload-copy.entered'
        $release = Join-Path $barrierRoot 'after-payload-copy.release'
        $running = Start-Publisher -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -Environment @{
            DYNAMO_REMEDIATION_TEST_MODE = '1'
            DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'after-payload-copy'
        }
        Wait-PublisherBarrier -Running $running -EnteredPath $entered -Case "after-payload $target drift"
        $restoreAcl = $null
        $restorePath = $null
        $restoreBytes = $null
        if ($target -ceq 'staged-file') {
            $stagingDirectories = @(Get-ChildItem -LiteralPath (Join-Path $fixture.EvidenceRoot 'Dynamo') -Directory -Filter '.staging-*' -Recurse)
            Assert-Contract ($stagingDirectories.Count -eq 1) 'staged-file drift fixture did not find exactly one staging directory'
            $restorePath = @(Get-ChildItem -LiteralPath $stagingDirectories[0].FullName -File -Recurse)[0].FullName
            $restoreBytes = [System.IO.File]::ReadAllBytes($restorePath)
            [System.IO.File]::WriteAllBytes($restorePath, $restoreBytes + [byte]10)
        }
        else {
            $restoreAcl = Add-ContractAclDrift -Path (Join-Path $fixture.EvidenceRoot 'Dynamo/plan-set') -Kind Directory
        }
        try {
            $null = Complete-RunningPublisherRejectedWithoutMutation -Running $running -Repository $fixture.Repository -EvidenceRoot $fixture.EvidenceRoot -ReleasePath $release -Case "$target drift during after-payload-copy barrier"
        }
        finally {
            if ($null -ne $restoreBytes) { [System.IO.File]::WriteAllBytes($restorePath, $restoreBytes) }
            if ($null -ne $restoreAcl) { Restore-ContractAcl $restoreAcl }
        }
    }

    $terminalRoot = Join-Path $CaseRoot 'before-complete-terminal-tamper'
    $null = New-Item -ItemType Directory -Path $terminalRoot -Force
    $terminalFixture = New-PublisherFixture -SourceRepository $SourceRepository -SuiteRoot $terminalRoot
    foreach ($target in @('binding','row','closed-lease')) {
        $barrierRoot = Join-Path $terminalFixture.EvidenceRoot '.publisher-test-barriers'
        $entered = Join-Path $barrierRoot 'before-publication-complete.entered'
        $release = Join-Path $barrierRoot 'before-publication-complete.release'
        foreach ($leaf in @($entered,$release)) { if (Test-Path -LiteralPath $leaf) { Remove-Item -LiteralPath $leaf -Force } }
        $running = Start-Publisher -Repository $terminalFixture.Repository -EvidenceRoot $terminalFixture.EvidenceRoot -Environment @{
            DYNAMO_REMEDIATION_TEST_MODE = '1'
            DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'before-publication-complete'
        }
        Wait-PublisherBarrier -Running $running -EnteredPath $entered -Case "terminal $target tamper"
        $common = (Invoke-Git -WorkingDirectory $terminalFixture.Repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()
        $control = Join-Path $common 'dynamo-remediation'
        $targetPath = switch ($target) {
            'binding' { Join-Path $control 'plan-set-binding-v1.json' }
            'row' { @(Get-ChildItem -LiteralPath $control -Filter '00000000000000000002-binding-committed.json' -File -Recurse)[0].FullName }
            'closed-lease' { @(Get-ChildItem -LiteralPath $control -Filter 'closed-publication.*.lock' -File -Recurse)[0].FullName }
        }
        Assert-Contract (Test-Path -LiteralPath $targetPath -PathType Leaf) "terminal $target fixture artifact is missing"
        $bytes = [System.IO.File]::ReadAllBytes($targetPath)
        [System.IO.File]::WriteAllBytes($targetPath, $bytes + [byte]10)
        try {
            $null = Complete-RunningPublisherRejectedWithoutMutation -Running $running -Repository $terminalFixture.Repository -EvidenceRoot $terminalFixture.EvidenceRoot -ReleasePath $release -Case "terminal $target tamper during final barrier"
        }
        finally { [System.IO.File]::WriteAllBytes($targetPath, $bytes) }
        $reused = Convert-Handoff -Invocation (Invoke-Publisher -Repository $terminalFixture.Repository -EvidenceRoot $terminalFixture.EvidenceRoot)
        Assert-Contract ($reused.publication_tail_sha256 -match '^[0-9a-f]{64}$') "terminal $target restoration did not preserve reusable completion"
    }
}

function Assert-PublisherContract {
    param([Parameter(Mandatory)]$Fixture)

    Assert-Contract ((Get-DomainHash -Domain 'dynamo-plan-set-v1' -CanonicalJson "{`"schema_version`":1,`"value`":`"fixture`"}`n") -ceq 'de815388a99ffa87fe263748fb96f92ab0e6248bf762a2b4e07529c37bdaacf4') 'canonical plan-set known vector mismatch'
    $scriptPath = Join-Path $Fixture.Repository 'scripts/remediation/publish-plan-set.ps1'
    $tokens = $null
    $parseErrors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$tokens, [ref]$parseErrors)
    Assert-Contract ($parseErrors.Count -eq 0) 'publisher implementation does not parse'
    Assert-Contract ($null -eq $ast.ParamBlock -or $ast.ParamBlock.Parameters.Count -eq 0) 'publisher must expose zero public parameters'
    Assert-PublisherBatchBlobDuplex -PublisherPath $scriptPath -CaseRoot (Join-Path (Split-Path -Parent $Fixture.Repository) 'publisher-batch-blob-duplex')

    $realCommon = (Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('rev-parse', '--path-format=absolute', '--git-common-dir')).Output[-1].Trim()
    $fixedControlRoot = Join-Path $realCommon 'dynamo-remediation'

    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'unguarded known publisher barrier' -Environment @{
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'after-source-snapshot'
    }
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'unknown guarded publisher barrier' -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'not-a-publisher-boundary'
    }

    $preflightBefore = Get-TreeFingerprint -Root $fixedControlRoot
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot 'relative-evidence' -Case 'relative evidence root'
    Assert-Contract (($preflightBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $fixedControlRoot) -join "`n")) 'relative-root rejection mutated control state'

    $nonexistentRoot = Join-Path (Split-Path -Parent $Fixture.EvidenceRoot) "nonexistent-evidence-$([guid]::NewGuid().ToString('N'))"
    Assert-Contract (-not (Test-Path -LiteralPath $nonexistentRoot)) 'nonexistent evidence-root fixture unexpectedly exists'
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $nonexistentRoot -Case 'nonexistent evidence root'
    Assert-Contract (($preflightBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $fixedControlRoot) -join "`n")) 'nonexistent-root rejection mutated control state'

    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.LinkedWorktree -Case 'evidence root equal to a worktree'
    Assert-Contract (($preflightBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $fixedControlRoot) -join "`n")) 'equal-worktree rejection mutated control state'

    $untrackedPath = Join-Path $Fixture.Repository 'publisher-preflight-untracked.tmp'
    try {
        [System.IO.File]::WriteAllText($untrackedPath, 'untracked', [System.Text.UTF8Encoding]::new($false))
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'untracked worktree leaf'
        Assert-Contract (($preflightBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $fixedControlRoot) -join "`n")) 'dirty-worktree rejection mutated control state'
    }
    finally {
        if (Test-Path -LiteralPath $untrackedPath) { Remove-Item -LiteralPath $untrackedPath -Force }
    }

    $containedRoot = Join-Path $Fixture.LinkedWorktree 'evidence-inside-worktree'
    $null = New-Item -ItemType Directory -Path $containedRoot
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $containedRoot -Case 'worktree-contained evidence root'
    Assert-Contract (($preflightBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $fixedControlRoot) -join "`n")) 'contained-root rejection mutated control state'

    $reparseTarget = Join-Path (Split-Path -Parent $Fixture.EvidenceRoot) 'reparse-target'
    $reparseRoot = Join-Path (Split-Path -Parent $Fixture.EvidenceRoot) 'reparse-evidence-root'
    $null = New-Item -ItemType Directory -Path $reparseTarget
    try {
        if ($IsWindows) { $null = New-Item -ItemType Junction -Path $reparseRoot -Target $reparseTarget }
        else { $null = New-Item -ItemType SymbolicLink -Path $reparseRoot -Target $reparseTarget }
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $reparseRoot -Case 'reparse evidence root'
        Assert-Contract (($preflightBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $fixedControlRoot) -join "`n")) 'reparse-root rejection mutated control state'
    }
    finally {
        if (Test-Path -LiteralPath $reparseRoot) { Remove-Item -LiteralPath $reparseRoot -Force }
    }

    $prunable = Join-Path (Split-Path -Parent $Fixture.Repository) 'prunable-worktree'
    $null = Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('worktree', 'add', '--quiet', '--detach', $prunable, 'HEAD')
    Assert-PathUnderRoot -Path $prunable -Root (Split-Path -Parent $Fixture.Repository) -Message 'prunable fixture escaped suite root'
    [System.IO.Directory]::Delete($prunable, $true)
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'missing/prunable registered worktree'
    Assert-Contract (($preflightBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $fixedControlRoot) -join "`n")) 'prunable-worktree rejection mutated control state'
    $null = Invoke-Git -WorkingDirectory $Fixture.Repository -Arguments @('worktree', 'prune', '--expire', 'now')

    $missingName = $PlanNames[-1]
    $missingPath = Join-Path $Fixture.Repository "docs/superpowers/plans/$missingName"
    $missingBytes = [System.IO.File]::ReadAllBytes($missingPath)
    Remove-Item -LiteralPath $missingPath -Force
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'missing reviewed source'
    Assert-Contract (($preflightBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $fixedControlRoot) -join "`n")) 'missing-source rejection mutated control state'
    [System.IO.File]::WriteAllBytes($missingPath, $missingBytes)

    $extraPath = Join-Path $Fixture.Repository 'docs/superpowers/plans/2026-07-12-unreviewed-extra.md'
    [System.IO.File]::WriteAllText($extraPath, "unreviewed`n", [System.Text.UTF8Encoding]::new($false))
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'extra reviewed-pattern source'
    Assert-Contract (($preflightBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $fixedControlRoot) -join "`n")) 'extra-source rejection mutated control state'
    Remove-Item -LiteralPath $extraPath -Force

    $driftPlan = Join-Path $Fixture.Repository 'docs/superpowers/plans/2026-07-13-wave0-bootstrap.md'
    $driftPlanBytes = [System.IO.File]::ReadAllBytes($driftPlan)
    $drift = Start-Publisher -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'after-source-snapshot'
    }
    $entered = Join-Path $Fixture.EvidenceRoot '.publisher-test-barriers/after-source-snapshot.entered'
    $release = Join-Path $Fixture.EvidenceRoot '.publisher-test-barriers/after-source-snapshot.release'
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not (Test-Path -LiteralPath $entered -PathType Leaf) -and -not $drift.Process.HasExited -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 25
    }
    if (-not (Test-Path -LiteralPath $entered -PathType Leaf) -and -not $drift.Process.HasExited) {
        $drift.Process.Kill($true)
        $drift.Process.WaitForExit()
        $null = $drift.StandardOutput.GetAwaiter().GetResult()
        $null = $drift.StandardError.GetAwaiter().GetResult()
    }
    $barrierDiagnostic = if ($drift.Process.HasExited) {
        "exit=$($drift.Process.ExitCode); out=$($drift.StandardOutput.GetAwaiter().GetResult()) | $($drift.StandardError.GetAwaiter().GetResult())"
    } else { 'child-still-running' }
    Assert-Contract (Test-Path -LiteralPath $entered -PathType Leaf) "publisher source-snapshot barrier was not reached: $barrierDiagnostic"
    [System.IO.File]::WriteAllBytes($driftPlan, $driftPlanBytes + [System.Text.UTF8Encoding]::new($false).GetBytes("`nsource-drift`n"))
    [System.IO.File]::WriteAllText($release, 'release', [System.Text.UTF8Encoding]::new($false))
    Assert-FailedWithoutOutput -Invocation (Complete-Publisher -Running $drift) -Case 'source drift after snapshot'
    [System.IO.File]::WriteAllBytes($driftPlan, $driftPlanBytes)
    $sourceDriftBarrierDirectory = Split-Path -Parent $entered
    if (Test-Path -LiteralPath $sourceDriftBarrierDirectory) { Remove-Item -LiteralPath $sourceDriftBarrierDirectory -Recurse -Force }

    $alternateEvidence = Join-Path (Split-Path -Parent $Fixture.EvidenceRoot) 'concurrent-external-evidence'
    $null = New-Item -ItemType Directory -Path $alternateEvidence
    $originalEvidence = $Fixture.EvidenceRoot
    $originalEvidenceBeforeRace = Get-TreeFingerprint -Root $originalEvidence
    $alternateEvidenceBeforeRace = Get-TreeFingerprint -Root $alternateEvidence
    $raceBarrier = 'before-publication-lease'
    $leftBarrierDirectory = Join-Path $originalEvidence '.publisher-test-barriers'
    $rightBarrierDirectory = Join-Path $alternateEvidence '.publisher-test-barriers'
    $leftEntered = Join-Path $leftBarrierDirectory "$raceBarrier.entered"
    $rightEntered = Join-Path $rightBarrierDirectory "$raceBarrier.entered"
    $leftRelease = Join-Path $leftBarrierDirectory "$raceBarrier.release"
    $rightRelease = Join-Path $rightBarrierDirectory "$raceBarrier.release"
    $raceEnvironment = @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = $raceBarrier
    }
    $leftPublisher = Start-Publisher -Repository $Fixture.Repository -EvidenceRoot $originalEvidence -Environment $raceEnvironment
    $rightPublisher = Start-Publisher -Repository $Fixture.LinkedWorktree -EvidenceRoot $alternateEvidence -Environment $raceEnvironment
    Wait-PublisherBarrier -Running $leftPublisher -EnteredPath $leftEntered -Case 'fresh different-root race left contender'
    Wait-PublisherBarrier -Running $rightPublisher -EnteredPath $rightEntered -Case 'fresh different-root race right contender'
    [System.IO.File]::WriteAllText($leftRelease, 'release', [System.Text.UTF8Encoding]::new($false))
    [System.IO.File]::WriteAllText($rightRelease, 'release', [System.Text.UTF8Encoding]::new($false))
    $leftResult = Complete-Publisher -Running $leftPublisher
    $rightResult = Complete-Publisher -Running $rightPublisher
    foreach ($barrierDirectory in @($leftBarrierDirectory, $rightBarrierDirectory)) {
        if (Test-Path -LiteralPath $barrierDirectory) { Remove-Item -LiteralPath $barrierDirectory -Recurse -Force }
    }
    $publisherWinners = @(@($leftResult, $rightResult) | Where-Object { $_.ExitCode -eq 0 })
    Assert-Contract ($publisherWinners.Count -eq 1) "concurrent different-root publishers did not produce exactly one winner; left=$($leftResult.ExitCode):$($leftResult.Output -join ' | '); right=$($rightResult.ExitCode):$($rightResult.Output -join ' | ')"
    $result = $publisherWinners[0]
    $Fixture.EvidenceRoot = if ($leftResult.ExitCode -eq 0) { $originalEvidence } else { $alternateEvidence }
    $losingEvidence = if ($leftResult.ExitCode -eq 0) { $alternateEvidence } else { $originalEvidence }
    $losingEvidenceBefore = if ($leftResult.ExitCode -eq 0) { $alternateEvidenceBeforeRace } else { $originalEvidenceBeforeRace }
    Assert-Contract (($losingEvidenceBefore -join "`n") -ceq ((Get-TreeFingerprint -Root $losingEvidence) -join "`n")) 'different-root race loser mutated its external evidence root'
    $handoff = Convert-Handoff -Invocation $result

    $handoffKeys = @('execution_baseline', 'plan_set_sha256', 'manifest_sha256', 'binding_sha256', 'publication_tail_sha256', 'git_common_dir_identity_sha256')
    Assert-Contract ((@($handoff.PSObject.Properties.Name) -join ',') -ceq ($handoffKeys -join ',')) 'handoff key set/order is not canonical'
    Assert-Contract ($handoff.execution_baseline -ceq $Fixture.ExecutionBaseline) 'handoff execution baseline mismatch'
    Assert-Contract ($handoff.plan_set_sha256 -match '^[0-9a-f]{64}$') 'handoff plan-set hash is malformed'
    Assert-Contract ($handoff.manifest_sha256 -match '^[0-9a-f]{64}$') 'handoff manifest hash is malformed'
    Assert-Contract ($handoff.binding_sha256 -match '^[0-9a-f]{64}$') 'handoff binding hash is malformed'
    Assert-Contract ($handoff.publication_tail_sha256 -match '^[0-9a-f]{64}$') 'handoff publication-tail hash is malformed'
    Assert-Contract ($handoff.git_common_dir_identity_sha256 -match '^[0-9a-f]{64}$') 'handoff common-directory identity hash is malformed'
    $handoffText = $result.Output[0].ToString()
    Assert-Contract ($handoffText -notmatch '(?:[A-Za-z]:[\\/]|(?:^|["''])(?:/|\\\\)|\.\.[\\/])') 'handoff contains a path-like value'

    $bundleRoot = Join-Path $Fixture.EvidenceRoot "Dynamo/plan-set/$($Fixture.ExecutionBaseline)/$($handoff.plan_set_sha256)"
    Assert-PathUnderRoot -Path $bundleRoot -Root $Fixture.EvidenceRoot -Message 'derived bundle escaped the external evidence root'
    Assert-Contract (Test-Path -LiteralPath $bundleRoot -PathType Container) 'published bundle is missing'
    $manifestMatches = @(
        Get-ChildItem -LiteralPath $bundleRoot -File -Recurse |
            Where-Object { (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() -ceq $handoff.manifest_sha256 }
    )
    Assert-Contract ($manifestMatches.Count -eq 1) 'derived bundle does not contain exactly one manifest-hash match'
    $manifestPath = $manifestMatches[0].FullName
    Assert-Contract ([System.IO.Path]::GetFullPath($manifestPath) -ceq [System.IO.Path]::GetFullPath((Join-Path $bundleRoot 'plan-set-manifest-v1.json'))) 'manifest leaf name/path is not fixed'
    $bindingPath = Join-Path $realCommon 'dynamo-remediation/plan-set-binding-v1.json'
    Assert-Contract (Test-Path -LiteralPath $bindingPath -PathType Leaf) 'Git-common-dir binding is missing'

    $manifestBytes = [System.IO.File]::ReadAllBytes($manifestPath)
    $bindingBytes = [System.IO.File]::ReadAllBytes($bindingPath)
    $manifestHash = (Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
    Assert-Contract ($manifestHash -ceq $handoff.manifest_sha256) 'manifest handoff hash mismatch'
    $manifestText = [System.Text.UTF8Encoding]::new($false, $true).GetString($manifestBytes)
    Assert-Contract (-not $manifestText.Contains("`r")) 'manifest is not LF-only'
    Assert-Contract ($manifestText.EndsWith("`n", [System.StringComparison]::Ordinal) -and -not $manifestText.Substring(0, $manifestText.Length - 1).Contains("`n")) 'manifest must have exactly one terminal LF'
    $manifest = $manifestText | ConvertFrom-Json -ErrorAction Stop
    $manifestKeys = @('schema_version', 'plan_set_sha256', 'audit_baseline', 'execution_baseline', 'git_common_dir_identity_sha256', 'payloads', 'controls', 'gitignore_evidence', 'published_at')
    Assert-Contract ((@($manifest.PSObject.Properties.Name) -join ',') -ceq ($manifestKeys -join ',')) 'manifest key set/order is not canonical'
    Assert-Contract ($manifest.plan_set_sha256 -ceq $handoff.plan_set_sha256) 'manifest plan-set hash mismatch'
    $payloadPaths = @($manifest.payloads | ForEach-Object { $_.path })
    $controlPaths = @($manifest.controls | ForEach-Object { $_.path })
    $expectedPayloadPaths = @($PlanNames | ForEach-Object { "docs/superpowers/plans/$_" } | Sort-Object -CaseSensitive)
    $expectedControlPaths = @($BootstrapPaths | Sort-Object -CaseSensitive)
    Assert-Contract (($payloadPaths -join "`n") -ceq ($expectedPayloadPaths -join "`n")) 'manifest payload set/order mismatch'
    Assert-Contract (($controlPaths -join "`n") -ceq ($expectedControlPaths -join "`n")) 'manifest control set/order mismatch'
    foreach ($row in @($manifest.payloads) + @($manifest.controls)) {
        Assert-Contract ((@($row.PSObject.Properties.Name) -join ',') -ceq 'path,bytes,sha256') "manifest core row key order mismatch: $($row.path)"
        Assert-Contract ($row.bytes -is [long] -or $row.bytes -is [int]) "manifest byte count is not an integer: $($row.path)"
        Assert-Contract ($row.sha256 -match '^[0-9a-f]{64}$') "manifest row hash is malformed: $($row.path)"
    }
    $gitignoreRows = @($manifest.gitignore_evidence)
    Assert-Contract ($gitignoreRows.Count -eq 5) 'gitignore_evidence must contain exactly five payload rows'
    Assert-Contract ((@($gitignoreRows | ForEach-Object { $_.path }) -join "`n") -ceq ($expectedPayloadPaths -join "`n")) 'gitignore_evidence path set/order mismatch'
    foreach ($row in $gitignoreRows) {
        Assert-Contract ((@($row.PSObject.Properties.Name) -join ',') -ceq 'path,rule_source,rule_line,pattern') "gitignore_evidence row schema mismatch: $($row.path)"
        Assert-Contract ($row.rule_source -match '^(?![A-Za-z]:|/|\\)[^\\]+(?:/[^\\]+)*$') "gitignore rule source is not repository-relative: $($row.path)"
        Assert-Contract ($row.rule_line -is [int] -or $row.rule_line -is [long]) "gitignore line is not an integer: $($row.path)"
        Assert-Contract (-not [string]::IsNullOrWhiteSpace($row.pattern)) "gitignore pattern is empty: $($row.path)"
    }
    Assert-Contract ($manifestText -match '"published_at":"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}Z"') 'manifest published_at is noncanonical'
    $canonicalCore = [ordered]@{
        schema_version = $manifest.schema_version
        audit_baseline = $manifest.audit_baseline
        execution_baseline = $manifest.execution_baseline
        git_common_dir_identity_sha256 = $manifest.git_common_dir_identity_sha256
        payloads = @($manifest.payloads)
        controls = @($manifest.controls)
    } | ConvertTo-Json -Compress -Depth 20
    Assert-Contract ((Get-DomainHash -Domain 'dynamo-plan-set-v1' -CanonicalJson ($canonicalCore + "`n")) -ceq $handoff.plan_set_sha256) 'deterministic canonical-core plan-set hash mismatch'
    $bundleLeaves = @(Get-ChildItem -LiteralPath $bundleRoot -File -Recurse)
    Assert-Contract ($bundleLeaves.Count -eq 6) 'final bundle must contain exactly five payloads plus one manifest'
    foreach ($row in @($manifest.payloads)) {
        $leaf = Join-Path $bundleRoot $row.path
        Assert-Contract (Test-Path -LiteralPath $leaf -PathType Leaf) "published payload is missing: $($row.path)"
        Assert-Contract ((Get-FileHash -LiteralPath $leaf -Algorithm SHA256).Hash.ToLowerInvariant() -ceq $row.sha256) "published payload hash mismatch: $($row.path)"
        Assert-Contract ((Get-Item -LiteralPath $leaf).Length -eq [long]$row.bytes) "published payload byte count mismatch: $($row.path)"
        Assert-Contract ([System.Convert]::ToBase64String([System.IO.File]::ReadAllBytes($leaf)) -ceq [System.Convert]::ToBase64String([System.IO.File]::ReadAllBytes((Join-Path $Fixture.Repository $row.path)))) "published payload bytes mismatch: $($row.path)"
    }
    foreach ($row in @($manifest.controls)) {
        $control = Join-Path $Fixture.Repository $row.path
        Assert-Contract ((Get-FileHash -LiteralPath $control -Algorithm SHA256).Hash.ToLowerInvariant() -ceq $row.sha256) "control hash mismatch: $($row.path)"
        Assert-Contract ((Get-Item -LiteralPath $control).Length -eq [long]$row.bytes) "control byte count mismatch: $($row.path)"
    }
    foreach ($name in $PlanNames) {
        Assert-Contract ($manifestText.Contains($name, [System.StringComparison]::Ordinal)) "manifest does not bind reviewed source $name"
    }
    Assert-Contract ($manifestText.Contains($AuditBaseline, [System.StringComparison]::Ordinal)) 'manifest does not bind the fixed audit baseline'
    Assert-Contract ($manifestText.Contains($Fixture.ExecutionBaseline, [System.StringComparison]::Ordinal)) 'manifest does not bind the execution baseline'

    $secretShape = '(?i)(mongodb(?:\+srv)?://|authorization\s*[:=]\s*bearer|client_secret|access_token|refresh_token|cookie\s*[:=]|remote(?:_url)?\s*[:=])'
    Assert-Contract ($manifestText -notmatch $secretShape) 'manifest contains remote/environment/credential-shaped material'
    $bindingText = [System.Text.UTF8Encoding]::new($false, $true).GetString($bindingBytes)
    Assert-Contract ($bindingText -notmatch $secretShape) 'binding contains remote/environment/credential-shaped material'
    $bindingObject = $bindingText | ConvertFrom-Json -ErrorAction Stop
    $bindingKeys = @('schema_version', 'audit_baseline', 'execution_baseline', 'plan_set_sha256', 'manifest_native_path', 'manifest_sha256', 'manifest_bytes', 'git_common_dir_native_path', 'git_common_dir_identity_sha256', 'git_common_dir_owner', 'git_common_dir_acl_sha256', 'control_schema_path', 'control_schema_sha256', 'control_schema_version', 'control_hashes', 'bundle_prepared_row_sha256', 'binding_sha256')
    Assert-Contract ((@($bindingObject.PSObject.Properties.Name) -join ',') -ceq ($bindingKeys -join ',')) 'binding key set/order mismatch'
    Assert-Contract ($bindingObject.schema_version -eq 2 -and $bindingObject.control_schema_version -eq 2 -and $bindingObject.control_schema_path -ceq 'scripts/remediation/control-schema-v2.json') 'binding v2 control schema fields mismatch'
    Assert-Contract ((@($bindingObject.control_hashes | ForEach-Object { $_.path }) -join "`n") -ceq ($expectedControlPaths -join "`n")) 'binding v2 control hashes do not exactly bind manifest controls'
    foreach ($row in @($bindingObject.control_hashes)) { Assert-Contract ((@($row.PSObject.Properties.Name) -join ',') -ceq 'path,sha256' -and $row.sha256 -match '^[0-9a-f]{64}$') "binding v2 control-hash row mismatch: $($row.path)" }
    Assert-SelfOmittingHash -Raw $bindingText -Property 'binding_sha256' -Domain 'dynamo-plan-set-binding-v1' -Expected $handoff.binding_sha256 -Case 'binding'

    $publicationRoot = Join-Path $realCommon "dynamo-remediation/publication-state-v1/$($Fixture.ExecutionBaseline)/$($handoff.plan_set_sha256)"
    Assert-Contract (Test-Path -LiteralPath $publicationRoot -PathType Container) 'fixed publication state root is missing'
    $rowsRoot = Join-Path $publicationRoot 'journal/rows'
    $rowLeaves = @(Get-ChildItem -LiteralPath $rowsRoot -File | Sort-Object Name)
    Assert-Contract (($rowLeaves.Name -join ',') -ceq '00000000000000000001-bundle-prepared.json,00000000000000000002-binding-committed.json') 'publication row leaf set is not exact'
    $preparedText = Get-Content -LiteralPath $rowLeaves[0].FullName -Raw
    $committedText = Get-Content -LiteralPath $rowLeaves[1].FullName -Raw
    Assert-Contract ($preparedText -match '"utc":"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}Z"') 'BundlePrepared utc is noncanonical in raw bytes'
    Assert-Contract ($committedText -match '"utc":"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}Z"') 'BindingCommitted utc is noncanonical in raw bytes'
    $preparedObject = $preparedText | ConvertFrom-Json -ErrorAction Stop
    $committedObject = $committedText | ConvertFrom-Json -ErrorAction Stop
    $publicationRowKeys = @('schema_version', 'seq', 'phase', 'execution_baseline', 'plan_set_sha256', 'attempt_id', 'generation', 'lease_sha256', 'evidence_root_identity_sha256', 'manifest_sha256', 'manifest_bytes', 'bundle_prepared_row_sha256', 'binding_sha256', 'utc', 'prev_row_sha256', 'row_sha256')
    Assert-Contract ((@($preparedObject.PSObject.Properties.Name) -join ',') -ceq ($publicationRowKeys -join ',')) 'BundlePrepared row schema mismatch'
    Assert-Contract ((@($committedObject.PSObject.Properties.Name) -join ',') -ceq ($publicationRowKeys -join ',')) 'BindingCommitted row schema mismatch'
    Assert-Contract ($preparedObject.seq -eq 1 -and $preparedObject.phase -ceq 'BundlePrepared') 'BundlePrepared sequence/phase mismatch'
    Assert-Contract ($committedObject.seq -eq 2 -and $committedObject.phase -ceq 'BindingCommitted') 'BindingCommitted sequence/phase mismatch'
    Assert-Contract ($preparedObject.prev_row_sha256 -ceq ('0' * 64) -and $preparedObject.bundle_prepared_row_sha256 -ceq ('0' * 64) -and $preparedObject.binding_sha256 -ceq ('0' * 64)) 'BundlePrepared zero-link invariants failed'
    Assert-Contract ($preparedObject.row_sha256 -match '^[0-9a-f]{64}$') 'BundlePrepared row hash is missing/malformed'
    Assert-Contract ($committedObject.row_sha256 -ceq $handoff.publication_tail_sha256) 'BindingCommitted row is not the handoff publication tail'
    Assert-Contract ($preparedObject.manifest_sha256 -ceq $handoff.manifest_sha256 -and $committedObject.manifest_sha256 -ceq $handoff.manifest_sha256) 'publication rows do not bind the handoff manifest'
    Assert-Contract ([long]$preparedObject.manifest_bytes -eq $manifestBytes.LongLength -and [long]$committedObject.manifest_bytes -eq $manifestBytes.LongLength) 'publication rows do not bind manifest bytes'
    Assert-Contract ($committedObject.prev_row_sha256 -ceq $preparedObject.row_sha256 -and $committedObject.bundle_prepared_row_sha256 -ceq $preparedObject.row_sha256 -and $committedObject.binding_sha256 -ceq $handoff.binding_sha256) 'BindingCommitted one-way link invariants failed'
    foreach ($property in @('execution_baseline', 'plan_set_sha256', 'attempt_id', 'evidence_root_identity_sha256', 'manifest_sha256', 'manifest_bytes')) {
        Assert-Contract ($preparedObject.$property -ceq $committedObject.$property) "publication rows disagree on $property"
    }
    Assert-SelfOmittingHash -Raw $preparedText -Property 'row_sha256' -Domain 'dynamo-publication-row-v1' -Expected $preparedObject.row_sha256 -Case 'BundlePrepared row'
    Assert-SelfOmittingHash -Raw $committedText -Property 'row_sha256' -Domain 'dynamo-publication-row-v1' -Expected $committedObject.row_sha256 -Case 'BindingCommitted row'
    Assert-Contract ($bindingText.Contains($preparedObject.row_sha256, [System.StringComparison]::Ordinal)) 'binding does not link backward to BundlePrepared'
    Assert-Contract (-not $bindingText.Contains($handoff.publication_tail_sha256, [System.StringComparison]::Ordinal)) 'binding illegally contains its future BindingCommitted hash'
    Assert-Contract ($committedText.Contains($handoff.binding_sha256, [System.StringComparison]::Ordinal)) 'BindingCommitted does not bind binding_sha256'
    Assert-Contract (-not (Test-Path -LiteralPath (Join-Path $publicationRoot 'active-publication.lock'))) 'PublicationComplete left an active publication lease'
    $closedLeases = @(Get-ChildItem -LiteralPath (Join-Path $publicationRoot 'leases') -File | Where-Object { $_.Name -match '^closed-publication\.[0-9a-f]{32}\.g\d{10}\.[0-9a-f]{64}\.lock$' })
    Assert-Contract ($closedLeases.Count -eq 1) 'PublicationComplete does not have exactly one attempt-specific closed lease'
    $leaseText = Get-Content -LiteralPath $closedLeases[0].FullName -Raw
    Assert-Contract ($leaseText -match '"created_at":"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}Z"') 'publication lease created_at is noncanonical in raw bytes'
    $lease = $leaseText | ConvertFrom-Json -ErrorAction Stop
    $leaseKeys = @('schema_version', 'execution_baseline', 'plan_set_sha256', 'attempt_id', 'generation', 'evidence_root_native_path', 'evidence_root_identity_sha256', 'owner', 'source_set_sha256', 'canonical_core_sha256', 'expected_tail_sha256', 'prior_lease_sha256', 'created_at', 'lease_sha256')
    Assert-Contract ((@($lease.PSObject.Properties.Name) -join ',') -ceq ($leaseKeys -join ',')) 'publication lease schema mismatch'
    Assert-SelfOmittingHash -Raw $leaseText -Property 'lease_sha256' -Domain 'dynamo-publication-lease-v1' -Expected $lease.lease_sha256 -Case 'publication lease'
    Assert-Contract ($lease.attempt_id -ceq $committedObject.attempt_id -and $lease.lease_sha256 -ceq $committedObject.lease_sha256) 'closed publication lease does not bind BindingCommitted'
    Assert-Contract ([long]$lease.generation -eq [long]$committedObject.generation -and $lease.evidence_root_identity_sha256 -ceq $committedObject.evidence_root_identity_sha256) 'closed publication lease generation/evidence identity does not bind BindingCommitted'
    Assert-Contract ($closedLeases[0].Name -ceq ("closed-publication.{0}.g{1:d10}.{2}.lock" -f $lease.attempt_id, [int]$lease.generation, $lease.lease_sha256)) 'closed publication lease filename mismatch'
    Assert-Contract (@(Get-ChildItem -LiteralPath (Join-Path $publicationRoot 'leases/archives') -File -ErrorAction SilentlyContinue).Count -eq 0) 'happy publication has an unexpected archived lease'
    foreach ($emptyPath in @('journal/tmp', 'journal/orphans', 'binding-tmp', 'binding-orphans')) {
        $candidate = Join-Path $publicationRoot $emptyPath
        Assert-Contract (-not (Test-Path -LiteralPath $candidate) -or @(Get-ChildItem -LiteralPath $candidate -Force).Count -eq 0) "PublicationComplete has unexpected residue in $emptyPath"
    }

    Assert-ContractAclTreeSemantics -Root (Join-Path $realCommon 'dynamo-remediation')
    Assert-ContractAclTreeSemantics -Root (Join-Path $Fixture.EvidenceRoot 'Dynamo')

    $intermediateAclPath = Join-Path $Fixture.EvidenceRoot 'Dynamo/plan-set'
    $intermediateAclState = Add-ContractAclDrift -Path $intermediateAclPath -Kind Directory
    try {
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'protected evidence intermediate-directory ACL drift'
    }
    finally { Restore-ContractAcl $intermediateAclState }

    $unknownBundleDirectory = Join-Path $bundleRoot 'unexpected-empty-directory'
    $null = New-Item -ItemType Directory -Path $unknownBundleDirectory
    $bundleAcl = [System.IO.FileSystemAclExtensions]::GetAccessControl((Get-Item -LiteralPath $bundleRoot -Force))
    [System.IO.FileSystemAclExtensions]::SetAccessControl((Get-Item -LiteralPath $unknownBundleDirectory -Force), $bundleAcl)
    try {
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'unknown empty directory in final bundle'
    }
    finally { Remove-Item -LiteralPath $unknownBundleDirectory -Force }
    Assert-CoherentNumericStringRejections -Fixture $Fixture -ManifestPath $manifestPath -ManifestText $manifestText -BindingPath $bindingPath -BindingText $bindingText -PreparedPath $rowLeaves[0].FullName -PreparedText $preparedText -CommittedPath $rowLeaves[1].FullName -CommittedText $committedText -LeasePath $closedLeases[0].FullName -LeaseText $leaseText

    $raceStable = Get-TreeFingerprint -Root $publicationRoot
    for ($iteration = 1; $iteration -lt 20; $iteration++) {
        $winnerCall = Start-Publisher -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot
        $loserCall = Start-Publisher -Repository $Fixture.LinkedWorktree -EvidenceRoot $losingEvidence
        $winnerResult = Complete-Publisher -Running $winnerCall
        $loserResult = Complete-Publisher -Running $loserCall
        Assert-Contract ($winnerResult.ExitCode -eq 0 -and $loserResult.ExitCode -ne 0) "different-root publication race $iteration did not preserve the bound winner"
        $raceHandoff = Convert-Handoff -Invocation $winnerResult
        Assert-Contract ($raceHandoff.publication_tail_sha256 -ceq $handoff.publication_tail_sha256) "different-root publication race $iteration changed the publication tail"
        Assert-FailedWithoutOutput -Invocation $loserResult -Case "different-root publication race loser $iteration"
        Assert-Contract (($raceStable -join "`n") -ceq ((Get-TreeFingerprint -Root $publicationRoot) -join "`n")) "different-root publication race $iteration mutated PublicationComplete"
    }

    $publicationBeforeReuse = Get-TreeFingerprint -Root $publicationRoot
    $second = Convert-Handoff -Invocation (Invoke-Publisher -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot)
    Assert-Contract ($second.manifest_sha256 -ceq $handoff.manifest_sha256) 'byte-identical manifest reuse changed its hash'
    Assert-Contract ($second.binding_sha256 -ceq $handoff.binding_sha256) 'byte-identical binding reuse changed its hash'
    Assert-Contract ($second.publication_tail_sha256 -ceq $handoff.publication_tail_sha256) 'PublicationComplete reuse changed its tail'
    Assert-Contract ([System.Convert]::ToBase64String($manifestBytes) -ceq [System.Convert]::ToBase64String([System.IO.File]::ReadAllBytes($manifestPath))) 'byte-identical manifest reuse changed bytes'
    Assert-Contract ([System.Convert]::ToBase64String($bindingBytes) -ceq [System.Convert]::ToBase64String([System.IO.File]::ReadAllBytes($bindingPath))) 'byte-identical binding reuse changed bytes'
    Assert-Contract (($publicationBeforeReuse -join "`n") -ceq ((Get-TreeFingerprint -Root $publicationRoot) -join "`n")) 'PublicationComplete reuse was not read-only'

    try {
        $duplicateBinding = $bindingText.Replace('{"schema_version":2,', '{"schema_version":2,"schema_version":2,', [System.StringComparison]::Ordinal)
        Assert-Contract ($duplicateBinding -cne $bindingText) 'duplicate-key binding fixture was not constructed'
        [System.IO.File]::WriteAllText($bindingPath, $duplicateBinding, [System.Text.UTF8Encoding]::new($false))
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'duplicate canonical binding key'
    }
    finally { [System.IO.File]::WriteAllBytes($bindingPath, $bindingBytes) }

    try {
        [byte[]]$bomManifest = @([byte]0xef, [byte]0xbb, [byte]0xbf) + $manifestBytes
        [System.IO.File]::WriteAllBytes($manifestPath, $bomManifest)
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'BOM-prefixed canonical manifest'
    }
    finally { [System.IO.File]::WriteAllBytes($manifestPath, $manifestBytes) }

    try {
        $whitespacePrepared = $preparedText.Substring(0, $preparedText.Length - 1) + " `n"
        [System.IO.File]::WriteAllText($rowLeaves[0].FullName, $whitespacePrepared, [System.Text.UTF8Encoding]::new($false))
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'noncanonical row whitespace'
    }
    finally { [System.IO.File]::WriteAllText($rowLeaves[0].FullName, $preparedText, [System.Text.UTF8Encoding]::new($false)) }

    try {
        $badManifestTimestamp = [regex]::Replace($manifestText, '"published_at":"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}Z"', '"published_at":"2026-07-13T00:00:00Z"', 1)
        Assert-Contract ($badManifestTimestamp -cne $manifestText) 'manifest timestamp fixture was not constructed'
        [System.IO.File]::WriteAllText($manifestPath, $badManifestTimestamp, [System.Text.UTF8Encoding]::new($false))
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'noncanonical manifest timestamp value'
    }
    finally { [System.IO.File]::WriteAllBytes($manifestPath, $manifestBytes) }

    try {
        $badPreparedTimestamp = Set-SelfHashedStringValue -Raw $preparedText -Property 'utc' -Value '2026-07-13T00:00:00Z' -HashProperty 'row_sha256' -Domain 'dynamo-publication-row-v1'
        [System.IO.File]::WriteAllText($rowLeaves[0].FullName, $badPreparedTimestamp, [System.Text.UTF8Encoding]::new($false))
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'noncanonical publication-row timestamp value'
    }
    finally { [System.IO.File]::WriteAllText($rowLeaves[0].FullName, $preparedText, [System.Text.UTF8Encoding]::new($false)) }

    try {
        $badLeaseTimestamp = Set-SelfHashedStringValue -Raw $leaseText -Property 'created_at' -Value '2026-07-13T00:00:00Z' -HashProperty 'lease_sha256' -Domain 'dynamo-publication-lease-v1'
        [System.IO.File]::WriteAllText($closedLeases[0].FullName, $badLeaseTimestamp, [System.Text.UTF8Encoding]::new($false))
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'noncanonical publication-lease timestamp value'
    }
    finally { [System.IO.File]::WriteAllText($closedLeases[0].FullName, $leaseText, [System.Text.UTF8Encoding]::new($false)) }

    $extraLeasePath = Join-Path $closedLeases[0].DirectoryName 'unexpected-extra.lock'
    try {
        [System.IO.File]::WriteAllBytes($extraLeasePath, [System.IO.File]::ReadAllBytes($closedLeases[0].FullName))
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'extra publication lease entry'
    }
    finally {
        if (Test-Path -LiteralPath $extraLeasePath) { Remove-Item -LiteralPath $extraLeasePath -Force }
    }

    $bindingAclState = Add-ContractAclDrift -Path $bindingPath -Kind File
    try {
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'binding ACL drift'
    } finally { Restore-ContractAcl $bindingAclState }

    $publicationAclState = Add-ContractAclDrift -Path $publicationRoot -Kind Directory
    try {
        $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'publication subtree ACL drift'
    } finally { Restore-ContractAcl $publicationAclState }

    $terminalBarrierDirectory = Join-Path $Fixture.EvidenceRoot '.publisher-test-barriers'
    $terminalEntered = Join-Path $terminalBarrierDirectory 'before-publication-complete.entered'
    $terminalRelease = Join-Path $terminalBarrierDirectory 'before-publication-complete.release'
    foreach ($barrierLeaf in @($terminalEntered, $terminalRelease)) { if (Test-Path -LiteralPath $barrierLeaf) { Remove-Item -LiteralPath $barrierLeaf -Force } }
    $terminalBarrier = Start-Publisher -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Environment @{
        DYNAMO_REMEDIATION_TEST_MODE = '1'
        DYNAMO_REMEDIATION_PUBLISH_BARRIER = 'before-publication-complete'
    }
    $terminalDeadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not (Test-Path -LiteralPath $terminalEntered -PathType Leaf) -and -not $terminalBarrier.Process.HasExited -and [DateTime]::UtcNow -lt $terminalDeadline) { Start-Sleep -Milliseconds 25 }
    Assert-Contract (Test-Path -LiteralPath $terminalEntered -PathType Leaf) 'publisher terminal ACL barrier was not reached'
    $evidenceAclState = Add-ContractAclDrift -Path $Fixture.EvidenceRoot -Kind Directory
    try {
        $null = Complete-RunningPublisherRejectedWithoutMutation -Running $terminalBarrier -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -ReleasePath $terminalRelease -Case 'evidence-root ACL drift during terminal readback'
    } finally { Restore-ContractAcl $evidenceAclState }

    $publisherBytes = [System.IO.File]::ReadAllBytes($scriptPath)
    [System.IO.File]::AppendAllText($scriptPath, "`n# contract-tampered publisher path/blob`n", [System.Text.UTF8Encoding]::new($false))
    $beforeTamperedPublisher = Get-TreeFingerprint -Root (Join-Path $realCommon 'dynamo-remediation')
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'non-HEAD publisher bytes'
    Assert-Contract (($beforeTamperedPublisher -join "`n") -ceq ((Get-TreeFingerprint -Root (Join-Path $realCommon 'dynamo-remediation')) -join "`n")) 'non-HEAD publisher mutated control state before preflight rejection'
    [System.IO.File]::WriteAllBytes($scriptPath, $publisherBytes)

    $unrelatedEvidence = Join-Path (Split-Path -Parent $Fixture.EvidenceRoot) 'unrelated-preexisting-evidence'
    $unrelatedFinal = Join-Path $unrelatedEvidence "Dynamo/plan-set/$($Fixture.ExecutionBaseline)/$($handoff.plan_set_sha256)"
    $null = New-Item -ItemType Directory -Path $unrelatedFinal -Force
    [System.IO.File]::WriteAllText((Join-Path $unrelatedFinal 'unrelated'), 'not a published bundle', [System.Text.UTF8Encoding]::new($false))
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $unrelatedEvidence -Case 'unrelated pre-existing final destination'

    $bindingObject = $bindingText | ConvertFrom-Json -ErrorAction Stop
    $reordered = [ordered]@{}
    $bindingProperties = @($bindingObject.PSObject.Properties | Where-Object { $_.Name -ne 'binding_sha256' })
    for ($index = $bindingProperties.Count - 1; $index -ge 0; $index--) {
        $reordered[$bindingProperties[$index].Name] = $bindingProperties[$index].Value
    }
    $reorderedPreimage = ($reordered | ConvertTo-Json -Compress -Depth 30) + "`n"
    $reordered['binding_sha256'] = Get-DomainHash -Domain 'dynamo-plan-set-binding-v1' -CanonicalJson $reorderedPreimage
    [System.IO.File]::WriteAllText($bindingPath, (($reordered | ConvertTo-Json -Compress -Depth 30) + "`n"), [System.Text.UTF8Encoding]::new($false))
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'reordered binding'
    [System.IO.File]::WriteAllBytes($bindingPath, $bindingBytes)

    $tampered = $bindingText -replace [regex]::Escape($Fixture.ExecutionBaseline), ('f' * 40)
    [System.IO.File]::WriteAllText($bindingPath, $tampered, [System.Text.UTF8Encoding]::new($false))
    $null = Invoke-PublisherRejectedWithoutMutation -Repository $Fixture.Repository -EvidenceRoot $Fixture.EvidenceRoot -Case 'tampered binding'

    $publisherFailpoints = @('after-lease-create', 'after-bundle-publish', 'after-row-temp-write', 'after-bundle-prepared', 'after-binding-temp-write', 'after-binding-create', 'after-binding-committed', 'after-lease-close')
    for ($index = 0; $index -lt $publisherFailpoints.Count; $index++) {
        Assert-PublisherFailpointRecovery -SourceRepository $Fixture.Repository -CaseRoot (Join-Path (Split-Path -Parent $Fixture.Repository) "publisher-failpoint-$index") -Failpoint $publisherFailpoints[$index] -CheckGuard:($index -eq 0)
    }
    Assert-ExactPreexistingBundleRejected -SourceRepository $Fixture.Repository -CaseRoot (Join-Path (Split-Path -Parent $Fixture.Repository) 'publisher-exact-preexisting')
    Assert-PublisherGitObjectHardening -SourceRepository $Fixture.Repository -CaseRoot (Join-Path (Split-Path -Parent $Fixture.Repository) 'publisher-git-object-hardening')
    Assert-PublisherRepositoryIntegrityHardening -SourceRepository $Fixture.Repository -CaseRoot (Join-Path (Split-Path -Parent $Fixture.Repository) 'publisher-repository-integrity-hardening')
    Assert-PublisherPartialSkeletonConvergence -SourceRepository $Fixture.Repository -CaseRoot (Join-Path (Split-Path -Parent $Fixture.Repository) 'publisher-partial-skeleton-convergence')
    Assert-PublisherPreterminalAdmissionHardening -SourceRepository $Fixture.Repository -CaseRoot (Join-Path (Split-Path -Parent $Fixture.Repository) 'publisher-preterminal-admission-hardening')
    Assert-PublisherChainedFailpointRecovery -SourceRepository $Fixture.Repository -CaseRoot (Join-Path (Split-Path -Parent $Fixture.Repository) 'publisher-chained-failpoint-recovery')
    Assert-PublisherDriftBarriers -SourceRepository $Fixture.Repository -CaseRoot (Join-Path (Split-Path -Parent $Fixture.Repository) 'publisher-drift-barriers')
    Assert-PublisherMutationBoundaryTamper -SourceRepository $Fixture.Repository -CaseRoot (Join-Path (Split-Path -Parent $Fixture.Repository) 'publisher-mutation-boundary-tamper')
}

if ($PSVersionTable.PSVersion -lt [version]'7.4') {
    throw 'plan-set-publisher-contract: PowerShell 7.4 or newer is required'
}

$sourceRepository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$before = Get-RepositorySnapshot -Repository $sourceRepository
$suiteRoot = Join-Path ([System.IO.Path]::GetTempPath()) "dynamo-plan-set-publisher-contract-$PID-$([guid]::NewGuid().ToString('N'))"
$marker = Join-Path $suiteRoot '.dynamo-contract-owned'
$fixture = $null
$testError = $null
$suiteIdentity = $null
$markerHash = $null

try {
    $null = New-Item -ItemType Directory -Path $suiteRoot
    [System.IO.File]::WriteAllText($marker, 'plan-set-publisher-contract', [System.Text.UTF8Encoding]::new($false))
    $suiteIdentity = Get-OwnedRootIdentity -Root $suiteRoot
    $markerHash = (Get-FileHash -LiteralPath $marker -Algorithm SHA256).Hash.ToLowerInvariant()
    $fixture = New-PublisherFixture -SourceRepository $sourceRepository -SuiteRoot $suiteRoot
    Assert-PublisherContract -Fixture $fixture
}
catch {
    $testError = $_
}
finally {
    try {
        Stop-PublisherChildren
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
        if ($null -eq $testError) {
            $testError = $_
        }
    }

    try {
        $after = Get-RepositorySnapshot -Repository $sourceRepository
        Assert-RepositoryUnchanged -Before $before -After $after
    }
    catch {
        if ($null -eq $testError) {
            $testError = $_
        }
    }
}

if ($null -ne $testError) {
    throw $testError
}

Write-Output '{"contract":"plan-set-publisher","status":"pass"}'
