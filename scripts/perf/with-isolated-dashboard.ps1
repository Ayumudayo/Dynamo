[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $FixtureMode,

    [Parameter(Mandatory)]
    [string] $Workload,

    [Parameter(Mandatory)]
    [string] $OutputRoot,

    [Parameter(Mandatory)]
    [string] $Label,

    [string] $Path = '/',

    [int] $Requests = 50,

    [int] $Concurrency = 1,

    [string[]] $Spec = @(),

    [string[]] $Project = @(),

    [string] $ExpectedOutcome = 'Pass',

    [string] $ExpectedFailureId
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$script:RunnerVersion = 'with-isolated-dashboard-v1'
$script:FixtureVersion = 'guild-detail-v1'
$script:FixtureSha256 = '5f08c171827be0ad90f5a6b7c980b4ab21d938cd2e73137f03f5ac7360b64885'
$script:ContractMarkerBody = "dynamo-perf-contract-v1`n"
$script:Utf8NoBom = [System.Text.UTF8Encoding]::new($false)

function Throw-RunnerFailure {
    param([Parameter(Mandatory)][string] $Code)
    $errorRecord = [System.InvalidOperationException]::new($Code)
    $errorRecord.Data['DynamoRunnerCode'] = $Code
    throw $errorRecord
}

function Get-RunnerFailureCode {
    param([Parameter(Mandatory)][System.Exception] $Exception)
    if ($Exception.Data.Contains('DynamoRunnerCode')) {
        return [string] $Exception.Data['DynamoRunnerCode']
    }
    return 'unexpected-runner-failure'
}

function Get-Sha256HexFromBytes {
    param([Parameter(Mandatory)][AllowEmptyCollection()][byte[]] $Bytes)
    $hash = [System.Security.Cryptography.SHA256]::HashData($Bytes)
    return [Convert]::ToHexString($hash).ToLowerInvariant()
}

function Get-Sha256HexFromString {
    param([Parameter(Mandatory)][AllowEmptyString()][string] $Value)
    return Get-Sha256HexFromBytes -Bytes $script:Utf8NoBom.GetBytes($Value)
}

function Get-FileSha256Hex {
    param([Parameter(Mandatory)][string] $LiteralPath)
    try {
        $stream = [System.IO.FileStream]::new(
            $LiteralPath,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::Read)
        try {
            return [Convert]::ToHexString(
                [System.Security.Cryptography.SHA256]::HashData($stream)
            ).ToLowerInvariant()
        }
        finally {
            $stream.Dispose()
        }
    }
    catch {
        Throw-RunnerFailure 'artifact-hash-failed'
    }
}

function Get-RandomHex {
    param([ValidateRange(16, 128)][int] $ByteCount = 32)
    $bytes = [byte[]]::new($ByteCount)
    [System.Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
    return [Convert]::ToHexString($bytes).ToLowerInvariant()
}

function New-ExclusiveDirectoryNative {
    param([Parameter(Mandatory)][string] $LiteralPath)
    if ($null -eq ('Dynamo.Perf.Runner.NativeDirectory' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

namespace Dynamo.Perf.Runner
{
    public static class NativeDirectory
    {
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern bool CreateDirectoryW(string path, IntPtr securityAttributes);

        public static int CreateExclusive(string path)
        {
            if (CreateDirectoryW(path, IntPtr.Zero)) return 0;
            return Marshal.GetLastWin32Error();
        }
    }
}
'@
    }
    $errorCode = [Dynamo.Perf.Runner.NativeDirectory]::CreateExclusive($LiteralPath)
    if ($errorCode -eq 0) { return $true }
    if ($errorCode -eq 183) { return $false }
    Throw-RunnerFailure 'attempt-allocation-failed'
}

function Assert-ExactJsonKeys {
    param(
        [Parameter(Mandatory)][object] $Value,
        [Parameter(Mandatory)][string[]] $Keys,
        [Parameter(Mandatory)][string] $FailureCode
    )
    if ($null -eq $Value -or $Value -is [string] -or $Value -is [System.Collections.IEnumerable] -and $Value -isnot [pscustomobject]) {
        Throw-RunnerFailure $FailureCode
    }
    $actual = @($Value.PSObject.Properties.Name | Sort-Object)
    $expected = @($Keys | Sort-Object)
    if ($actual.Count -ne $expected.Count) { Throw-RunnerFailure $FailureCode }
    for ($index = 0; $index -lt $expected.Count; $index++) {
        if ($actual[$index] -cne $expected[$index]) { Throw-RunnerFailure $FailureCode }
    }
}

function Assert-RegularPath {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [Parameter(Mandatory)][ValidateSet('Leaf', 'Container')][string] $Kind,
        [Parameter(Mandatory)][string] $FailureCode
    )
    try {
        $item = Get-Item -LiteralPath $LiteralPath -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            Throw-RunnerFailure $FailureCode
        }
        if ($Kind -eq 'Leaf' -and -not ($item -is [System.IO.FileInfo])) {
            Throw-RunnerFailure $FailureCode
        }
        if ($Kind -eq 'Container' -and -not ($item -is [System.IO.DirectoryInfo])) {
            Throw-RunnerFailure $FailureCode
        }
        return $item
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure $FailureCode
    }
}

function Assert-PathUnderRoot {
    param(
        [Parameter(Mandatory)][string] $Root,
        [Parameter(Mandatory)][string] $Candidate,
        [Parameter(Mandatory)][string] $FailureCode
    )
    $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
    $candidateFull = [System.IO.Path]::GetFullPath($Candidate)
    $prefix = "$rootFull$([System.IO.Path]::DirectorySeparatorChar)"
    if (-not $candidateFull.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        Throw-RunnerFailure $FailureCode
    }
    return $candidateFull
}

function Assert-ExistingPathChainNoReparse {
    param(
        [Parameter(Mandatory)][string] $Root,
        [Parameter(Mandatory)][string] $Candidate,
        [Parameter(Mandatory)][string] $FailureCode
    )
    $rootItem = Assert-RegularPath -LiteralPath $Root -Kind Container -FailureCode $FailureCode
    $candidateFull = Assert-PathUnderRoot -Root $rootItem.FullName -Candidate $Candidate -FailureCode $FailureCode
    $relative = [System.IO.Path]::GetRelativePath($rootItem.FullName, $candidateFull)
    $cursor = $rootItem.FullName
    foreach ($component in $relative.Split([System.IO.Path]::DirectorySeparatorChar, [System.StringSplitOptions]::RemoveEmptyEntries)) {
        $cursor = Join-Path $cursor $component
        if (Test-Path -LiteralPath $cursor) {
            [void](Assert-RegularPath -LiteralPath $cursor -Kind Container -FailureCode $FailureCode)
        }
        else {
            break
        }
    }
    return $candidateFull
}

function New-OwnedDirectoryChain {
    param(
        [Parameter(Mandatory)][string] $Root,
        [Parameter(Mandatory)][string] $Candidate,
        [Parameter(Mandatory)][string] $FailureCode
    )
    $candidateFull = Assert-ExistingPathChainNoReparse -Root $Root -Candidate $Candidate -FailureCode $FailureCode
    $relative = [System.IO.Path]::GetRelativePath($Root, $candidateFull)
    $cursor = [System.IO.Path]::GetFullPath($Root)
    foreach ($component in $relative.Split([System.IO.Path]::DirectorySeparatorChar, [System.StringSplitOptions]::RemoveEmptyEntries)) {
        $next = Join-Path $cursor $component
        if (-not (Test-Path -LiteralPath $next)) {
            try {
                [void][System.IO.Directory]::CreateDirectory($next)
            }
            catch {
                Throw-RunnerFailure $FailureCode
            }
        }
        [void](Assert-RegularPath -LiteralPath $next -Kind Container -FailureCode $FailureCode)
        $cursor = $next
    }
    return $cursor
}

function Set-ProtectedAttemptAcl {
    param([Parameter(Mandatory)][string] $AttemptDirectory)
    try {
        $currentSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
        $systemSid = [System.Security.Principal.SecurityIdentifier]::new(
            [System.Security.Principal.WellKnownSidType]::LocalSystemSid,
            $null)
        $inheritance = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
            [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
        $propagation = [System.Security.AccessControl.PropagationFlags]::None
        $rights = [System.Security.AccessControl.FileSystemRights]::FullControl
        $allow = [System.Security.AccessControl.AccessControlType]::Allow
        $directoryInfo = [System.IO.DirectoryInfo]::new($AttemptDirectory)
        $security = [System.IO.FileSystemAclExtensions]::GetAccessControl($directoryInfo)
        $existingOwner = $security.GetOwner(
            [System.Security.Principal.SecurityIdentifier]).Value
        if ($existingOwner -cne $currentSid.Value) {
            Throw-RunnerFailure 'attempt-acl-owner-invalid'
        }
        $security.SetAccessRuleProtection($true, $false)
        foreach ($existingRule in @($security.GetAccessRules(
            $true,
            $false,
            [System.Security.Principal.SecurityIdentifier]))) {
            [void]$security.RemoveAccessRuleSpecific($existingRule)
        }
        [void]$security.AddAccessRule(
            [System.Security.AccessControl.FileSystemAccessRule]::new(
                $currentSid, $rights, $inheritance, $propagation, $allow))
        [void]$security.AddAccessRule(
            [System.Security.AccessControl.FileSystemAccessRule]::new(
                $systemSid, $rights, $inheritance, $propagation, $allow))
        [System.IO.FileSystemAclExtensions]::SetAccessControl($directoryInfo, $security)
        $readback = [System.IO.FileSystemAclExtensions]::GetAccessControl($directoryInfo)
        $readbackOwner = $readback.GetOwner([System.Security.Principal.SecurityIdentifier]).Value
        if ($readbackOwner -cne $currentSid.Value -or -not $readback.AreAccessRulesProtected) {
            Throw-RunnerFailure 'attempt-acl-readback-failed'
        }
        $readbackRules = @($readback.GetAccessRules(
            $true,
            $true,
            [System.Security.Principal.SecurityIdentifier]))
        if ($readbackRules.Count -ne 2) {
            Throw-RunnerFailure 'attempt-acl-readback-failed'
        }
        $expectedSids = @($currentSid.Value, $systemSid.Value)
        $seenSids = [System.Collections.Generic.HashSet[string]]::new(
            [System.StringComparer]::Ordinal)
        foreach ($rule in $readbackRules) {
            $ruleSid = $rule.IdentityReference.Value
            if ($expectedSids -cnotcontains $ruleSid -or
                -not $seenSids.Add($ruleSid) -or $rule.IsInherited -or
                $rule.AccessControlType -ne $allow -or $rule.FileSystemRights -ne $rights -or
                $rule.InheritanceFlags -ne $inheritance -or
                $rule.PropagationFlags -ne $propagation) {
                Throw-RunnerFailure 'attempt-acl-readback-failed'
            }
        }
        if ($seenSids.Count -ne 2) { Throw-RunnerFailure 'attempt-acl-readback-failed' }
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure 'attempt-acl-failed'
    }
}

function Assert-DirectoryOwnedByCurrentUser {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [Parameter(Mandatory)][string] $FailureCode
    )
    try {
        $directory = Assert-RegularPath -LiteralPath $LiteralPath -Kind Container `
            -FailureCode $FailureCode
        $security = [System.IO.FileSystemAclExtensions]::GetAccessControl($directory)
        $owner = $security.GetOwner([System.Security.Principal.SecurityIdentifier]).Value
        $current = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
        if ($owner -cne $current) { Throw-RunnerFailure $FailureCode }
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure $FailureCode
    }
}

function Write-ExclusiveJson {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [Parameter(Mandatory)][object] $Value
    )
    $parent = Split-Path -LiteralPath $LiteralPath
    [void](Assert-RegularPath -LiteralPath $parent -Kind Container -FailureCode 'artifact-parent-invalid')
    if (Test-Path -LiteralPath $LiteralPath) { Throw-RunnerFailure 'artifact-leaf-exists' }
    $body = ($Value | ConvertTo-Json -Depth 32 -Compress) + "`n"
    $temporary = Join-Path $parent ('.publish-' + (Get-RandomHex -ByteCount 16) + '.tmp')
    $published = $false
    try {
        $stream = [System.IO.FileStream]::new(
            $temporary,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None,
            4096,
            [System.IO.FileOptions]::WriteThrough)
        try {
            $bytes = $script:Utf8NoBom.GetBytes($body)
            $stream.Write($bytes, 0, $bytes.Length)
            $stream.Flush($true)
        }
        finally {
            $stream.Dispose()
        }
        if ([System.IO.File]::ReadAllText($temporary, $script:Utf8NoBom) -cne $body) {
            Throw-RunnerFailure 'artifact-temporary-readback-failed'
        }
        [System.IO.File]::Move($temporary, $LiteralPath, $false)
        $published = $true
        if ([System.IO.File]::ReadAllText($LiteralPath, $script:Utf8NoBom) -cne $body) {
            Throw-RunnerFailure 'artifact-final-readback-failed'
        }
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure 'artifact-publication-failed'
    }
    finally {
        if (-not $published -and (Test-Path -LiteralPath $temporary)) {
            Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
        }
    }
}

function Read-BoundedUtf8File {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [ValidateRange(1, 16777216)][int] $MaximumBytes = 2097152,
        [switch] $AllowEmpty
    )
    try {
        $item = Assert-RegularPath -LiteralPath $LiteralPath -Kind Leaf -FailureCode 'child-log-invalid'
        if ($item.Length -gt $MaximumBytes -or (-not $AllowEmpty -and $item.Length -eq 0)) {
            Throw-RunnerFailure 'child-log-size-invalid'
        }
        $stream = [System.IO.FileStream]::new(
            $LiteralPath,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::ReadWrite)
        try {
            $reader = [System.IO.StreamReader]::new(
                $stream,
                $script:Utf8NoBom,
                $true,
                4096,
                $true)
            try { return $reader.ReadToEnd() }
            finally { $reader.Dispose() }
        }
        finally { $stream.Dispose() }
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure 'child-log-read-failed'
    }
}

function Read-ExactJsonFile {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [ValidateRange(1, 16777216)][int] $MaximumBytes = 2097152,
        [Parameter(Mandatory)][string] $FailureCode
    )
    try {
        $item = Assert-RegularPath -LiteralPath $LiteralPath -Kind Leaf -FailureCode $FailureCode
        if ($item.Length -le 0 -or $item.Length -gt $MaximumBytes) { Throw-RunnerFailure $FailureCode }
        $body = [System.IO.File]::ReadAllText($LiteralPath, $script:Utf8NoBom)
        return $body | ConvertFrom-Json -Depth 32
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure $FailureCode
    }
}

function ConvertFrom-ExactJsonLine {
    param(
        [Parameter(Mandatory)][string] $Body,
        [Parameter(Mandatory)][string] $FailureCode
    )
    if ($Body -notmatch '^\{[^\r\n]*\}\r?\n$') { Throw-RunnerFailure $FailureCode }
    try { return $Body.TrimEnd("`r", "`n") | ConvertFrom-Json -Depth 32 }
    catch { Throw-RunnerFailure $FailureCode }
}

function Get-MinimalChildEnvironment {
    $environment = [ordered]@{}
    foreach ($name in @(
        'SystemRoot', 'WINDIR', 'ComSpec', 'PATH', 'PATHEXT', 'TEMP', 'TMP',
        'USERPROFILE', 'HOME', 'LOCALAPPDATA', 'APPDATA', 'PROGRAMDATA',
        'NUMBER_OF_PROCESSORS', 'PROCESSOR_ARCHITECTURE', 'CARGO_HOME',
        'RUSTUP_HOME', 'RUSTUP_TOOLCHAIN', 'PLAYWRIGHT_BROWSERS_PATH', 'CI'
    )) {
        $value = [System.Environment]::GetEnvironmentVariable($name, 'Process')
        if ($null -ne $value -and $value.Length -gt 0) { $environment[$name] = [string]$value }
    }
    $environment['NO_COLOR'] = '1'
    $environment['CARGO_TERM_COLOR'] = 'never'
    return $environment
}

function Resolve-ReparseFreeApplicationPath {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [Parameter(Mandatory)][string] $FailureCode
    )
    try {
        $candidate = [System.IO.Path]::GetFullPath($LiteralPath)
        for ($hop = 0; $hop -lt 8; $hop++) {
            $root = [System.IO.Path]::GetPathRoot($candidate)
            if ([string]::IsNullOrEmpty($root)) { Throw-RunnerFailure $FailureCode }
            $components = @($candidate.Substring($root.Length).Split(
                [System.IO.Path]::DirectorySeparatorChar,
                [System.StringSplitOptions]::RemoveEmptyEntries))
            if ($components.Count -lt 1) { Throw-RunnerFailure $FailureCode }
            $cursor = $root
            $resolvedReparse = $false
            for ($index = 0; $index -lt $components.Count; $index++) {
                $cursor = Join-Path $cursor $components[$index]
                $item = Get-Item -LiteralPath $cursor -Force
                $isFinal = $index -eq ($components.Count - 1)
                $isReparse = ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0
                if ($isReparse) {
                    $target = $item.ResolveLinkTarget($true)
                    if ($null -eq $target -or (-not $isFinal -and
                        $target -isnot [System.IO.DirectoryInfo])) {
                        Throw-RunnerFailure $FailureCode
                    }
                    $candidate = [System.IO.Path]::GetFullPath($target.FullName)
                    for ($remaining = $index + 1; $remaining -lt $components.Count; $remaining++) {
                        $candidate = Join-Path $candidate $components[$remaining]
                    }
                    $resolvedReparse = $true
                    break
                }
                if (-not $isFinal -and $item -isnot [System.IO.DirectoryInfo]) {
                    Throw-RunnerFailure $FailureCode
                }
                if ($isFinal -and $item -isnot [System.IO.FileInfo]) {
                    Throw-RunnerFailure $FailureCode
                }
            }
            if (-not $resolvedReparse) {
                [void](Assert-RegularPath -LiteralPath $candidate -Kind Leaf `
                    -FailureCode $FailureCode)
                return $candidate
            }
        }
        Throw-RunnerFailure $FailureCode
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure $FailureCode
    }
}

function Resolve-Executable {
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][string] $FailureCode
    )
    try {
        $command = Get-Command -Name $Name -CommandType Application -ErrorAction Stop | Select-Object -First 1
        $path = [System.IO.Path]::GetFullPath([string]$command.Source)
        return Resolve-ReparseFreeApplicationPath -LiteralPath $path -FailureCode $FailureCode
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure $FailureCode
    }
}

function Invoke-DirectBoundedProcess {
    param(
        [Parameter(Mandatory)][string] $ExecutablePath,
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]] $ArgumentList,
        [Parameter(Mandatory)][string] $WorkingDirectory,
        [ValidateRange(100, 120000)][int] $TimeoutMilliseconds = 30000,
        [int[]] $AllowedExitCodes = @(0),
        [Parameter(Mandatory)][string] $FailureCode
    )
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $ExecutablePath
    $start.WorkingDirectory = $WorkingDirectory
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardOutputEncoding = $script:Utf8NoBom
    $start.StandardErrorEncoding = $script:Utf8NoBom
    $start.Environment.Clear()
    foreach ($entry in (Get-MinimalChildEnvironment).GetEnumerator()) {
        $start.Environment[[string]$entry.Key] = [string]$entry.Value
    }
    $start.Environment['GIT_OPTIONAL_LOCKS'] = '0'
    foreach ($argument in $ArgumentList) { [void]$start.ArgumentList.Add($argument) }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    try {
        if (-not $process.Start()) { Throw-RunnerFailure $FailureCode }
        $process.StandardInput.Close()
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($TimeoutMilliseconds)) {
            $killError = $null
            try {
                if (-not $process.HasExited) { $process.Kill($true) }
            }
            catch { $killError = $_.Exception }
            $terminated = $process.WaitForExit(5000)
            if (-not $terminated) { Throw-RunnerFailure 'direct-process-cleanup-failed' }
            Throw-RunnerFailure $FailureCode
        }
        [void][System.Threading.Tasks.Task]::WaitAll(@($stdoutTask, $stderrTask), 5000)
        if ($stdoutTask.Result.Length -gt 33554432 -or $stderrTask.Result.Length -gt 4194304) {
            Throw-RunnerFailure $FailureCode
        }
        if ($AllowedExitCodes -notcontains $process.ExitCode) { Throw-RunnerFailure $FailureCode }
        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = $stdoutTask.Result
            Stderr = $stderrTask.Result
        }
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure $FailureCode
    }
    finally { $process.Dispose() }
}

function Invoke-Git {
    param(
        [Parameter(Mandatory)][string] $GitPath,
        [Parameter(Mandatory)][string] $RepositoryRoot,
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]] $Arguments,
        [int[]] $AllowedExitCodes = @(0),
        [string] $FailureCode = 'git-command-failed'
    )
    $safeArguments = @(
        '--no-pager',
        '-c', 'core.fsmonitor=false',
        '-c', 'core.untrackedCache=false',
        '-c', 'diff.external='
    ) + $Arguments
    return Invoke-DirectBoundedProcess -ExecutablePath $GitPath -ArgumentList $safeArguments `
        -WorkingDirectory $RepositoryRoot -AllowedExitCodes $AllowedExitCodes `
        -FailureCode $FailureCode
}

function Get-SourceSnapshot {
    param(
        [Parameter(Mandatory)][string] $GitPath,
        [Parameter(Mandatory)][string] $RepositoryRoot
    )
    $head = (Invoke-Git -GitPath $GitPath -RepositoryRoot $RepositoryRoot `
        -Arguments @('rev-parse', '--verify', 'HEAD') -FailureCode 'source-head-failed').Stdout.Trim()
    if ($head -cnotmatch '^[0-9a-f]{40}$') { Throw-RunnerFailure 'source-head-invalid' }
    $status = (Invoke-Git -GitPath $GitPath -RepositoryRoot $RepositoryRoot `
        -Arguments @('status', '--porcelain=v1', '-z', '--untracked-files=all') `
        -FailureCode 'source-status-failed').Stdout
    if ($status.Length -ne 0) { Throw-RunnerFailure 'source-not-clean' }
    $flags = (Invoke-Git -GitPath $GitPath -RepositoryRoot $RepositoryRoot `
        -Arguments @('ls-files', '-v', '-z', '--') -FailureCode 'source-index-flags-failed').Stdout
    foreach ($record in $flags.Split([char]0, [System.StringSplitOptions]::RemoveEmptyEntries)) {
        if ($record.Length -lt 2) { Throw-RunnerFailure 'source-index-flags-invalid' }
        $flag = $record[0]
        if ([char]::IsLower($flag) -or $flag -ceq 'S') {
            Throw-RunnerFailure 'source-hidden-index-state'
        }
    }
    $worktreeDiff = (Invoke-Git -GitPath $GitPath -RepositoryRoot $RepositoryRoot `
        -Arguments @('diff', '--no-ext-diff', '--binary', 'HEAD', '--') `
        -FailureCode 'source-diff-failed').Stdout
    $cachedDiff = (Invoke-Git -GitPath $GitPath -RepositoryRoot $RepositoryRoot `
        -Arguments @('diff', '--no-ext-diff', '--cached', '--binary', 'HEAD', '--') `
        -FailureCode 'source-cached-diff-failed').Stdout
    $canonical = "worktree`0$worktreeDiff`0cached`0$cachedDiff`0untracked`0"
    return [ordered]@{
        head = $head
        clean = $true
        diff_sha256 = Get-Sha256HexFromString -Value $canonical
    }
}

function Assert-SnapshotEqual {
    param(
        [Parameter(Mandatory)][System.Collections.IDictionary] $Expected,
        [Parameter(Mandatory)][System.Collections.IDictionary] $Actual
    )
    foreach ($key in @('head', 'clean', 'diff_sha256')) {
        if ([string]$Expected[$key] -cne [string]$Actual[$key]) {
            Throw-RunnerFailure 'source-state-drift'
        }
    }
}

function Get-EnvironmentIdentity {
    param(
        [Parameter(Mandatory)][bool] $ContractMode,
        [string] $NodePath,
        [string] $RustcPath,
        [Parameter(Mandatory)][string] $RepositoryRoot
    )
    if ($ContractMode) {
        $raw = [ordered]@{
            os_build = 'contract-windows'
            arch = 'x64'
            cpu_model = 'contract-cpu'
            logical_cores = 1
            power_profile = 'contract-power'
            node_version = 'v0.0.0-contract'
            rustc_version = 'rustc-contract'
            cargo_profile = 'release'
        }
    }
    else {
        $nodeVersion = (Invoke-DirectBoundedProcess -ExecutablePath $NodePath `
            -ArgumentList @('--version') -WorkingDirectory $RepositoryRoot `
            -FailureCode 'node-version-failed').Stdout.Trim()
        $rustcVersion = (Invoke-DirectBoundedProcess -ExecutablePath $RustcPath `
            -ArgumentList @('--version') -WorkingDirectory $RepositoryRoot `
            -FailureCode 'rustc-version-failed').Stdout.Trim()
        $cpuModel = [System.Environment]::GetEnvironmentVariable('PROCESSOR_IDENTIFIER', 'Process')
        if ([string]::IsNullOrWhiteSpace($cpuModel)) { $cpuModel = 'windows-cpu' }
        $powerProfile = 'windows-active-profile-unrecorded'
        try {
            $powerCfg = Resolve-Executable -Name 'powercfg.exe' -FailureCode 'powercfg-unavailable'
            $power = Invoke-DirectBoundedProcess -ExecutablePath $powerCfg -ArgumentList @('/getactivescheme') `
                -WorkingDirectory $RepositoryRoot -FailureCode 'power-profile-failed'
            $candidate = $power.Stdout.Trim()
            if (-not [string]::IsNullOrWhiteSpace($candidate)) { $powerProfile = $candidate }
        }
        catch {
            $powerProfile = 'windows-active-profile-unavailable'
        }
        foreach ($value in @($nodeVersion, $rustcVersion, $cpuModel, $powerProfile)) {
            if ([string]::IsNullOrWhiteSpace($value) -or $value.Length -gt 256) {
                Throw-RunnerFailure 'environment-identity-invalid'
            }
        }
        $raw = [ordered]@{
            os_build = [System.Environment]::OSVersion.VersionString
            arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
            cpu_model = $cpuModel
            logical_cores = [System.Environment]::ProcessorCount
            power_profile = $powerProfile
            node_version = $nodeVersion
            rustc_version = $rustcVersion
            cargo_profile = 'release'
        }
    }
    $fingerprint = Get-Sha256HexFromString -Value ($raw | ConvertTo-Json -Compress)
    return [ordered]@{
        fingerprint_sha256 = $fingerprint
        os_build = $raw.os_build
        arch = $raw.arch
        cpu_model = $raw.cpu_model
        logical_cores = $raw.logical_cores
        power_profile = $raw.power_profile
        node_version = $raw.node_version
        rustc_version = $raw.rustc_version
        cargo_profile = $raw.cargo_profile
    }
}

function Start-RunnerJob {
    param(
        [Parameter(Mandatory)][string] $ExecutablePath,
        [Parameter(Mandatory)][AllowEmptyCollection()][object[]] $ArgumentList,
        [Parameter(Mandatory)][string] $WorkingDirectory,
        [Parameter(Mandatory)][System.Collections.IDictionary] $Environment,
        [Parameter(Mandatory)][string] $AttemptDirectory,
        [Parameter(Mandatory)][string] $Name,
        [string] $LaunchDiagnosticPath
    )
    $stdoutPath = Join-Path $AttemptDirectory (".$Name.stdout.tmp")
    $stderrPath = Join-Path $AttemptDirectory (".$Name.stderr.tmp")
    if ((Test-Path -LiteralPath $stdoutPath) -or (Test-Path -LiteralPath $stderrPath)) {
        Throw-RunnerFailure 'child-log-leaf-exists'
    }
    try {
        $job = Start-DynamoIsolatedProcess -ExecutablePath $ExecutablePath `
            -ArgumentList $ArgumentList -WorkingDirectory $WorkingDirectory `
            -Environment $Environment -StandardOutputPath $stdoutPath `
            -StandardErrorPath $stderrPath
        $evidence = Get-DynamoIsolatedProcessEvidence -Process $job
        if (-not $evidence.IsProcessInJob -or $evidence.ActiveProcessCount -lt 1 -or
            $evidence.ActiveProcessIds -notcontains [uint64]$job.ProcessId) {
            Stop-DynamoIsolatedProcess -Process $job
            Throw-RunnerFailure 'child-job-membership-failed'
        }
        return [pscustomobject]@{ Job = $job; StdoutPath = $stdoutPath; StderrPath = $stderrPath }
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        if (-not [string]::IsNullOrEmpty($LaunchDiagnosticPath)) {
            try {
                Write-ExclusiveJson -LiteralPath $LaunchDiagnosticPath -Value ([ordered]@{
                    schema_version = 1
                    failure_code = 'child-launch-failed'
                    process_role = $Name
                    exception_type = $_.Exception.GetType().FullName
                    exception_hresult = [int64]$_.Exception.HResult
                    inner_exception_type = if ($null -eq $_.Exception.InnerException) {
                        $null
                    } else {
                        $_.Exception.InnerException.GetType().FullName
                    }
                    inner_exception_hresult = if ($null -eq $_.Exception.InnerException) {
                        [int64]0
                    } else {
                        [int64]$_.Exception.InnerException.HResult
                    }
                })
            }
            catch { }
        }
        Throw-RunnerFailure 'child-launch-failed'
    }
}

function Get-JobEvidenceRow {
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][string] $Phase,
        [Parameter(Mandatory)][object] $Handle,
        [AllowNull()][object] $Evidence
    )
    if ($null -eq $Evidence) { $Evidence = Get-DynamoIsolatedProcessEvidence -Process $Handle.Job }
    if ($Evidence.ProcessId -ne $Handle.Job.ProcessId -or
        $Evidence.CreationFileTimeUtc -ne $Handle.Job.CreationFileTimeUtc -or
        $Evidence.ActiveProcessCount -lt 0 -or $Evidence.TotalProcessCount -lt 1) {
        Throw-RunnerFailure 'job-evidence-invalid'
    }
    return [ordered]@{
        name = $Name
        phase = $Phase
        direct_pid = $Handle.Job.ProcessId
        creation_file_time_utc = [uint64]$Handle.Job.CreationFileTimeUtc
        is_process_in_job = [bool]$Evidence.IsProcessInJob
        active_processes = [int64]$Evidence.ActiveProcessCount
        total_processes = [int64]$Evidence.TotalProcessCount
        terminated_processes = [int64]$Evidence.TerminatedProcessCount
        active_process_ids = @($Evidence.ActiveProcessIds | ForEach-Object { [uint64]$_ })
    }
}

function Get-HarnessRssBytes {
    param([Parameter(Mandatory)][object] $HarnessHandle)
    try {
        $process = [System.Diagnostics.Process]::GetProcessById($HarnessHandle.Job.ProcessId)
        try {
            $creation = [uint64]$process.StartTime.ToUniversalTime().ToFileTimeUtc()
            $rss = [int64]$process.WorkingSet64
            if ($creation -ne $HarnessHandle.Job.CreationFileTimeUtc -or $rss -le 0) {
                Throw-RunnerFailure 'harness-rss-invalid'
            }
            return $rss
        }
        finally { $process.Dispose() }
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure 'harness-rss-unavailable'
    }
}

function Get-SanitizedProcessSnapshot {
    param(
        [Parameter(Mandatory)][object] $Handle,
        [Parameter(Mandatory)][object] $Evidence,
        [Parameter(Mandatory)][int64] $ObservedElapsedMilliseconds,
        [Parameter(Mandatory)][string] $Phase
    )
    $processIds = @($Evidence.ActiveProcessIds | Select-Object -First 64 | ForEach-Object { [uint64]$_ })
    $parentRecords = @{}
    $parentQueryStatus = 'not-needed'
    if ($processIds.Count -gt 0) {
        try {
            $filter = ($processIds | ForEach-Object { "ProcessId = $_" }) -join ' OR '
            $query = "SELECT ProcessId, Name, ExecutablePath, ParentProcessId, CreationDate FROM Win32_Process WHERE $filter"
            foreach ($record in @(Get-CimInstance -Query $query -OperationTimeoutSec 2 -ErrorAction Stop)) {
                $parentRecords[[uint64]$record.ProcessId] = $record
            }
            $parentQueryStatus = 'ok'
        }
        catch { $parentQueryStatus = 'unavailable' }
    }
    $rows = [System.Collections.Generic.List[object]]::new()
    foreach ($processId in $processIds) {
        $row = [ordered]@{
            pid = $processId
            creation_file_time_utc = [uint64]0
            name = $null
            executable_path = $null
            parent_pid = [uint64]0
            observed_elapsed_ms = $ObservedElapsedMilliseconds
            phase = $Phase
            observed_in_initial_job_snapshot = $true
            job_member = $false
            query_status = 'unavailable'
            parent_query_status = $parentQueryStatus
        }
        try {
            $process = [System.Diagnostics.Process]::GetProcessById([int]$processId)
            try {
                $creationFileTimeUtc = [uint64]$process.StartTime.ToUniversalTime().ToFileTimeUtc()
                $name = [string]$process.ProcessName
                $path = [string]$process.MainModule.FileName
            }
            finally { $process.Dispose() }
            if ($name.Length -lt 1 -or $name.Length -gt 256 -or $name -match '[\x00-\x1f\x7f]' -or
                $path.Length -lt 1 -or $path.Length -gt 1024 -or $path -match '[\x00-\x1f\x7f]') {
                $row.query_status = 'invalid-data'
            }
            else {
                $currentEvidence = Get-DynamoIsolatedProcessEvidence -Process $Handle.Job
                $stillActive = @($currentEvidence.ActiveProcessIds) -contains $processId
                $birthStillMatches = $false
                if ($stillActive) {
                    try {
                        $currentProcess = [System.Diagnostics.Process]::GetProcessById([int]$processId)
                        try {
                            $birthStillMatches = [uint64]$currentProcess.StartTime.ToUniversalTime().ToFileTimeUtc() `
                                -eq $creationFileTimeUtc
                        }
                        finally { $currentProcess.Dispose() }
                    }
                    catch { $birthStillMatches = $false }
                }
                if (-not $stillActive -or -not $birthStillMatches) {
                    $row.query_status = 'raced-or-exited'
                }
                else {
                    $row.creation_file_time_utc = $creationFileTimeUtc
                    $row.name = $name
                    $row.executable_path = [System.IO.Path]::GetFullPath($path)
                    $row.job_member = $true
                    $row.query_status = 'ok-parent-unavailable'
                    if ($parentRecords.ContainsKey($processId)) {
                        $parentRecord = $parentRecords[$processId]
                        $cimCreationFileTimeUtc = [uint64]0
                        try {
                            $cimCreationFileTimeUtc = [uint64]([DateTime]$parentRecord.CreationDate).ToUniversalTime().ToFileTimeUtc()
                        }
                        catch { }
                        if ($cimCreationFileTimeUtc -eq $creationFileTimeUtc) {
                            $row.parent_pid = [uint64]$parentRecord.ParentProcessId
                            $row.parent_query_status = 'ok'
                            $row.query_status = 'ok'
                        }
                        else { $row.parent_query_status = 'raced-or-unavailable' }
                    }
                }
            }
        }
        catch {
            $row.query_status = 'raced-or-exited'
        }
        $rows.Add([pscustomobject]$row)
    }
    return [ordered]@{
        phase = $Phase
        observed_elapsed_ms = $ObservedElapsedMilliseconds
        active_processes = [int64]$Evidence.ActiveProcessCount
        captured_processes = $rows.Count
        truncated = [int64]$Evidence.ActiveProcessCount -gt $rows.Count
        processes = @($rows.ToArray())
    }
}

function Write-DescendantDiagnostic {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [Parameter(Mandatory)][object] $Handle,
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][object[]] $Samples,
        [string] $FailureCode = 'child-descendants-survived'
    )
    Write-ExclusiveJson -LiteralPath $LiteralPath -Value ([ordered]@{
        schema_version = 1
        failure_code = $FailureCode
        process_role = $Name
        direct_pid = [uint64]$Handle.Job.ProcessId
        direct_creation_file_time_utc = [uint64]$Handle.Job.CreationFileTimeUtc
        samples = @($Samples)
    })
}

function Test-AllowlistedBuildHelperSnapshot {
    param(
        [Parameter(Mandatory)][object] $Sample,
        [Parameter(Mandatory)][object] $Evidence,
        [bool] $AllowContractBuildHelper = $false
    )
    $rows = @($Sample.processes)
    $activeIds = @($Evidence.ActiveProcessIds | ForEach-Object { [uint64]$_ } | Sort-Object)
    $rowIds = @($rows | ForEach-Object { [uint64]$_.pid } | Sort-Object)
    if ($rows.Count -lt 1 -or $Sample.truncated -or
        $rows.Count -ne [int64]$Evidence.ActiveProcessCount -or
        $rowIds.Count -ne $activeIds.Count) {
        return $false
    }
    for ($index = 0; $index -lt $activeIds.Count; $index++) {
        if ($rowIds[$index] -ne $activeIds[$index]) { return $false }
    }
    foreach ($row in $rows) {
        if (-not $row.observed_in_initial_job_snapshot -or -not $row.job_member -or
            [uint64]$row.creation_file_time_utc -eq 0 -or
            [string]$row.query_status -cnotlike 'ok*') {
            return $false
        }
        $path = [System.IO.Path]::GetFullPath([string]$row.executable_path)
        $isContractPwsh = [string]$row.name -ceq 'pwsh' -and
            [string]::Equals(
                $path,
                [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName,
                [System.StringComparison]::OrdinalIgnoreCase)
        $contractConhostPath = Join-Path ([System.Environment]::GetFolderPath(
            [System.Environment+SpecialFolder]::System)) 'conhost.exe'
        $isContractConhost = [string]$row.name -ceq 'conhost' -and
            [string]::Equals(
                $path,
                $contractConhostPath,
                [System.StringComparison]::OrdinalIgnoreCase)
        $isContractHelper = $AllowContractBuildHelper -and
            ($isContractPwsh -or $isContractConhost)
        $isVctip = [string]$row.name -ceq 'vctip' -and
            $path -cmatch '(?i)\\Microsoft Visual Studio\\[^\\]+\\[^\\]+\\VC\\Tools\\MSVC\\[0-9.]+\\bin\\HostX64\\x64\\VCTIP\.EXE$'
        if (-not $isContractHelper -and -not $isVctip) { return $false }
        if ($isVctip) {
            try {
                $programFiles = [System.Environment]::GetFolderPath(
                    [System.Environment+SpecialFolder]::ProgramFiles)
                $helperParent = [System.IO.Path]::GetDirectoryName($path)
                [void](Assert-ExistingPathChainNoReparse -Root $programFiles -Candidate $helperParent `
                    -FailureCode 'build-helper-path-invalid')
                $item = Assert-RegularPath -LiteralPath $path -Kind Leaf `
                    -FailureCode 'build-helper-path-invalid'
                if (-not [string]::Equals(
                    $item.FullName,
                    $path,
                    [System.StringComparison]::OrdinalIgnoreCase)) {
                    return $false
                }
            }
            catch { return $false }
        }
    }
    return $true
}

function Get-CurrentBuildHelperEvidence {
    param(
        [Parameter(Mandatory)][object] $Handle,
        [Parameter(Mandatory)][object] $Sample
    )
    try {
        $freshEvidence = Get-DynamoIsolatedProcessEvidence -Process $Handle.Job
        $rows = @($Sample.processes)
        $freshIds = @($freshEvidence.ActiveProcessIds | ForEach-Object { [uint64]$_ } | Sort-Object)
        $rowIds = @($rows | ForEach-Object { [uint64]$_.pid } | Sort-Object)
        if ($freshIds.Count -ne $rowIds.Count -or
            $freshIds.Count -ne [int64]$freshEvidence.ActiveProcessCount) {
            return $null
        }
        for ($index = 0; $index -lt $freshIds.Count; $index++) {
            if ($freshIds[$index] -ne $rowIds[$index]) { return $null }
        }
        foreach ($row in $rows) {
            $process = [System.Diagnostics.Process]::GetProcessById([int]$row.pid)
            try {
                $birth = [uint64]$process.StartTime.ToUniversalTime().ToFileTimeUtc()
                $name = [string]$process.ProcessName
                $path = [System.IO.Path]::GetFullPath([string]$process.MainModule.FileName)
            }
            finally { $process.Dispose() }
            if ($birth -ne [uint64]$row.creation_file_time_utc -or
                $name -cne [string]$row.name -or
                -not [string]::Equals(
                    $path,
                    [string]$row.executable_path,
                    [System.StringComparison]::OrdinalIgnoreCase)) {
                return $null
            }
        }
        return $freshEvidence
    }
    catch { return $null }
}

function Wait-RunnerJob {
    param(
        [Parameter(Mandatory)][object] $Handle,
        [Parameter(Mandatory)][int] $TimeoutMilliseconds,
        [Parameter(Mandatory)][string] $FailureCode,
        [int[]] $AllowedExitCodes = @(0),
        [string] $DiagnosticPath,
        [string] $Name = 'child',
        [bool] $AllowContractBuildHelper = $false
    )
    $wait = Wait-DynamoIsolatedProcess -Process $Handle.Job -TimeoutMilliseconds $TimeoutMilliseconds
    if (-not $wait.Exited) {
        Stop-DynamoIsolatedProcess -Process $Handle.Job
        [void](Wait-DynamoIsolatedProcess -Process $Handle.Job -TimeoutMilliseconds 5000)
        Throw-RunnerFailure $FailureCode
    }
    if ($AllowedExitCodes -notcontains [int64]$wait.ExitCode) { Throw-RunnerFailure $FailureCode }
    $drainTimeoutMilliseconds = 5000
    $drainClock = [System.Diagnostics.Stopwatch]::StartNew()
    $diagnosticSamples = [System.Collections.Generic.List[object]]::new()
    $allowedHelperTerminated = $false
    $allowedHelperNames = @()
    while ($true) {
        $evidence = Get-DynamoIsolatedProcessEvidence -Process $Handle.Job
        if ($evidence.ProcessId -ne $Handle.Job.ProcessId -or
            $evidence.CreationFileTimeUtc -ne $Handle.Job.CreationFileTimeUtc -or
            -not $evidence.IsProcessInJob -or $evidence.TotalProcessCount -lt 1) {
            Throw-RunnerFailure 'job-evidence-invalid'
        }
        if ($evidence.ActiveProcessCount -eq 0 -and
            @($evidence.ActiveProcessIds).Count -eq 0) {
            break
        }
        if ($diagnosticSamples.Count -eq 0 -and -not [string]::IsNullOrEmpty($DiagnosticPath)) {
            try {
                $diagnosticSamples.Add((Get-SanitizedProcessSnapshot -Handle $Handle -Evidence $evidence `
                    -ObservedElapsedMilliseconds $drainClock.ElapsedMilliseconds -Phase 'direct-exit'))
            }
            catch { }
        }
        $remaining = $drainTimeoutMilliseconds - $drainClock.ElapsedMilliseconds
        if ($remaining -le 0) {
            $deadlineSample = $null
            try {
                $deadlineSample = Get-SanitizedProcessSnapshot -Handle $Handle -Evidence $evidence `
                    -ObservedElapsedMilliseconds $drainClock.ElapsedMilliseconds -Phase 'drain-deadline'
                if (Test-AllowlistedBuildHelperSnapshot -Sample $deadlineSample -Evidence $evidence `
                    -AllowContractBuildHelper $AllowContractBuildHelper) {
                    $freshHelperEvidence = Get-CurrentBuildHelperEvidence -Handle $Handle `
                        -Sample $deadlineSample
                    if ($null -ne $freshHelperEvidence) {
                        $allowedHelperNames = @($deadlineSample.processes | ForEach-Object { [string]$_.name } | Sort-Object -Unique)
                        Stop-DynamoIsolatedProcess -Process $Handle.Job
                        $cleanupClock = [System.Diagnostics.Stopwatch]::StartNew()
                        while ($true) {
                            $cleanupEvidence = Get-DynamoIsolatedProcessEvidence -Process $Handle.Job
                            if ($cleanupEvidence.ActiveProcessCount -eq 0 -and
                                @($cleanupEvidence.ActiveProcessIds).Count -eq 0) {
                                $evidence = $cleanupEvidence
                                $allowedHelperTerminated = $true
                                break
                            }
                            $cleanupRemaining = 5000 - $cleanupClock.ElapsedMilliseconds
                            if ($cleanupRemaining -le 0) { break }
                            Start-Sleep -Milliseconds ([int][Math]::Min(25, [Math]::Ceiling($cleanupRemaining)))
                        }
                        if ($allowedHelperTerminated) { break }
                    }
                }
            }
            catch { }
            if (-not [string]::IsNullOrEmpty($DiagnosticPath)) {
                try {
                    if ($null -ne $deadlineSample) { $diagnosticSamples.Add($deadlineSample) }
                    Write-DescendantDiagnostic -LiteralPath $DiagnosticPath -Handle $Handle `
                        -Name $Name -Samples @($diagnosticSamples.ToArray())
                }
                catch { }
            }
            Throw-RunnerFailure 'child-descendants-survived'
        }
        Start-Sleep -Milliseconds ([int][Math]::Min(25, [Math]::Ceiling($remaining)))
    }
    return [pscustomobject]@{
        ExitCode = [int64]$wait.ExitCode
        Stdout = Read-BoundedUtf8File -LiteralPath $Handle.StdoutPath -AllowEmpty
        Stderr = Read-BoundedUtf8File -LiteralPath $Handle.StderrPath -AllowEmpty
        Evidence = $evidence
        AllowedHelperTerminated = $allowedHelperTerminated
        AllowedHelperNames = @($allowedHelperNames)
    }
}

function Remove-RunnerJobHandle {
    param([AllowNull()][object] $Handle)
    if ($null -eq $Handle) { return }
    $cleanupFault = $false
    $exited = $false
    try {
        $wait = Wait-DynamoIsolatedProcess -Process $Handle.Job -TimeoutMilliseconds 0
        $exited = [bool]$wait.Exited
    }
    catch { $cleanupFault = $true }
    if (-not $exited) {
        try {
            Stop-DynamoIsolatedProcess -Process $Handle.Job
        }
        catch {
            $cleanupFault = $true
            try { $Handle.Job.Terminate([uint32]3758161936) }
            catch { Throw-RunnerFailure 'teardown-child-cleanup-failed' }
        }
        try {
            $terminated = Wait-DynamoIsolatedProcess -Process $Handle.Job -TimeoutMilliseconds 5000
            if (-not $terminated.Exited) { Throw-RunnerFailure 'teardown-child-cleanup-failed' }
            $exited = $true
        }
        catch {
            if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
            Throw-RunnerFailure 'teardown-child-cleanup-failed'
        }
    }
    $proofFault = $false
    try {
        $evidence = Get-DynamoIsolatedProcessEvidence -Process $Handle.Job
        if ($evidence.ActiveProcessCount -ne 0 -or $evidence.ActiveProcessIds.Count -ne 0) {
            try { Stop-DynamoIsolatedProcess -Process $Handle.Job }
            catch {
                $cleanupFault = $true
                $Handle.Job.Terminate([uint32]3758161936)
            }
            $deadline = [System.Diagnostics.Stopwatch]::StartNew()
            do {
                Start-Sleep -Milliseconds 25
                $evidence = Get-DynamoIsolatedProcessEvidence -Process $Handle.Job
            } while ($evidence.ActiveProcessCount -ne 0 -and $deadline.ElapsedMilliseconds -lt 5000)
        }
        if (-not $evidence.IsProcessInJob -or $evidence.ActiveProcessCount -ne 0 -or
            $evidence.ActiveProcessIds.Count -ne 0) {
            $proofFault = $true
        }
        Assert-OriginalProcessAbsent -ProcessId $Handle.Job.ProcessId `
            -CreationFileTimeUtc $Handle.Job.CreationFileTimeUtc
    }
    catch { $proofFault = $true }
    try {
        Remove-DynamoIsolatedProcess -Process $Handle.Job
    }
    catch {
        $cleanupFault = $true
        try { $Handle.Job.Dispose() } catch { Throw-RunnerFailure 'teardown-child-cleanup-failed' }
    }
    if ($cleanupFault -or $proofFault) { Throw-RunnerFailure 'teardown-child-cleanup-failed' }
}

function Remove-ChildLogs {
    param([AllowNull()][object] $Handle)
    if ($null -eq $Handle) { return }
    foreach ($path in @($Handle.StdoutPath, $Handle.StderrPath)) {
        if (Test-Path -LiteralPath $path) {
            Remove-Item -LiteralPath $path -Force
            if (Test-Path -LiteralPath $path) { Throw-RunnerFailure 'temporary-file-cleanup-failed' }
        }
    }
}

function Wait-ReadyFile {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [Parameter(Mandatory)][object] $HarnessHandle,
        [Parameter(Mandatory)][int] $TimeoutMilliseconds
    )
    $deadline = [System.Diagnostics.Stopwatch]::StartNew()
    while ($deadline.ElapsedMilliseconds -lt $TimeoutMilliseconds) {
        if (Test-Path -LiteralPath $LiteralPath) {
            return Read-ExactJsonFile -LiteralPath $LiteralPath -MaximumBytes 65536 `
                -FailureCode 'ready-file-invalid'
        }
        $wait = Wait-DynamoIsolatedProcess -Process $HarnessHandle.Job -TimeoutMilliseconds 0
        if ($wait.Exited) { Throw-RunnerFailure 'harness-exited-before-ready' }
        Start-Sleep -Milliseconds 50
    }
    Throw-RunnerFailure 'ready-timeout'
}

function Assert-ReadyFile {
    param(
        [Parameter(Mandatory)][object] $Ready,
        [Parameter(Mandatory)][string] $Revision,
        [Parameter(Mandatory)][string] $Nonce,
        [Parameter(Mandatory)][int] $ProcessId,
        [Parameter(Mandatory)][string] $FixtureMode
    )
    Assert-ExactJsonKeys -Value $Ready -Keys @(
        'schema_version', 'host', 'dynamic_port', 'port', 'pid', 'revision',
        'nonce', 'fixture_mode', 'fixture', 'guild_id', 'cookie_name', 'cookie_value'
    ) -FailureCode 'ready-schema-mismatch'
    Assert-ExactJsonKeys -Value $Ready.fixture -Keys @('version', 'sha256') `
        -FailureCode 'ready-fixture-schema-mismatch'
    if ($Ready.schema_version -ne 1 -or $Ready.host -cne '127.0.0.1' -or
        $Ready.dynamic_port -ne $true -or $Ready.port -lt 1 -or $Ready.port -gt 65535 -or
        $Ready.pid -ne $ProcessId -or $Ready.revision -cne $Revision -or
        $Ready.nonce -cne $Nonce -or $Ready.fixture_mode -cne $FixtureMode -or
        $Ready.fixture.version -cne $script:FixtureVersion -or
        $Ready.fixture.sha256 -cne $script:FixtureSha256 -or
        $Ready.guild_id -ne 9000000000000000101 -or
        $Ready.cookie_name -cne 'dynamo_dashboard_session' -or
        $Ready.cookie_value -isnot [string] -or
        $Ready.cookie_value -cnotmatch '^perf_[0-9a-f]{64}$' -or
        $Ready.cookie_value -ceq $Nonce -or $Ready.cookie_value -ceq "perf_$Nonce") {
        Throw-RunnerFailure 'ready-identity-mismatch'
    }
}

function Invoke-LoopbackJson {
    param(
        [Parameter(Mandatory)][System.Net.Http.HttpClient] $Client,
        [Parameter(Mandatory)][ValidateSet('GET', 'POST')][string] $Method,
        [Parameter(Mandatory)][int] $Port,
        [Parameter(Mandatory)][string] $Route,
        [string] $ControlToken,
        [int] $ExpectedStatus = 200,
        [switch] $NoBody
    )
    if ($Route -notmatch '^/[a-z0-9_./-]+$' -or $Route.Contains('?') -or $Route.Contains('#')) {
        Throw-RunnerFailure 'control-route-invalid'
    }
    $request = [System.Net.Http.HttpRequestMessage]::new(
        [System.Net.Http.HttpMethod]::new($Method),
        "http://127.0.0.1:$Port$Route")
    try {
        if ($null -ne $ControlToken) {
            [void]$request.Headers.TryAddWithoutValidation('x-dynamo-perf-control', $ControlToken)
        }
        $response = $Client.Send($request)
        try {
            if ([int]$response.StatusCode -ne $ExpectedStatus) { Throw-RunnerFailure 'control-status-mismatch' }
            if ($NoBody) { return $null }
            $body = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
            if ($body.Length -le 0 -or $body.Length -gt 65536) { Throw-RunnerFailure 'control-body-invalid' }
            try { return $body | ConvertFrom-Json -Depth 16 }
            catch { Throw-RunnerFailure 'control-json-invalid' }
        }
        finally { $response.Dispose() }
    }
    finally { $request.Dispose() }
}

function Assert-InstanceSnapshot {
    param(
        [Parameter(Mandatory)][object] $Instance,
        [Parameter(Mandatory)][string] $Revision,
        [Parameter(Mandatory)][string] $Nonce,
        [Parameter(Mandatory)][int] $ProcessId,
        [Parameter(Mandatory)][string] $FixtureMode
    )
    Assert-ExactJsonKeys -Value $Instance -Keys @(
        'schema_version', 'revision', 'nonce', 'pid', 'fixture_mode', 'fixture',
        'outbound_calls', 'browser_outbound_attempts'
    ) -FailureCode 'instance-schema-mismatch'
    Assert-ExactJsonKeys -Value $Instance.fixture -Keys @('version', 'sha256') `
        -FailureCode 'instance-fixture-schema-mismatch'
    if ($Instance.schema_version -ne 1 -or $Instance.revision -cne $Revision -or
        $Instance.nonce -cne $Nonce -or $Instance.pid -ne $ProcessId -or
        $Instance.fixture_mode -cne $FixtureMode -or
        $Instance.fixture.version -cne $script:FixtureVersion -or
        $Instance.fixture.sha256 -cne $script:FixtureSha256 -or
        $Instance.outbound_calls -ne 0 -or $Instance.browser_outbound_attempts -ne 0) {
        Throw-RunnerFailure 'instance-identity-or-counter-mismatch'
    }
}

function Assert-CounterSnapshot {
    param([Parameter(Mandatory)][object] $Counters)
    Assert-ExactJsonKeys -Value $Counters -Keys @(
        'schema_version', 'denied_requests', 'server_write_attempts', 'repository_reads',
        'repository_mutations', 'outbound_calls', 'browser_outbound_attempts',
        'provider_guild_lookups'
    ) -FailureCode 'counter-schema-mismatch'
    if ($Counters.schema_version -ne 1 -or $Counters.denied_requests -ne 0 -or
        $Counters.server_write_attempts -ne 0 -or
        $Counters.repository_reads -ne 0 -or $Counters.repository_mutations -ne 0 -or
        $Counters.outbound_calls -ne 0 -or
        $Counters.browser_outbound_attempts -ne 0 -or $Counters.provider_guild_lookups -ne 0) {
        Throw-RunnerFailure 'counter-drift-detected'
    }
}

function Assert-LoadResult {
    param(
        [Parameter(Mandatory)][object] $Result,
        [Parameter(Mandatory)][System.Collections.IDictionary] $SourceState,
        [Parameter(Mandatory)][System.Collections.IDictionary] $Environment,
        [Parameter(Mandatory)][string] $Nonce,
        [Parameter(Mandatory)][int] $ProcessId,
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][int] $Requests,
        [Parameter(Mandatory)][int] $Concurrency
    )
    Assert-ExactJsonKeys -Value $Result -Keys @(
        'schema_version', 'runner_version', 'source_state', 'fixture', 'environment',
        'instance', 'path', 'requests', 'concurrency', 'ok', 'failed', 'decoded_bytes',
        'wire_bytes', 'content_encodings', 'p50_ms', 'p95_ms', 'max_ms', 'statuses'
    ) -FailureCode 'load-result-schema-mismatch'
    if ($Result.schema_version -ne 1 -or $Result.runner_version -cne 'dashboard-load-v1' -or
        $Result.source_state.head -cne $SourceState.head -or
        $Result.source_state.clean -ne $true -or
        $Result.source_state.diff_sha256 -cne $SourceState.diff_sha256 -or
        $Result.fixture.version -cne $script:FixtureVersion -or
        $Result.fixture.sha256 -cne $script:FixtureSha256 -or
        $Result.environment.fingerprint_sha256 -cne $Environment.fingerprint_sha256 -or
        $Result.instance.revision -cne $SourceState.head -or
        $Result.instance.nonce -cne $Nonce -or $Result.instance.pid -ne $ProcessId -or
        $Result.instance.fixture_mode -cne 'Public' -or
        $Result.instance.outbound_calls_before -ne 0 -or
        $Result.instance.outbound_calls_after -ne 0 -or
        $Result.instance.browser_outbound_attempts -ne 0 -or
        $Result.path -cne $Path -or $Result.requests -ne $Requests -or
        $Result.concurrency -ne $Concurrency -or $Result.ok -ne $Requests -or
        $Result.failed -ne 0) {
        Throw-RunnerFailure 'load-result-identity-mismatch'
    }
}

function Assert-PortClosed {
    param([Parameter(Mandatory)][int] $Port)
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $Port)
    $listener.ExclusiveAddressUse = $true
    try { $listener.Start(1) }
    catch { Throw-RunnerFailure 'teardown-port-still-open' }
    finally { try { $listener.Stop() } catch { } }
}

function Assert-OriginalProcessAbsent {
    param(
        [Parameter(Mandatory)][int] $ProcessId,
        [Parameter(Mandatory)][uint64] $CreationFileTimeUtc
    )
    try {
        $candidate = [System.Diagnostics.Process]::GetProcessById($ProcessId)
        try {
            $candidateCreation = [uint64]$candidate.StartTime.ToUniversalTime().ToFileTimeUtc()
            if ($candidateCreation -eq $CreationFileTimeUtc) {
                Throw-RunnerFailure 'teardown-process-still-alive'
            }
        }
        finally { $candidate.Dispose() }
    }
    catch [System.ArgumentException] { }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure 'teardown-process-proof-failed'
    }
}

function Assert-NoReparseTree {
    param(
        [Parameter(Mandatory)][string] $TreeRoot,
        [Parameter(Mandatory)][string] $FailureCode
    )
    $rootFull = [System.IO.Path]::GetFullPath($TreeRoot)
    [void](Assert-RegularPath -LiteralPath $rootFull -Kind Container -FailureCode $FailureCode)
    $pending = [System.Collections.Generic.Queue[System.IO.DirectoryInfo]]::new()
    $pending.Enqueue([System.IO.DirectoryInfo]::new($rootFull))
    try {
        while ($pending.Count -gt 0) {
            $directory = $pending.Dequeue()
            foreach ($entry in $directory.EnumerateFileSystemInfos()) {
                $entryFull = Assert-PathUnderRoot -Root $rootFull -Candidate $entry.FullName `
                    -FailureCode $FailureCode
                $attributes = [System.IO.File]::GetAttributes($entryFull)
                if (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                    Throw-RunnerFailure $FailureCode
                }
                if (($attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
                    $pending.Enqueue([System.IO.DirectoryInfo]::new($entryFull))
                }
            }
        }
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure $FailureCode
    }
}

function Remove-OwnedAttempt {
    param(
        [Parameter(Mandatory)][string] $AttemptsRoot,
        [Parameter(Mandatory)][string] $AttemptDirectory,
        [Parameter(Mandatory)][string] $AttemptId,
        [Parameter(Mandatory)][string] $MarkerPath
    )
    if (-not (Test-Path -LiteralPath $AttemptDirectory)) { return }
    $full = Assert-PathUnderRoot -Root $AttemptsRoot -Candidate $AttemptDirectory `
        -FailureCode 'attempt-cleanup-containment-failed'
    if ((Split-Path -Leaf $full) -cne $AttemptId -or $AttemptId -cnotmatch '^[0-9a-f]{64}$') {
        Throw-RunnerFailure 'attempt-cleanup-identity-failed'
    }
    [void](Assert-RegularPath -LiteralPath $full -Kind Container `
        -FailureCode 'attempt-cleanup-directory-invalid')
    $marker = Read-ExactJsonFile -LiteralPath $MarkerPath -MaximumBytes 4096 `
        -FailureCode 'attempt-cleanup-marker-invalid'
    Assert-ExactJsonKeys -Value $marker -Keys @('schema_version', 'attempt_id', 'runner_version') `
        -FailureCode 'attempt-cleanup-marker-invalid'
    if ($marker.schema_version -ne 1 -or $marker.attempt_id -cne $AttemptId -or
        $marker.runner_version -cne $script:RunnerVersion) {
        Throw-RunnerFailure 'attempt-cleanup-marker-mismatch'
    }
    Assert-NoReparseTree -TreeRoot $full -FailureCode 'attempt-cleanup-reparse-detected'
    Remove-Item -LiteralPath $full -Recurse -Force
    if (Test-Path -LiteralPath $full) { Throw-RunnerFailure 'attempt-cleanup-readback-failed' }
}

function Remove-UnmarkedAttempt {
    param(
        [Parameter(Mandatory)][string] $AttemptsRoot,
        [Parameter(Mandatory)][string] $AttemptDirectory,
        [Parameter(Mandatory)][string] $AttemptId
    )
    $full = Assert-PathUnderRoot -Root $AttemptsRoot -Candidate $AttemptDirectory `
        -FailureCode 'attempt-cleanup-containment-failed'
    if ((Split-Path -Leaf $full) -cne $AttemptId -or $AttemptId -cnotmatch '^[0-9a-f]{64}$') {
        Throw-RunnerFailure 'attempt-cleanup-identity-failed'
    }
    [void](Assert-RegularPath -LiteralPath $full -Kind Container `
        -FailureCode 'attempt-cleanup-directory-invalid')
    foreach ($child in @(Get-ChildItem -LiteralPath $full -Force)) {
        if ($child -isnot [System.IO.FileInfo] -or
            ($child.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
            $child.Name -cnotmatch '^\.publish-[0-9a-f]{32}\.tmp$') {
            Throw-RunnerFailure 'attempt-cleanup-unprovable'
        }
        Remove-Item -LiteralPath $child.FullName -Force
    }
    Remove-Item -LiteralPath $full -Force
    if (Test-Path -LiteralPath $full) { Throw-RunnerFailure 'attempt-cleanup-readback-failed' }
}

$attemptCreated = $false
$attemptSucceeded = $false
$attemptsRoot = $null
$attemptDirectory = $null
$attemptId = $null
$markerPath = $null
$allHandles = [System.Collections.Generic.List[object]]::new()
$harnessHandle = $null
$httpClient = $null
$finalOutput = $null
$failureCode = $null
$contractMode = $false
$jobEvidenceRecords = [System.Collections.Generic.List[object]]::new()
$teardownPort = $null
$diagnosticEnabled = $false
$diagnosticsRoot = $null
$buildDiagnosticPath = $null
$launchDiagnosticPaths = @{}
$descendantDiagnosticPaths = @{}

try {
    if ($PSVersionTable.PSVersion -lt [version]'7.4') { Throw-RunnerFailure 'powershell-version-unsupported' }
    if (-not $IsWindows) { Throw-RunnerFailure 'windows-required' }
    if (@('Public', 'GuildDetail', 'ReadOnly') -cnotcontains $FixtureMode -or
        @('Load', 'Npm', 'Playwright') -cnotcontains $Workload -or
        @('output/perf', 'output/playwright') -cnotcontains $OutputRoot -or
        $Label -cnotmatch '^[a-z0-9][a-z0-9-]{0,63}$' -or
        $Path.Length -lt 1 -or $Path.Length -gt 2048 -or -not $Path.StartsWith('/') -or
        $Path.StartsWith('//') -or $Path.Contains('?') -or $Path.Contains('#') -or
        $Path -match '[\r\n]' -or $Requests -lt 1 -or $Requests -gt 1000000 -or
        $Concurrency -lt 1 -or $Concurrency -gt 1024 -or $Concurrency -gt $Requests -or
        $null -eq $Spec -or
        $null -eq $Project -or @('Pass', 'Red') -cnotcontains $ExpectedOutcome) {
        Throw-RunnerFailure 'parameter-value-invalid'
    }
    if ($PSBoundParameters.ContainsKey('ExpectedFailureId') -and
        @(
            'public-responsive-reduced-motion',
            'guild-readonly-dialog',
            'same-origin-font-proof'
        ) -cnotcontains $ExpectedFailureId) {
        Throw-RunnerFailure 'parameter-value-invalid'
    }
    if ($FixtureMode -cne 'Public' -or $Workload -cne 'Load' -or $OutputRoot -cne 'output/perf') {
        Throw-RunnerFailure 'checkpoint-mode-not-implemented'
    }
    if ($PSBoundParameters.ContainsKey('Spec') -or $PSBoundParameters.ContainsKey('Project') -or
        $PSBoundParameters.ContainsKey('ExpectedOutcome') -or
        $PSBoundParameters.ContainsKey('ExpectedFailureId')) {
        Throw-RunnerFailure 'load-browser-arguments-forbidden'
    }
    foreach ($name in @(
        'PERF_BASE_URL', 'PERF_COOKIE', 'PERF_HEADERS', 'PERF_STORAGE_STATE',
        'PERF_INSTANCE_HANDOFF', 'PERF_PATH', 'PERF_REQUESTS', 'PERF_CONCURRENCY', 'PERF_OUT',
        'PLAYWRIGHT_BASE_URL', 'PLAYWRIGHT_STORAGE_STATE', 'PLAYWRIGHT_JSON_OUTPUT_NAME',
        'PLAYWRIGHT_HTML_REPORT', 'PLAYWRIGHT_OUTPUT_DIR',
        'DYNAMO_PERF_REVISION', 'DYNAMO_PERF_NONCE', 'DYNAMO_PERF_FIXTURE_MODE',
        'DYNAMO_PERF_FIXTURE_VERSION', 'DYNAMO_PERF_FIXTURE_SHA256', 'DYNAMO_PERF_READY_FILE',
        'DYNAMO_PERF_HOST', 'DYNAMO_PERF_PORT', 'DYNAMO_PERF_BASE_URL'
    )) {
        if ($null -ne [System.Environment]::GetEnvironmentVariable($name, 'Process')) {
            Throw-RunnerFailure 'caller-environment-forbidden'
        }
    }

    $repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
    [void](Assert-RegularPath -LiteralPath $repositoryRoot -Kind Container -FailureCode 'repository-root-invalid')
    $gitPath = Resolve-Executable -Name 'git.exe' -FailureCode 'git-unavailable'
    $reportedRoot = (Invoke-Git -GitPath $gitPath -RepositoryRoot $repositoryRoot `
        -Arguments @('rev-parse', '--show-toplevel') -FailureCode 'repository-root-unavailable').Stdout.Trim()
    if ([System.IO.Path]::GetFullPath($reportedRoot).TrimEnd('\') -cne $repositoryRoot.TrimEnd('\')) {
        Throw-RunnerFailure 'repository-root-mismatch'
    }

    $contractMode = [System.Environment]::GetEnvironmentVariable('DYNAMO_PERF_CONTRACT_MODE', 'Process') -ceq '1'
    $contractScenario = [System.Environment]::GetEnvironmentVariable('DYNAMO_PERF_CONTRACT_SCENARIO', 'Process')
    if ($contractMode) {
        $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\')
        [void](Assert-PathUnderRoot -Root $tempRoot -Candidate $repositoryRoot `
            -FailureCode 'contract-repository-not-temporary')
        $contractMarker = Join-Path $repositoryRoot '.dynamo-perf-contract-v1'
        [void](Assert-RegularPath -LiteralPath $contractMarker -Kind Leaf `
            -FailureCode 'contract-marker-invalid')
        if ([System.IO.File]::ReadAllText($contractMarker, $script:Utf8NoBom) -cne $script:ContractMarkerBody) {
            Throw-RunnerFailure 'contract-marker-invalid'
        }
        [void](Invoke-Git -GitPath $gitPath -RepositoryRoot $repositoryRoot `
            -Arguments @('ls-files', '--error-unmatch', '--', '.dynamo-perf-contract-v1') `
            -FailureCode 'contract-marker-untracked')
        if ([string]::IsNullOrEmpty($contractScenario)) { $contractScenario = 'success' }
        $knownContractScenario = @(
            'success', 'wrong-ready-nonce', 'counters-drift', 'budget-fail',
            'ready-timeout', 'shutdown-fail', 'cleanup-junction'
        ) -ccontains $contractScenario
        $descendantContractScenario = $contractScenario -cmatch `
            '^(short|long|allowlisted)-job-descendant-[0-9a-f]{32}$'
        $harnessDescendantContractScenario = $contractScenario -cmatch `
            '^harness-(short|long)-descendant-[0-9a-f]{32}$'
        if (-not $knownContractScenario -and -not $descendantContractScenario -and
            -not $harnessDescendantContractScenario) {
            Throw-RunnerFailure 'contract-scenario-invalid'
        }
    }
    elseif ($null -ne $contractScenario) {
        Throw-RunnerFailure 'contract-scenario-without-mode'
    }
    $diagnosticSetting = [System.Environment]::GetEnvironmentVariable(
        'DYNAMO_PERF_DIAGNOSTICS', 'Process')
    if ($null -ne $diagnosticSetting -and $diagnosticSetting -cne '1') {
        Throw-RunnerFailure 'diagnostic-setting-invalid'
    }
    $diagnosticEnabled = $diagnosticSetting -ceq '1'

    $sourceState = Get-SourceSnapshot -GitPath $gitPath -RepositoryRoot $repositoryRoot

    $modulePath = Join-Path $repositoryRoot 'scripts\perf\isolated-process-job.psm1'
    $fixturePath = Join-Path $repositoryRoot 'tests\perf\fixtures\guild-detail-v1.json'
    $loadScriptPath = Join-Path $repositoryRoot 'scripts\perf\dashboard-load.cjs'
    $budgetScriptPath = Join-Path $repositoryRoot 'scripts\perf\assert-budgets.cjs'
    $budgetPath = Join-Path $repositoryRoot 'tests\perf\budgets\public-root.json'
    foreach ($leaf in @($modulePath, $fixturePath, $loadScriptPath, $budgetScriptPath, $budgetPath)) {
        [void](Assert-RegularPath -LiteralPath $leaf -Kind Leaf -FailureCode 'required-runner-file-invalid')
    }
    if ((Get-FileSha256Hex -LiteralPath $fixturePath) -cne $script:FixtureSha256) {
        Throw-RunnerFailure 'fixture-hash-mismatch'
    }
    foreach ($trackedRunnerPath in @(
        'scripts/perf/with-isolated-dashboard.ps1',
        'scripts/perf/isolated-process-job.psm1',
        'scripts/perf/dashboard-load.cjs',
        'scripts/perf/assert-budgets.cjs',
        'tests/perf/budgets/public-root.json',
        'tests/perf/fixtures/guild-detail-v1.json'
    )) {
        [void](Invoke-Git -GitPath $gitPath -RepositoryRoot $repositoryRoot `
            -Arguments @('ls-files', '--error-unmatch', '--', $trackedRunnerPath) `
            -FailureCode 'required-runner-file-untracked')
    }
    $fixedBudget = Read-ExactJsonFile -LiteralPath $budgetPath -MaximumBytes 4096 `
        -FailureCode 'public-budget-invalid'
    Assert-ExactJsonKeys -Value $fixedBudget -Keys @(
        'max_p95_ms', 'max_failed', 'max_decoded_bytes_per_request'
    ) -FailureCode 'public-budget-invalid'
    if ($fixedBudget.max_p95_ms -ne 10 -or $fixedBudget.max_failed -ne 0 -or
        $fixedBudget.max_decoded_bytes_per_request -ne 27041) {
        Throw-RunnerFailure 'public-budget-invalid'
    }

    $ignore = Invoke-Git -GitPath $gitPath -RepositoryRoot $repositoryRoot `
        -Arguments @('check-ignore', '-q', '--', 'output/perf/.dynamo-perf-probe') `
        -AllowedExitCodes @(0, 1) -FailureCode 'output-ignore-check-failed'
    if ($ignore.ExitCode -ne 0) { Throw-RunnerFailure 'output-root-not-ignored' }

    $pwshPath = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
    [void](Assert-RegularPath -LiteralPath $pwshPath -Kind Leaf -FailureCode 'pwsh-executable-invalid')
    $nodePath = $null
    $cargoPath = $null
    $rustcPath = $null
    $contractStubPath = $null
    if ($contractMode) {
        $contractStubPath = Join-Path $repositoryRoot 'tests\perf\contract-child.ps1'
        [void](Assert-RegularPath -LiteralPath $contractStubPath -Kind Leaf -FailureCode 'contract-child-invalid')
        [void](Invoke-Git -GitPath $gitPath -RepositoryRoot $repositoryRoot `
            -Arguments @('ls-files', '--error-unmatch', '--', 'tests/perf/contract-child.ps1') `
            -FailureCode 'contract-child-untracked')
    }
    else {
        $nodePath = Resolve-Executable -Name 'node.exe' -FailureCode 'node-unavailable'
        $cargoPath = Resolve-Executable -Name 'cargo.exe' -FailureCode 'cargo-unavailable'
        $rustcPath = Resolve-Executable -Name 'rustc.exe' -FailureCode 'rustc-unavailable'
    }
    $environmentIdentity = Get-EnvironmentIdentity -ContractMode $contractMode `
        -NodePath $nodePath -RustcPath $rustcPath -RepositoryRoot $repositoryRoot

    Import-Module -Name $modulePath -Force -ErrorAction Stop

    $outputRootAbsolute = Assert-PathUnderRoot -Root $repositoryRoot `
        -Candidate (Join-Path $repositoryRoot ($OutputRoot -replace '/', '\')) `
        -FailureCode 'output-root-escape'
    [void](New-OwnedDirectoryChain -Root $repositoryRoot -Candidate $outputRootAbsolute `
        -FailureCode 'output-root-invalid')
    Assert-DirectoryOwnedByCurrentUser -LiteralPath $outputRootAbsolute `
        -FailureCode 'output-root-owner-invalid'
    Set-ProtectedAttemptAcl -AttemptDirectory $outputRootAbsolute
    if ($diagnosticEnabled) {
        $diagnosticsRoot = Join-Path $outputRootAbsolute 'diagnostics'
        [void](New-OwnedDirectoryChain -Root $repositoryRoot -Candidate $diagnosticsRoot `
            -FailureCode 'diagnostics-root-invalid')
        Assert-DirectoryOwnedByCurrentUser -LiteralPath $diagnosticsRoot `
            -FailureCode 'diagnostics-root-owner-invalid'
        Set-ProtectedAttemptAcl -AttemptDirectory $diagnosticsRoot
    }
    $attemptsRoot = Join-Path $outputRootAbsolute 'attempts'
    [void](New-OwnedDirectoryChain -Root $repositoryRoot -Candidate $attemptsRoot `
        -FailureCode 'attempts-root-invalid')
    Assert-DirectoryOwnedByCurrentUser -LiteralPath $attemptsRoot `
        -FailureCode 'attempts-root-owner-invalid'
    Set-ProtectedAttemptAcl -AttemptDirectory $attemptsRoot

    for ($attempt = 0; $attempt -lt 8 -and -not $attemptCreated; $attempt++) {
        $attemptId = Get-RandomHex -ByteCount 32
        $attemptDirectory = Join-Path $attemptsRoot $attemptId
        $attemptCreated = New-ExclusiveDirectoryNative -LiteralPath $attemptDirectory
    }
    if (-not $attemptCreated) { Throw-RunnerFailure 'attempt-allocation-failed' }
    if ($diagnosticEnabled) {
        $buildDiagnosticPath = Join-Path $diagnosticsRoot "$attemptId-build-descendants.json"
        foreach ($role in @('build', 'harness', 'load', 'budget')) {
            $launchDiagnosticPaths[$role] = Join-Path $diagnosticsRoot `
                "$attemptId-$role-launch.json"
        }
        foreach ($role in @('harness', 'load', 'budget')) {
            $descendantDiagnosticPaths[$role] = Join-Path $diagnosticsRoot `
                "$attemptId-$role-descendants.json"
        }
        foreach ($candidate in @($buildDiagnosticPath) + @($launchDiagnosticPaths.Values) +
            @($descendantDiagnosticPaths.Values)) {
            if (Test-Path -LiteralPath $candidate) {
                Throw-RunnerFailure 'diagnostic-leaf-exists'
            }
        }
    }
    [void](Assert-RegularPath -LiteralPath $attemptDirectory -Kind Container `
        -FailureCode 'attempt-directory-invalid')
    Set-ProtectedAttemptAcl -AttemptDirectory $attemptDirectory
    $markerPath = Join-Path $attemptDirectory '.dynamo-perf-attempt-v1.json'
    Write-ExclusiveJson -LiteralPath $markerPath -Value ([ordered]@{
        schema_version = 1
        attempt_id = $attemptId
        runner_version = $script:RunnerVersion
    })

    $baseEnvironment = Get-MinimalChildEnvironment
    if ($contractMode) {
        $buildExecutable = $pwshPath
        $buildArguments = @('-NoProfile', '-NonInteractive', '-File', $contractStubPath, '-Operation', 'Build')
        $buildEnvironment = [ordered]@{} + $baseEnvironment
        $buildEnvironment['DYNAMO_PERF_CONTRACT_SCENARIO'] = $contractScenario
        $buildEnvironment['DYNAMO_PERF_BUILD_REVISION'] = [string]$sourceState.head
    }
    else {
        $targetRoot = Join-Path $repositoryRoot ("target\perf-harness\$($sourceState.head)")
        [void](New-OwnedDirectoryChain -Root $repositoryRoot -Candidate $targetRoot `
            -FailureCode 'cargo-target-invalid')
        $buildExecutable = $cargoPath
        $buildArguments = @(
            'build', '--locked', '--release', '-p', 'dynamo-dashboard',
            '--features', 'perf-harness', '--bin', 'dynamo-dashboard-perf-harness'
        )
        $buildEnvironment = [ordered]@{} + $baseEnvironment
        $buildEnvironment['CARGO_TARGET_DIR'] = $targetRoot
        $buildEnvironment['DYNAMO_PERF_BUILD_REVISION'] = [string]$sourceState.head
    }
    $buildHandle = Start-RunnerJob -ExecutablePath $buildExecutable -ArgumentList $buildArguments `
        -WorkingDirectory $repositoryRoot -Environment $buildEnvironment `
        -AttemptDirectory $attemptDirectory -Name 'build' `
        -LaunchDiagnosticPath $launchDiagnosticPaths['build']
    $allHandles.Add($buildHandle)
    $jobEvidenceRecords.Add((Get-JobEvidenceRow -Name 'build' -Phase 'started' `
        -Handle $buildHandle))
    $buildTimeout = if ($contractMode) { 10000 } else { 600000 }
    $buildExecution = Wait-RunnerJob -Handle $buildHandle -TimeoutMilliseconds $buildTimeout `
        -FailureCode 'build-failed' -DiagnosticPath $buildDiagnosticPath -Name 'build' `
        -AllowContractBuildHelper ($contractMode -and
            $contractScenario -cmatch '^allowlisted-job-descendant-[0-9a-f]{32}$')
    $jobEvidenceRecords.Add((Get-JobEvidenceRow -Name 'build' -Phase 'exited' `
        -Handle $buildHandle -Evidence $buildExecution.Evidence))
    Remove-RunnerJobHandle -Handle $buildHandle
    $allHandles.Remove($buildHandle) | Out-Null
    Remove-ChildLogs -Handle $buildHandle

    $afterBuildSource = Get-SourceSnapshot -GitPath $gitPath -RepositoryRoot $repositoryRoot
    Assert-SnapshotEqual -Expected $sourceState -Actual $afterBuildSource

    if ($contractMode) {
        $harnessExecutable = $pwshPath
        $harnessArguments = @('-NoProfile', '-NonInteractive', '-File', $contractStubPath, '-Operation', 'Harness')
    }
    else {
        $harnessExecutable = Join-Path $targetRoot 'release\dynamo-dashboard-perf-harness.exe'
        [void](Assert-RegularPath -LiteralPath $harnessExecutable -Kind Leaf `
            -FailureCode 'harness-executable-missing')
        $harnessArguments = @()
    }
    $harnessExecutableItem = Assert-RegularPath -LiteralPath $harnessExecutable -Kind Leaf `
        -FailureCode 'harness-executable-invalid'
    $harnessExecutableSha256 = Get-FileSha256Hex -LiteralPath $harnessExecutable

    $nonce = Get-RandomHex -ByteCount 32
    $readyPath = Join-Path $attemptDirectory '.ready.json.tmp'
    $handoffPath = Join-Path $attemptDirectory '.handoff.json.tmp'
    $resultPath = Join-Path $attemptDirectory ("$Label-result.json")
    $reportPath = Join-Path $attemptDirectory ("$Label-budget.json")
    $summaryPath = Join-Path $attemptDirectory ("$Label-summary.json")
    $harnessEnvironment = [ordered]@{} + $baseEnvironment
    $harnessEnvironment['DYNAMO_PERF_REVISION'] = [string]$sourceState.head
    $harnessEnvironment['DYNAMO_PERF_NONCE'] = $nonce
    $harnessEnvironment['DYNAMO_PERF_FIXTURE_MODE'] = $FixtureMode
    $harnessEnvironment['DYNAMO_PERF_FIXTURE_VERSION'] = $script:FixtureVersion
    $harnessEnvironment['DYNAMO_PERF_FIXTURE_SHA256'] = $script:FixtureSha256
    $harnessEnvironment['DYNAMO_PERF_READY_FILE'] = $readyPath
    if ($contractMode) { $harnessEnvironment['DYNAMO_PERF_CONTRACT_SCENARIO'] = $contractScenario }

    $harnessHandle = Start-RunnerJob -ExecutablePath $harnessExecutable -ArgumentList $harnessArguments `
        -WorkingDirectory $repositoryRoot -Environment $harnessEnvironment `
        -AttemptDirectory $attemptDirectory -Name 'harness' `
        -LaunchDiagnosticPath $launchDiagnosticPaths['harness']
    $allHandles.Add($harnessHandle)
    if ((Get-FileSha256Hex -LiteralPath $harnessExecutable) -cne $harnessExecutableSha256) {
        Throw-RunnerFailure 'harness-executable-drift'
    }
    $jobEvidenceRecords.Add((Get-JobEvidenceRow -Name 'harness' -Phase 'started' `
        -Handle $harnessHandle))
    $readyTimeout = if ($contractMode) { 3000 } else { 30000 }
    $ready = Wait-ReadyFile -LiteralPath $readyPath -HarnessHandle $harnessHandle `
        -TimeoutMilliseconds $readyTimeout
    if ($ready.PSObject.Properties.Name -contains 'port' -and
        $ready.port -is [int64] -and $ready.port -ge 1 -and $ready.port -le 65535) {
        $teardownPort = [int]$ready.port
    }
    Assert-ReadyFile -Ready $ready -Revision $sourceState.head -Nonce $nonce `
        -ProcessId $harnessHandle.Job.ProcessId -FixtureMode $FixtureMode
    $harnessEvidence = Get-DynamoIsolatedProcessEvidence -Process $harnessHandle.Job
    if (-not $harnessEvidence.IsProcessInJob -or
        $harnessEvidence.CreationFileTimeUtc -ne $harnessHandle.Job.CreationFileTimeUtc -or
        $harnessEvidence.ActiveProcessIds -notcontains [uint64]$ready.pid) {
        Throw-RunnerFailure 'harness-job-identity-mismatch'
    }
    $port = [int]$ready.port
    $controlToken = [string]$ready.cookie_value
    $rssAfterReady = Get-HarnessRssBytes -HarnessHandle $harnessHandle
    $jobEvidenceRecords.Add((Get-JobEvidenceRow -Name 'harness' -Phase 'ready' `
        -Handle $harnessHandle))
    Remove-Item -LiteralPath $readyPath -Force
    if (Test-Path -LiteralPath $readyPath) { Throw-RunnerFailure 'ready-file-cleanup-failed' }

    $handler = [System.Net.Http.HttpClientHandler]::new()
    $handler.UseProxy = $false
    $handler.AllowAutoRedirect = $false
    $httpClient = [System.Net.Http.HttpClient]::new($handler, $true)
    $httpClient.Timeout = [TimeSpan]::FromSeconds(3)

    $instanceBefore = Invoke-LoopbackJson -Client $httpClient -Method GET -Port $port `
        -Route '/__perf/instance'
    Assert-InstanceSnapshot -Instance $instanceBefore -Revision $sourceState.head -Nonce $nonce `
        -ProcessId $harnessHandle.Job.ProcessId -FixtureMode $FixtureMode
    $countersBefore = Invoke-LoopbackJson -Client $httpClient -Method GET -Port $port `
        -Route '/__perf/counters'
    Assert-CounterSnapshot -Counters $countersBefore

    $issuedAt = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $handoff = [ordered]@{
        schema_version = 1
        issued_at_unix_ms = $issuedAt
        expires_at_unix_ms = $issuedAt + 600000
        revision = $sourceState.head
        nonce = $nonce
        pid = $harnessHandle.Job.ProcessId
        host = '127.0.0.1'
        port = $port
        dynamic_port = $true
        fixture_mode = $FixtureMode
        fixture = [ordered]@{ version = $script:FixtureVersion; sha256 = $script:FixtureSha256 }
        source_state = $sourceState
        environment = $environmentIdentity
    }
    Write-ExclusiveJson -LiteralPath $handoffPath -Value $handoff

    $loadEnvironment = [ordered]@{} + $baseEnvironment
    $loadEnvironment['PERF_INSTANCE_HANDOFF'] = $handoffPath
    $loadEnvironment['PERF_PATH'] = $Path
    $loadEnvironment['PERF_REQUESTS'] = [string]$Requests
    $loadEnvironment['PERF_CONCURRENCY'] = [string]$Concurrency
    $loadEnvironment['PERF_OUT'] = $resultPath
    if ($contractMode) {
        $loadEnvironment['DYNAMO_PERF_CONTRACT_SCENARIO'] = $contractScenario
        $loadExecutable = $pwshPath
        $loadArguments = @('-NoProfile', '-NonInteractive', '-File', $contractStubPath, '-Operation', 'Load')
    }
    else {
        $loadExecutable = $nodePath
        $loadArguments = @($loadScriptPath)
    }
    $loadHandle = Start-RunnerJob -ExecutablePath $loadExecutable -ArgumentList $loadArguments `
        -WorkingDirectory $repositoryRoot -Environment $loadEnvironment `
        -AttemptDirectory $attemptDirectory -Name 'load' `
        -LaunchDiagnosticPath $launchDiagnosticPaths['load']
    $allHandles.Add($loadHandle)
    $jobEvidenceRecords.Add((Get-JobEvidenceRow -Name 'load' -Phase 'started' `
        -Handle $loadHandle))
    $loadExecution = Wait-RunnerJob -Handle $loadHandle -TimeoutMilliseconds 120000 `
        -FailureCode 'load-failed' -DiagnosticPath $descendantDiagnosticPaths['load'] `
        -Name 'load'
    $jobEvidenceRecords.Add((Get-JobEvidenceRow -Name 'load' -Phase 'exited' `
        -Handle $loadHandle -Evidence $loadExecution.Evidence))
    if ($loadExecution.Stderr.Length -ne 0) { Throw-RunnerFailure 'load-stderr-not-empty' }
    $loadEnvelope = ConvertFrom-ExactJsonLine -Body $loadExecution.Stdout `
        -FailureCode 'load-stdout-invalid'
    Assert-ExactJsonKeys -Value $loadEnvelope -Keys @('schema_version', 'result_path', 'exit_code') `
        -FailureCode 'load-stdout-schema-mismatch'
    if ($loadEnvelope.schema_version -ne 1 -or $loadEnvelope.result_path -cne $resultPath -or
        $loadEnvelope.exit_code -ne 0) { Throw-RunnerFailure 'load-stdout-identity-mismatch' }
    Remove-RunnerJobHandle -Handle $loadHandle
    $allHandles.Remove($loadHandle) | Out-Null
    Remove-ChildLogs -Handle $loadHandle
    $result = Read-ExactJsonFile -LiteralPath $resultPath -FailureCode 'load-result-invalid'
    Assert-LoadResult -Result $result -SourceState $sourceState -Environment $environmentIdentity `
        -Nonce $nonce -ProcessId $harnessHandle.Job.ProcessId -Path $Path `
        -Requests $Requests -Concurrency $Concurrency
    $rssAfterLoad = Get-HarnessRssBytes -HarnessHandle $harnessHandle
    $jobEvidenceRecords.Add((Get-JobEvidenceRow -Name 'harness' -Phase 'after_load' `
        -Handle $harnessHandle))

    Remove-Item -LiteralPath $handoffPath -Force
    if (Test-Path -LiteralPath $handoffPath) { Throw-RunnerFailure 'handoff-cleanup-failed' }

    $budgetEnvironment = [ordered]@{} + $baseEnvironment
    if ($contractMode) {
        $budgetEnvironment['DYNAMO_PERF_CONTRACT_SCENARIO'] = $contractScenario
        $budgetExecutable = $pwshPath
        $budgetArguments = @(
            '-NoProfile', '-NonInteractive', '-File', $contractStubPath,
            '-Operation', 'Budget', '-Current', $resultPath, '-Budget', $budgetPath
        )
    }
    else {
        $budgetExecutable = $nodePath
        $budgetArguments = @($budgetScriptPath, $resultPath, $budgetPath)
    }
    $budgetHandle = Start-RunnerJob -ExecutablePath $budgetExecutable `
        -ArgumentList $budgetArguments -WorkingDirectory $repositoryRoot `
        -Environment $budgetEnvironment -AttemptDirectory $attemptDirectory -Name 'budget' `
        -LaunchDiagnosticPath $launchDiagnosticPaths['budget']
    $allHandles.Add($budgetHandle)
    $jobEvidenceRecords.Add((Get-JobEvidenceRow -Name 'budget' -Phase 'started' `
        -Handle $budgetHandle))
    $budgetExecution = Wait-RunnerJob -Handle $budgetHandle -TimeoutMilliseconds 120000 `
        -FailureCode 'budget-failed' -DiagnosticPath $descendantDiagnosticPaths['budget'] `
        -Name 'budget'
    $jobEvidenceRecords.Add((Get-JobEvidenceRow -Name 'budget' -Phase 'exited' `
        -Handle $budgetHandle -Evidence $budgetExecution.Evidence))
    if ($budgetExecution.Stderr.Length -ne 0) { Throw-RunnerFailure 'budget-stderr-not-empty' }
    $budgetDecision = ConvertFrom-ExactJsonLine -Body $budgetExecution.Stdout `
        -FailureCode 'budget-stdout-invalid'
    Assert-ExactJsonKeys -Value $budgetDecision -Keys @('schema_version', 'passed', 'checks') `
        -FailureCode 'budget-schema-mismatch'
    if ($budgetDecision.schema_version -ne 1 -or $budgetDecision.passed -ne $true -or
        $budgetDecision.checks.Count -lt 1) { Throw-RunnerFailure 'budget-decision-failed' }
    Remove-RunnerJobHandle -Handle $budgetHandle
    $allHandles.Remove($budgetHandle) | Out-Null
    Remove-ChildLogs -Handle $budgetHandle
    Write-ExclusiveJson -LiteralPath $reportPath -Value $budgetDecision

    $instanceAfter = Invoke-LoopbackJson -Client $httpClient -Method GET -Port $port `
        -Route '/__perf/instance'
    Assert-InstanceSnapshot -Instance $instanceAfter -Revision $sourceState.head -Nonce $nonce `
        -ProcessId $harnessHandle.Job.ProcessId -FixtureMode $FixtureMode
    $countersAfter = Invoke-LoopbackJson -Client $httpClient -Method GET -Port $port `
        -Route '/__perf/counters'
    Assert-CounterSnapshot -Counters $countersAfter
    foreach ($key in @(
        'denied_requests', 'server_write_attempts', 'repository_reads', 'repository_mutations',
        'outbound_calls', 'browser_outbound_attempts', 'provider_guild_lookups'
    )) {
        if ([int64]$countersBefore.$key -ne [int64]$countersAfter.$key) {
            Throw-RunnerFailure 'counter-drift-detected'
        }
    }
    $rssBeforeShutdown = Get-HarnessRssBytes -HarnessHandle $harnessHandle
    $jobEvidenceRecords.Add((Get-JobEvidenceRow -Name 'harness' -Phase 'before_shutdown' `
        -Handle $harnessHandle))

    [void](Invoke-LoopbackJson -Client $httpClient -Method POST -Port $port `
        -Route '/__perf/shutdown' -ControlToken $controlToken -ExpectedStatus 204 -NoBody)
    $shutdownWait = Wait-DynamoIsolatedProcess -Process $harnessHandle.Job -TimeoutMilliseconds 10000
    if (-not $shutdownWait.Exited -or [int64]$shutdownWait.ExitCode -ne 0) {
        Throw-RunnerFailure 'graceful-shutdown-failed'
    }
    $shutdownDrainClock = [System.Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        $closedEvidence = Get-DynamoIsolatedProcessEvidence -Process $harnessHandle.Job
        if ($closedEvidence.ActiveProcessCount -eq 0 -and
            @($closedEvidence.ActiveProcessIds).Count -eq 0) {
            break
        }
        $shutdownDrainRemaining = 5000 - $shutdownDrainClock.ElapsedMilliseconds
        if ($shutdownDrainRemaining -le 0) { break }
        Start-Sleep -Milliseconds ([int][Math]::Min(
            25,
            [Math]::Ceiling($shutdownDrainRemaining)))
    }
    if ($closedEvidence.ActiveProcessCount -ne 0 -or
        @($closedEvidence.ActiveProcessIds).Count -ne 0) {
        if ($diagnosticEnabled) {
            try {
                $harnessShutdownSample = Get-SanitizedProcessSnapshot -Handle $harnessHandle `
                    -Evidence $closedEvidence `
                    -ObservedElapsedMilliseconds $shutdownDrainClock.ElapsedMilliseconds `
                    -Phase 'graceful-shutdown'
                Write-DescendantDiagnostic `
                    -LiteralPath $descendantDiagnosticPaths['harness'] `
                    -Handle $harnessHandle -Name 'harness' `
                    -Samples @($harnessShutdownSample) `
                    -FailureCode 'teardown-descendants-survived'
            }
            catch { }
        }
        Throw-RunnerFailure 'teardown-descendants-survived'
    }
    $jobEvidenceRecords.Add((Get-JobEvidenceRow -Name 'harness' -Phase 'exited' `
        -Handle $harnessHandle -Evidence $closedEvidence))
    Assert-OriginalProcessAbsent -ProcessId $harnessHandle.Job.ProcessId `
        -CreationFileTimeUtc $harnessHandle.Job.CreationFileTimeUtc
    Assert-PortClosed -Port $port
    Remove-RunnerJobHandle -Handle $harnessHandle
    $allHandles.Remove($harnessHandle) | Out-Null
    $harnessLog = Read-BoundedUtf8File -LiteralPath $harnessHandle.StdoutPath -AllowEmpty
    $harnessError = Read-BoundedUtf8File -LiteralPath $harnessHandle.StderrPath -AllowEmpty
    if ($harnessLog.Contains($controlToken) -or $harnessError.Contains($controlToken)) {
        Throw-RunnerFailure 'harness-secret-log-detected'
    }
    if ($harnessLog.Length -ne 0 -or $harnessError.Length -ne 0) {
        Throw-RunnerFailure 'harness-log-not-empty'
    }
    Remove-ChildLogs -Handle $harnessHandle
    $harnessHandle = $null

    foreach ($temporary in @($readyPath, $handoffPath)) {
        if (Test-Path -LiteralPath $temporary) {
            Remove-Item -LiteralPath $temporary -Force
        }
        if (Test-Path -LiteralPath $temporary) { Throw-RunnerFailure 'temporary-file-cleanup-failed' }
    }

    $finalSource = Get-SourceSnapshot -GitPath $gitPath -RepositoryRoot $repositoryRoot
    Assert-SnapshotEqual -Expected $sourceState -Actual $finalSource
    if ((Get-FileSha256Hex -LiteralPath $harnessExecutable) -cne $harnessExecutableSha256) {
        Throw-RunnerFailure 'harness-executable-drift'
    }

    $resultItem = Assert-RegularPath -LiteralPath $resultPath -Kind Leaf -FailureCode 'result-artifact-invalid'
    $reportItem = Assert-RegularPath -LiteralPath $reportPath -Kind Leaf -FailureCode 'report-artifact-invalid'
    $summary = [ordered]@{
        schema_version = 1
        runner_version = $script:RunnerVersion
        attempt = [ordered]@{
            id = $attemptId
            label = $Label
            output_root = $OutputRoot
        }
        source_state = $sourceState
        fixture = [ordered]@{ version = $script:FixtureVersion; sha256 = $script:FixtureSha256 }
        environment = $environmentIdentity
        revision_binding = [ordered]@{
            source_head = $sourceState.head
            build_environment_revision = $sourceState.head
            runtime_environment_revision = $sourceState.head
            ready_revision = $ready.revision
            instance_before_revision = $instanceBefore.revision
            instance_after_revision = $instanceAfter.revision
        }
        harness_executable = [ordered]@{
            sha256 = $harnessExecutableSha256
            bytes = $harnessExecutableItem.Length
            compiled_revision = $sourceState.head
        }
        build_descendant_cleanup = [ordered]@{
            terminated_allowlisted = [bool]$buildExecution.AllowedHelperTerminated
            helpers = @($buildExecution.AllowedHelperNames)
        }
        instance = [ordered]@{
            revision = $sourceState.head
            pid = $ready.pid
            fixture_mode = $FixtureMode
            outbound_calls_before = $instanceBefore.outbound_calls
            outbound_calls_after = $instanceAfter.outbound_calls
            browser_outbound_attempts = $instanceAfter.browser_outbound_attempts
        }
        process_rss_bytes = [ordered]@{
            after_ready = $rssAfterReady
            after_load = $rssAfterLoad
            before_shutdown = $rssBeforeShutdown
        }
        job_evidence = @($jobEvidenceRecords.ToArray())
        workload = [ordered]@{
            kind = 'Load'
            path = $Path
            requests = $Requests
            concurrency = $Concurrency
            selected_specs = @()
            selected_projects = @()
            expected_outcome = $null
            expected_failure_id = $null
        }
        test_counts = [ordered]@{ expected = 0; skipped = 0; flaky = 0; unexpected = 0 }
        coverage = [ordered]@{
            checkpoint = 'W0-01C-A'
            status = 'partial'
            implemented = @(
                'Public+Load',
                'three-point-process-rss',
                'windows-job-phase-cardinality',
                'zero-server-and-provider-counters'
            )
            unavailable_metrics = @(
                [ordered]@{
                    metric = 'settings-cache-waiters'
                    status = 'unavailable'
                    reason = 'product-instrumentation-not-yet-implemented'
                    follow_up_task_id = 'E2-settings-cache-instrumentation'
                },
                [ordered]@{
                    metric = 'runtime-cache-cardinality'
                    status = 'unavailable'
                    reason = 'product-instrumentation-not-yet-implemented'
                    follow_up_task_id = 'E5-cache-cardinality-instrumentation'
                }
            )
            unsupported_boundary = [ordered]@{
                combinations = @('GuildDetail+Load', 'ReadOnly+Playwright', 'Public+Npm')
                readonly_playwright_proof = [ordered]@{
                    status = 'unsupported-in-this-checkpoint'
                    playwright_version = '1.58.2'
                    browser_artifacts_prelaunch = 'must-be-absent'
                    report_and_result_prelaunch = 'must-be-absent'
                    node_environment = 'minimal-child-allowlist'
                    worker_temporary_pattern = '.playwright-artifacts-*'
                    orchestrator_removes_last_run = $true
                    expected_outcome = @('Pass', 'Red')
                    pass_expected_failure_id = $null
                    red_expected_failure_ids = @(
                        'public-responsive-reduced-motion',
                        'guild-readonly-dialog',
                        'same-origin-font-proof'
                    )
                    required_cells = 12
                }
            }
        }
        result = [ordered]@{
            leaf = [System.IO.Path]::GetFileName($resultPath)
            sha256 = Get-FileSha256Hex -LiteralPath $resultPath
            bytes = $resultItem.Length
        }
        report = [ordered]@{
            leaf = [System.IO.Path]::GetFileName($reportPath)
            sha256 = Get-FileSha256Hex -LiteralPath $reportPath
            bytes = $reportItem.Length
        }
        counters = [ordered]@{
            before = [ordered]@{
                denied_requests = $countersBefore.denied_requests
                server_write_attempts = $countersBefore.server_write_attempts
                repository_reads = $countersBefore.repository_reads
                repository_mutations = $countersBefore.repository_mutations
                outbound_calls = $countersBefore.outbound_calls
                browser_outbound_attempts = $countersBefore.browser_outbound_attempts
                provider_guild_lookups = $countersBefore.provider_guild_lookups
            }
            after = [ordered]@{
                denied_requests = $countersAfter.denied_requests
                server_write_attempts = $countersAfter.server_write_attempts
                repository_reads = $countersAfter.repository_reads
                repository_mutations = $countersAfter.repository_mutations
                outbound_calls = $countersAfter.outbound_calls
                browser_outbound_attempts = $countersAfter.browser_outbound_attempts
                provider_guild_lookups = $countersAfter.provider_guild_lookups
            }
        }
        exit_classification = 'green'
        teardown = [ordered]@{
            graceful = $true
            job_active_processes = $closedEvidence.ActiveProcessCount
            descendants_closed = $true
            original_process_absent = $true
            port_closed = $true
            temporary_files_absent = $true
        }
    }
    $summaryBody = $summary | ConvertTo-Json -Depth 32 -Compress
    if ($summaryBody.Contains($controlToken) -or $summaryBody -match '(?i)cookie|authorization|storage_state|storage-state') {
        Throw-RunnerFailure 'summary-secret-field-detected'
    }
    Write-ExclusiveJson -LiteralPath $summaryPath -Value $summary
    foreach ($retainedPath in @($markerPath, $resultPath, $reportPath, $summaryPath)) {
        $retainedBody = [System.IO.File]::ReadAllText($retainedPath, $script:Utf8NoBom)
        if ($retainedBody.Contains($controlToken)) {
            Throw-RunnerFailure 'retained-control-secret-detected'
        }
    }

    $allowedLeaves = @(
        [System.IO.Path]::GetFileName($markerPath),
        [System.IO.Path]::GetFileName($resultPath),
        [System.IO.Path]::GetFileName($reportPath),
        [System.IO.Path]::GetFileName($summaryPath)
    ) | Sort-Object
    $actualLeaves = @(Get-ChildItem -LiteralPath $attemptDirectory -Force | ForEach-Object {
        if ($_ -isnot [System.IO.FileInfo] -or
            ($_.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            Throw-RunnerFailure 'attempt-final-inventory-invalid'
        }
        $_.Name
    } | Sort-Object)
    if ($actualLeaves.Count -ne $allowedLeaves.Count) { Throw-RunnerFailure 'attempt-final-inventory-invalid' }
    for ($index = 0; $index -lt $allowedLeaves.Count; $index++) {
        if ($actualLeaves[$index] -cne $allowedLeaves[$index]) {
            Throw-RunnerFailure 'attempt-final-inventory-invalid'
        }
    }

    $finalOutput = [ordered]@{
        attempt_id = $attemptId
        attempt_dir = $attemptDirectory
        result_path = $resultPath
        report_path = $reportPath
        summary_path = $summaryPath
    }
    $attemptSucceeded = $true
}
catch {
    $failureCode = Get-RunnerFailureCode -Exception $_.Exception
}
finally {
    if ($null -ne $httpClient) {
        try { $httpClient.Dispose() } catch { }
    }
    $teardownCleanupFailed = $false
    foreach ($handle in @($allHandles.ToArray())) {
        try { Remove-RunnerJobHandle -Handle $handle }
        catch { $teardownCleanupFailed = $true }
    }
    if (-not $attemptSucceeded -and $null -ne $teardownPort -and -not $teardownCleanupFailed) {
        try { Assert-PortClosed -Port $teardownPort }
        catch { $teardownCleanupFailed = $true }
    }
    if ($teardownCleanupFailed) {
        $attemptSucceeded = $false
        $failureCode = 'teardown-child-cleanup-failed'
    }
    if ($attemptCreated -and -not $attemptSucceeded) {
        try {
            if ($null -ne $markerPath -and (Test-Path -LiteralPath $markerPath)) {
                Remove-OwnedAttempt -AttemptsRoot $attemptsRoot -AttemptDirectory $attemptDirectory `
                    -AttemptId $attemptId -MarkerPath $markerPath
            }
            elseif (Test-Path -LiteralPath $attemptDirectory) {
                Remove-UnmarkedAttempt -AttemptsRoot $attemptsRoot -AttemptDirectory $attemptDirectory `
                    -AttemptId $attemptId
            }
        }
        catch {
            $failureCode = 'attempt-cleanup-failed'
        }
    }
}

if ($attemptSucceeded) {
    [Console]::Out.WriteLine(($finalOutput | ConvertTo-Json -Compress))
    exit 0
}

if ([string]::IsNullOrWhiteSpace($failureCode)) { $failureCode = 'unexpected-runner-failure' }
[Console]::Error.WriteLine("with-isolated-dashboard failed: $failureCode")
exit 2
