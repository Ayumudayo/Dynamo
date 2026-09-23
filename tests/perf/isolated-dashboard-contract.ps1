$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($PSVersionTable.PSVersion -lt [version]'7.4') {
    throw 'isolated-dashboard-contract requires PowerShell 7.4 or newer'
}
if (-not $IsWindows) {
    throw 'isolated-dashboard-contract requires Windows'
}

$script:Assertions = 0
$script:Utf8NoBom = [System.Text.UTF8Encoding]::new($false)
$script:SecretSentinel = 'DYNAMO_CONTRACT_SECRET_47f0f945b09d4d53aee10ab130b49c84'
$script:ExpectedFixtureVersion = 'guild-detail-v1'
$script:ExpectedFixtureSha256 = '5f08c171827be0ad90f5a6b7c980b4ab21d938cd2e73137f03f5ac7360b64885'

function Assert-True {
    param([Parameter(Mandatory)][bool] $Condition, [Parameter(Mandatory)][string] $Message)
    $script:Assertions++
    if (-not $Condition) { throw "contract assertion failed: $Message" }
}

function Assert-Equal {
    param(
        [AllowNull()][object] $Actual,
        [AllowNull()][object] $Expected,
        [Parameter(Mandatory)][string] $Message
    )
    $script:Assertions++
    if ([string]$Actual -cne [string]$Expected) {
        throw "contract assertion failed: $Message (actual='$Actual', expected='$Expected')"
    }
}

function Assert-ExactKeys {
    param(
        [Parameter(Mandatory)][object] $Value,
        [Parameter(Mandatory)][string[]] $Expected,
        [Parameter(Mandatory)][string] $Message
    )
    $actualKeys = @($Value.PSObject.Properties.Name | Sort-Object)
    $expectedKeys = @($Expected | Sort-Object)
    Assert-Equal -Actual $actualKeys.Count -Expected $expectedKeys.Count -Message "$Message count"
    for ($index = 0; $index -lt $expectedKeys.Count; $index++) {
        Assert-Equal -Actual $actualKeys[$index] -Expected $expectedKeys[$index] `
            -Message "$Message key $index"
    }
}

function Assert-ExactRunnerAcl {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [Parameter(Mandatory)][string] $Message
    )
    $directory = Get-Item -LiteralPath $LiteralPath -Force
    Assert-True -Condition ($directory -is [System.IO.DirectoryInfo]) `
        -Message "$Message is a directory"
    Assert-True -Condition (($directory.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
        -Message "$Message is not a reparse point"
    $security = [System.IO.FileSystemAclExtensions]::GetAccessControl($directory)
    $currentSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
    $systemSid = [System.Security.Principal.SecurityIdentifier]::new(
        [System.Security.Principal.WellKnownSidType]::LocalSystemSid,
        $null)
    Assert-Equal -Actual $security.GetOwner(
        [System.Security.Principal.SecurityIdentifier]).Value -Expected $currentSid.Value `
        -Message "$Message owner is current user"
    Assert-Equal -Actual $security.AreAccessRulesProtected -Expected $true `
        -Message "$Message DACL is protected"
    $rules = @($security.GetAccessRules(
        $true,
        $true,
        [System.Security.Principal.SecurityIdentifier]))
    Assert-Equal -Actual $rules.Count -Expected 2 -Message "$Message has exact ACE count"
    $expectedSids = @($currentSid.Value, $systemSid.Value)
    $seenSids = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal)
    $expectedInheritance = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
        [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
    foreach ($rule in $rules) {
        Assert-True -Condition ($expectedSids -ccontains $rule.IdentityReference.Value) `
            -Message "$Message ACE identity is allowlisted"
        Assert-True -Condition $seenSids.Add($rule.IdentityReference.Value) `
            -Message "$Message ACE identity is unique"
        Assert-Equal -Actual $rule.IsInherited -Expected $false `
            -Message "$Message ACE is explicit"
        Assert-Equal -Actual $rule.AccessControlType `
            -Expected ([System.Security.AccessControl.AccessControlType]::Allow) `
            -Message "$Message ACE is Allow"
        Assert-Equal -Actual $rule.FileSystemRights `
            -Expected ([System.Security.AccessControl.FileSystemRights]::FullControl) `
            -Message "$Message ACE has FullControl"
        Assert-Equal -Actual $rule.InheritanceFlags -Expected $expectedInheritance `
            -Message "$Message ACE inheritance is container and object"
        Assert-Equal -Actual $rule.PropagationFlags `
            -Expected ([System.Security.AccessControl.PropagationFlags]::None) `
            -Message "$Message ACE propagation is None"
    }
    Assert-Equal -Actual $seenSids.Count -Expected 2 `
        -Message "$Message contains current and SYSTEM exactly once"
}

function Assert-SafeDiagnosticLeaf {
    param(
        [Parameter(Mandatory)][System.IO.FileInfo] $File,
        [Parameter(Mandatory)][string] $Message
    )
    Assert-True -Condition (($File.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
        -Message "$Message is not a reparse point"
    Assert-True -Condition ($File.Length -gt 0 -and $File.Length -le 65536) `
        -Message "$Message has bounded nonempty content"
    $security = [System.IO.FileSystemAclExtensions]::GetAccessControl($File)
    $currentSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
    $systemSid = [System.Security.Principal.SecurityIdentifier]::new(
        [System.Security.Principal.WellKnownSidType]::LocalSystemSid,
        $null)
    Assert-Equal -Actual $security.GetOwner(
        [System.Security.Principal.SecurityIdentifier]).Value -Expected $currentSid.Value `
        -Message "$Message owner is current user"
    $allowedSids = @($currentSid.Value, $systemSid.Value)
    $rules = @($security.GetAccessRules(
        $true,
        $true,
        [System.Security.Principal.SecurityIdentifier]))
    Assert-True -Condition ($rules.Count -ge 1 -and $rules.Count -le 4) `
        -Message "$Message has bounded access rules"
    foreach ($rule in $rules) {
        Assert-True -Condition ($allowedSids -ccontains $rule.IdentityReference.Value) `
            -Message "$Message ACE identity is allowlisted"
        Assert-Equal -Actual $rule.AccessControlType `
            -Expected ([System.Security.AccessControl.AccessControlType]::Allow) `
            -Message "$Message ACE is Allow"
    }
}

function Write-Utf8File {
    param([Parameter(Mandatory)][string] $LiteralPath, [Parameter(Mandatory)][string] $Value)
    [System.IO.File]::WriteAllText($LiteralPath, $Value, $script:Utf8NoBom)
}

function Assert-CanonicalJsonArtifact {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [Parameter(Mandatory)][string] $Message
    )
    $bytes = [System.IO.File]::ReadAllBytes($LiteralPath)
    Assert-True -Condition ($bytes.Length -ge 3) -Message "$Message is nonempty"
    Assert-True -Condition (-not ($bytes[0] -eq 0xef -and $bytes[1] -eq 0xbb -and $bytes[2] -eq 0xbf)) `
        -Message "$Message has no UTF-8 BOM"
    $body = [System.IO.File]::ReadAllText($LiteralPath, $script:Utf8NoBom)
    Assert-True -Condition ($body -match '^\{[^\r\n]*\}\n$') `
        -Message "$Message is one compact JSON object with one trailing LF"
    try {
        [void]($body.TrimEnd("`n") | ConvertFrom-Json -Depth 32)
    }
    catch {
        throw "contract assertion failed: $Message parses as JSON"
    }
}

function Get-DescendantRecordPath {
    param([Parameter(Mandatory)][string] $Scenario)
    Assert-True -Condition ($Scenario -cmatch `
        '^((short|long|allowlisted)-job|harness-(short|long))-descendant-[0-9a-f]{32}$') `
        -Message 'descendant scenario has a safe unique name'
    return Join-Path ([System.IO.Path]::GetTempPath()) `
        ("dynamo-isolated-dashboard-$Scenario.json")
}

function Read-DescendantRecord {
    param([Parameter(Mandatory)][string] $LiteralPath)
    Assert-True -Condition (Test-Path -LiteralPath $LiteralPath -PathType Leaf) `
        -Message 'descendant identity record exists'
    $record = [System.IO.File]::ReadAllText($LiteralPath, $script:Utf8NoBom) |
        ConvertFrom-Json -Depth 8
    Assert-ExactKeys -Value $record -Expected @(
        'pid', 'creation_file_time_utc', 'delay_milliseconds'
    ) -Message 'descendant identity record'
    Assert-True -Condition ($record.pid -is [int64] -and $record.pid -gt 0) `
        -Message 'descendant identity record has a PID'
    Assert-True -Condition ($record.creation_file_time_utc -is [int64] -and
        $record.creation_file_time_utc -gt 0) `
        -Message 'descendant identity record has a creation time'
    return $record
}

function Assert-RecordedProcessAbsent {
    param(
        [Parameter(Mandatory)][object] $Record,
        [Parameter(Mandatory)][string] $Message
    )
    $matchingIdentityPresent = $false
    try {
        $candidate = [System.Diagnostics.Process]::GetProcessById([int]$Record.pid)
        try {
            $candidateCreation = [uint64]$candidate.StartTime.ToUniversalTime().ToFileTimeUtc()
            $matchingIdentityPresent = $candidateCreation -eq [uint64]$Record.creation_file_time_utc
        }
        finally { $candidate.Dispose() }
    }
    catch [System.ArgumentException] { }
    Assert-True -Condition (-not $matchingIdentityPresent) -Message $Message
}

function Invoke-GitChecked {
    param(
        [Parameter(Mandatory)][string] $GitPath,
        [Parameter(Mandatory)][string] $Repository,
        [Parameter(Mandatory)][string[]] $Arguments
    )
    $output = & $GitPath -C $Repository @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "git setup failed: $($output -join ' ')"
    }
    return $output
}

function Get-ChildEnvironment {
    $environment = [ordered]@{}
    foreach ($name in @(
        'SystemRoot', 'WINDIR', 'ComSpec', 'PATH', 'PATHEXT', 'TEMP', 'TMP',
        'USERPROFILE', 'HOME', 'LOCALAPPDATA', 'APPDATA', 'PROGRAMDATA',
        'NUMBER_OF_PROCESSORS', 'PROCESSOR_ARCHITECTURE'
    )) {
        $value = [System.Environment]::GetEnvironmentVariable($name, 'Process')
        if ($null -ne $value -and $value.Length -gt 0) { $environment[$name] = $value }
    }
    return $environment
}

function Invoke-ContractRunner {
    param(
        [Parameter(Mandatory)][string] $Repository,
        [string] $Scenario = 'success',
        [string[]] $RunnerArguments = @(
            '-FixtureMode', 'Public', '-Workload', 'Load', '-Path', '/',
            '-Requests', '4', '-Concurrency', '2', '-OutputRoot', 'output/perf',
            '-Label', 'contract-public'
        ),
        [hashtable] $AdditionalEnvironment = @{}
    )
    $pwshPath = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
    $launcherPath = Join-Path $Repository 'scripts\perf\with-isolated-dashboard.ps1'
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $pwshPath
    $start.WorkingDirectory = $Repository
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardOutputEncoding = $script:Utf8NoBom
    $start.StandardErrorEncoding = $script:Utf8NoBom
    $start.Environment.Clear()
    foreach ($entry in (Get-ChildEnvironment).GetEnumerator()) {
        $start.Environment[[string]$entry.Key] = [string]$entry.Value
    }
    $start.Environment['DYNAMO_PERF_CONTRACT_MODE'] = '1'
    $start.Environment['DYNAMO_PERF_CONTRACT_SCENARIO'] = $Scenario
    $start.Environment['DYNAMO_CONTRACT_SECRET_SENTINEL'] = $script:SecretSentinel
    $start.Environment['NODE_OPTIONS'] = '--require=caller-controlled-module'
    $start.Environment['NODE_PATH'] = 'C:\caller-controlled-node-path'
    $start.Environment['DEBUG'] = '*'
    $start.Environment['HTTP_PROXY'] = 'http://caller.invalid:9'
    $start.Environment['HTTPS_PROXY'] = 'http://caller.invalid:9'
    $start.Environment['ALL_PROXY'] = 'http://caller.invalid:9'
    $start.Environment['NO_PROXY'] = '*'
    $start.Environment['DISCORD_TOKEN'] = $script:SecretSentinel
    $start.Environment['MONGODB_URI'] = 'mongodb://caller-secret.invalid/'
    $start.Environment['DASHBOARD_CLIENT_SECRET'] = $script:SecretSentinel
    foreach ($entry in $AdditionalEnvironment.GetEnumerator()) {
        $start.Environment[[string]$entry.Key] = [string]$entry.Value
    }
    foreach ($argument in @('-NoProfile', '-NonInteractive', '-File', $launcherPath)) {
        [void]$start.ArgumentList.Add($argument)
    }
    foreach ($argument in $RunnerArguments) { [void]$start.ArgumentList.Add($argument) }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    try {
        Assert-True -Condition $process.Start() -Message 'runner subprocess started'
        $process.StandardInput.Close()
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(90000)) {
            try { $process.Kill($true) } catch { }
            [void]$process.WaitForExit(5000)
            throw 'contract runner exceeded 90 seconds'
        }
        [void][System.Threading.Tasks.Task]::WaitAll(@($stdoutTask, $stderrTask), 5000)
        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = $stdoutTask.Result
            Stderr = $stderrTask.Result
        }
    }
    finally { $process.Dispose() }
}

function Assert-SafeFailure {
    param(
        [Parameter(Mandatory)][object] $Execution,
        [string] $ExpectedCode
    )
    Assert-True -Condition ($Execution.ExitCode -ne 0) -Message 'negative runner exits nonzero'
    Assert-Equal -Actual $Execution.Stdout -Expected '' -Message 'negative runner has no success stdout'
    Assert-True -Condition (-not $Execution.Stderr.Contains($script:SecretSentinel)) `
        -Message 'negative stderr excludes caller secret sentinel'
    if (-not [string]::IsNullOrEmpty($ExpectedCode)) {
        Assert-Equal -Actual $Execution.Stderr `
            -Expected "with-isolated-dashboard failed: $ExpectedCode`r`n" `
            -Message "negative stderr is stable for $ExpectedCode"
    }
}

$sourceRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$launcherSource = Join-Path $sourceRoot 'scripts\perf\with-isolated-dashboard.ps1'
$artifactHelperSource = Join-Path $sourceRoot 'scripts\perf\artifact-json.ps1'
$runnerEvidenceHelperSource = Join-Path $sourceRoot 'scripts\perf\runner-evidence.ps1'
$runnerValidationHelperSource = Join-Path $sourceRoot 'scripts\perf\runner-validation.ps1'
$moduleSource = Join-Path $sourceRoot 'scripts\perf\isolated-process-job.psm1'
$fixtureSource = Join-Path $sourceRoot 'tests\perf\fixtures\guild-detail-v1.json'

$fixtureHash = (Get-FileHash -LiteralPath $fixtureSource -Algorithm SHA256).Hash.ToLowerInvariant()
Assert-Equal -Actual $fixtureHash -Expected $script:ExpectedFixtureSha256 `
    -Message 'source fixture has the pinned guild-detail-v1 SHA-256'

$parseErrors = $null
$launcherAst = [System.Management.Automation.Language.Parser]::ParseFile(
    $launcherSource, [ref]$null, [ref]$parseErrors)
Assert-Equal -Actual $parseErrors.Count -Expected 0 -Message 'launcher parses without errors'
$pathFunctionNames = @(
    'Throw-RunnerFailure',
    'Assert-RegularPath',
    'Resolve-ReparseFreeApplicationPath'
)
$pathFunctionDefinitions = @($launcherAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $pathFunctionNames -ccontains $node.Name
}, $true) | ForEach-Object { $_.Extent.Text })
Assert-Equal -Actual $pathFunctionDefinitions.Count -Expected $pathFunctionNames.Count `
    -Message 'launcher path resolver function extraction'
. ([scriptblock]::Create(($pathFunctionDefinitions -join "`n`n")))
$launcherText = [System.IO.File]::ReadAllText($launcherSource, $script:Utf8NoBom)
$runnerEvidenceHelperText = [System.IO.File]::ReadAllText($runnerEvidenceHelperSource, $script:Utf8NoBom)
$runnerValidationHelperText = [System.IO.File]::ReadAllText($runnerValidationHelperSource, $script:Utf8NoBom)
Assert-True -Condition $launcherText.Contains("`$script:FixtureVersion = '$($script:ExpectedFixtureVersion)'") `
    -Message 'launcher declares the pinned fixture version'
Assert-True -Condition $launcherText.Contains("`$script:FixtureSha256 = '$($script:ExpectedFixtureSha256)'") `
    -Message 'launcher declares the pinned fixture SHA-256'
foreach ($forbidden in @(
    'Start-Process',
    'Invoke-Expression',
    'cmd.exe',
    ' npx ',
    '[System.Security.AccessControl.DirectorySecurity]::new',
    '.SetOwner('
)) {
    Assert-True -Condition (-not $launcherText.Contains($forbidden)) `
        -Message "launcher excludes forbidden command surface $forbidden"
}
foreach ($required in @(
    'Import-Module', 'Start-DynamoIsolatedProcess', 'Get-DynamoIsolatedProcessEvidence',
    'Wait-DynamoIsolatedProcess', 'Stop-DynamoIsolatedProcess',
    'drainTimeoutMilliseconds', '[System.Diagnostics.Stopwatch]::StartNew', 'ActiveProcessIds',
    '[System.IO.Path]::GetDirectoryName($path)', '-Candidate $helperParent',
    'DYNAMO_PERF_BUILD_REVISION', 'x-dynamo-perf-control',
    'provider_guild_lookups', 'repository_reads', 'denied_requests'
)) {
    Assert-True -Condition (($launcherText + "`n" + $runnerEvidenceHelperText + "`n" + $runnerValidationHelperText).Contains($required)) `
        -Message "runner control sources contain required isolation contract $required"
}

$temporaryBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\')
$contractRoot = Join-Path $temporaryBase ('dynamo-isolated-dashboard-' + [Guid]::NewGuid().ToString('N'))
$repository = Join-Path $contractRoot 'repo'
$externalJunctionTarget = Join-Path $contractRoot 'junction-target'
$gitPath = (Get-Command git.exe -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source

try {
    [void][System.IO.Directory]::CreateDirectory((Join-Path $repository 'scripts\perf'))
    [void][System.IO.Directory]::CreateDirectory((Join-Path $repository 'tests\perf\fixtures'))
    [void][System.IO.Directory]::CreateDirectory((Join-Path $repository 'tests\perf\budgets'))
    $physicalToolRoot = Join-Path $contractRoot 'physical-tool'
    $linkedToolRoot = Join-Path $contractRoot 'linked-tool'
    [void][System.IO.Directory]::CreateDirectory($physicalToolRoot)
    $physicalToolPath = Join-Path $physicalToolRoot 'tool.exe'
    Write-Utf8File -LiteralPath $physicalToolPath -Value "contract-tool`n"
    [void](New-Item -ItemType Junction -Path $linkedToolRoot -Target $physicalToolRoot)
    try {
        $resolvedToolPath = Resolve-ReparseFreeApplicationPath `
            -LiteralPath (Join-Path $linkedToolRoot 'tool.exe') `
            -FailureCode 'contract-tool-resolution-failed'
        Assert-Equal -Actual $resolvedToolPath -Expected $physicalToolPath `
            -Message 'reparse-free resolver returns the physical application path'
        Assert-True -Condition (((Get-Item -LiteralPath $resolvedToolPath -Force).Attributes `
            -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
            -Message 'resolved application leaf is not a reparse point'
    }
    finally {
        if (Test-Path -LiteralPath $linkedToolRoot) {
            Remove-Item -LiteralPath $linkedToolRoot -Force
        }
    }
    Assert-True -Condition (-not (Test-Path -LiteralPath $linkedToolRoot)) `
        -Message 'application resolver junction fixture is removed'
    Copy-Item -LiteralPath $launcherSource -Destination (Join-Path $repository 'scripts\perf\with-isolated-dashboard.ps1')
    Copy-Item -LiteralPath $artifactHelperSource -Destination (Join-Path $repository 'scripts\perf\artifact-json.ps1')
    Copy-Item -LiteralPath $runnerEvidenceHelperSource -Destination (Join-Path $repository 'scripts\perf\runner-evidence.ps1')
    Copy-Item -LiteralPath $runnerValidationHelperSource -Destination (Join-Path $repository 'scripts\perf\runner-validation.ps1')
    Copy-Item -LiteralPath $moduleSource -Destination (Join-Path $repository 'scripts\perf\isolated-process-job.psm1')
    Copy-Item -LiteralPath $fixtureSource -Destination (Join-Path $repository 'tests\perf\fixtures\guild-detail-v1.json')
    Write-Utf8File -LiteralPath (Join-Path $repository '.gitignore') -Value "output/`ntarget/`n"
    Write-Utf8File -LiteralPath (Join-Path $repository '.dynamo-perf-contract-v1') `
        -Value "dynamo-perf-contract-v1`n"
    Write-Utf8File -LiteralPath (Join-Path $repository 'scripts\perf\dashboard-load.cjs') `
        -Value "'use strict';`n"
    Write-Utf8File -LiteralPath (Join-Path $repository 'scripts\perf\assert-budgets.cjs') `
        -Value "'use strict';`n"
    Write-Utf8File -LiteralPath (Join-Path $repository 'tests\perf\budgets\public-root.json') `
        -Value "{`"max_p95_ms`":10,`"max_failed`":0,`"max_decoded_bytes_per_request`":27041}`n"

    $contractChild = @'
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Build', 'Harness', 'Load', 'Budget')][string] $Operation,
    [string] $Current,
    [string] $Budget
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$utf8 = [System.Text.UTF8Encoding]::new($false)
foreach ($forbidden in @(
    'NODE_OPTIONS', 'NODE_PATH', 'DEBUG', 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY',
    'NO_PROXY', 'DISCORD_TOKEN', 'MONGODB_URI', 'DASHBOARD_CLIENT_SECRET',
    'DYNAMO_CONTRACT_SECRET_SENTINEL'
)) {
    if ($null -ne [System.Environment]::GetEnvironmentVariable($forbidden, 'Process')) { exit 97 }
}

function Write-NewJson([string] $Path, [object] $Value) {
    $body = ($Value | ConvertTo-Json -Depth 32 -Compress) + "`n"
    $stream = [System.IO.FileStream]::new(
        $Path, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None, 4096, [System.IO.FileOptions]::WriteThrough)
    try {
        $bytes = $utf8.GetBytes($body)
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush($true)
    }
    finally { $stream.Dispose() }
}

$scenario = $env:DYNAMO_PERF_CONTRACT_SCENARIO
if ($Operation -eq 'Build') {
    if ($env:DYNAMO_PERF_BUILD_REVISION -cnotmatch '^[0-9a-f]{40}$') { exit 2 }
    if ($scenario -cmatch '^(?<lifetime>short|long|allowlisted)-job-descendant-(?<id>[0-9a-f]{32})$') {
        $delayMilliseconds = if ($Matches.lifetime -ceq 'short') { 750 } else { 30000 }
        $descendantStart = [System.Diagnostics.ProcessStartInfo]::new()
        $descendantStart.FileName = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
        $descendantStart.UseShellExecute = $false
        $descendantStart.CreateNoWindow = $true
        foreach ($argument in @(
            '-NoProfile', '-NonInteractive', '-Command',
            "[System.Threading.Thread]::Sleep($delayMilliseconds)"
        )) {
            [void]$descendantStart.ArgumentList.Add($argument)
        }
        $descendant = [System.Diagnostics.Process]::new()
        $descendant.StartInfo = $descendantStart
        try {
            if (-not $descendant.Start()) { exit 98 }
            $recordPath = Join-Path $env:TEMP ("dynamo-isolated-dashboard-$scenario.json")
            Write-NewJson -Path $recordPath -Value ([ordered]@{
                pid = $descendant.Id
                creation_file_time_utc = $descendant.StartTime.ToUniversalTime().ToFileTimeUtc()
                delay_milliseconds = $delayMilliseconds
            })
        }
        finally { $descendant.Dispose() }
    }
    exit 0
}

if ($Operation -eq 'Load') {
    $handoff = [System.IO.File]::ReadAllText($env:PERF_INSTANCE_HANDOFF, $utf8) | ConvertFrom-Json -Depth 32
    $requests = [int]$env:PERF_REQUESTS
    $concurrency = [int]$env:PERF_CONCURRENCY
    $result = [ordered]@{
        schema_version = 1
        runner_version = 'dashboard-load-v1'
        source_state = $handoff.source_state
        fixture = $handoff.fixture
        environment = $handoff.environment
        instance = [ordered]@{
            revision = $handoff.revision
            nonce = $handoff.nonce
            pid = $handoff.pid
            fixture_mode = $handoff.fixture_mode
            outbound_calls_before = 0
            outbound_calls_after = 0
            browser_outbound_attempts = 0
        }
        path = $env:PERF_PATH
        requests = $requests
        concurrency = $concurrency
        ok = $requests
        failed = 0
        decoded_bytes = 100 * $requests
        wire_bytes = 100 * $requests
        content_encodings = [ordered]@{ identity = $requests }
        p50_ms = 0.1
        p95_ms = 0.2
        max_ms = 0.3
        statuses = [ordered]@{ '200' = $requests }
    }
    Write-NewJson -Path $env:PERF_OUT -Value $result
    [Console]::Out.WriteLine((([ordered]@{
        schema_version = 1
        result_path = $env:PERF_OUT
        exit_code = 0
    }) | ConvertTo-Json -Compress))
    exit 0
}

if ($Operation -eq 'Budget') {
    $passed = $scenario -cne 'budget-fail'
    $decision = [ordered]@{
        schema_version = 1
        passed = $passed
        checks = @([ordered]@{ name = 'contract'; observed = 0; allowed = 0; passed = $passed })
    }
    [Console]::Out.WriteLine(($decision | ConvertTo-Json -Depth 8 -Compress))
    if ($passed) { exit 0 }
    exit 2
}

if ($Operation -ne 'Harness') { exit 2 }
if ($scenario -cmatch '^harness-(?<lifetime>short|long)-descendant-[0-9a-f]{32}$') {
    $delayMilliseconds = if ($Matches.lifetime -ceq 'short') { 750 } else { 30000 }
    $descendantStart = [System.Diagnostics.ProcessStartInfo]::new()
    $descendantStart.FileName = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
    $descendantStart.UseShellExecute = $false
    $descendantStart.CreateNoWindow = $true
    foreach ($argument in @(
        '-NoProfile', '-NonInteractive', '-Command',
        "[System.Threading.Thread]::Sleep($delayMilliseconds)"
    )) {
        [void]$descendantStart.ArgumentList.Add($argument)
    }
    $descendant = [System.Diagnostics.Process]::new()
    $descendant.StartInfo = $descendantStart
    try {
        if (-not $descendant.Start()) { exit 98 }
        $recordPath = Join-Path $env:TEMP ("dynamo-isolated-dashboard-$scenario.json")
        Write-NewJson -Path $recordPath -Value ([ordered]@{
            pid = $descendant.Id
            creation_file_time_utc = $descendant.StartTime.ToUniversalTime().ToFileTimeUtc()
            delay_milliseconds = $delayMilliseconds
        })
    }
    finally { $descendant.Dispose() }
}
if ($scenario -ceq 'ready-timeout') {
    Start-Sleep -Seconds 10
    exit 0
}

$random = [byte[]]::new(32)
[System.Security.Cryptography.RandomNumberGenerator]::Fill($random)
$control = 'perf_' + [Convert]::ToHexString($random).ToLowerInvariant()
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
$listener.ExclusiveAddressUse = $true
$listener.Start(16)
$port = ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
$publishedNonce = if ($scenario -in @('wrong-ready-nonce', 'cleanup-junction')) { 'f' * 64 } else { $env:DYNAMO_PERF_NONCE }
$cleanupJunction = $null
if ($scenario -ceq 'cleanup-junction') {
    $cleanupJunction = Join-Path (Split-Path -LiteralPath $env:DYNAMO_PERF_READY_FILE) '.contract-child-junction'
    [void](New-Item -ItemType Junction -Path $cleanupJunction -Target $env:TEMP)
}
$ready = [ordered]@{
    schema_version = 1
    host = '127.0.0.1'
    dynamic_port = $true
    port = $port
    pid = $PID
    revision = $env:DYNAMO_PERF_REVISION
    nonce = $publishedNonce
    fixture_mode = $env:DYNAMO_PERF_FIXTURE_MODE
    fixture = [ordered]@{
        version = $env:DYNAMO_PERF_FIXTURE_VERSION
        sha256 = $env:DYNAMO_PERF_FIXTURE_SHA256
    }
    guild_id = 9000000000000000101
    cookie_name = 'dynamo_dashboard_session'
    cookie_value = $control
}
$readyTemporary = $env:DYNAMO_PERF_READY_FILE + '.' + [Guid]::NewGuid().ToString('N') + '.publish'
Write-NewJson -Path $readyTemporary -Value $ready
[System.IO.File]::Move($readyTemporary, $env:DYNAMO_PERF_READY_FILE, $false)

function Send-Response([System.Net.Sockets.NetworkStream] $Stream, [int] $Status, [string] $Body) {
    $reason = switch ($Status) { 200 { 'OK' } 204 { 'No Content' } 403 { 'Forbidden' } default { 'Error' } }
    $bodyBytes = $utf8.GetBytes($Body)
    $headers = "HTTP/1.1 $Status $reason`r`nContent-Type: application/json`r`nContent-Length: $($bodyBytes.Length)`r`nConnection: close`r`n`r`n"
    $headerBytes = $utf8.GetBytes($headers)
    $Stream.Write($headerBytes, 0, $headerBytes.Length)
    if ($bodyBytes.Length -gt 0) { $Stream.Write($bodyBytes, 0, $bodyBytes.Length) }
    $Stream.Flush()
}

$counterRequests = 0
$running = $true
try {
    while ($running) {
        $client = $listener.AcceptTcpClient()
        try {
            $stream = $client.GetStream()
            $buffer = [byte[]]::new(4096)
            $builder = [System.Text.StringBuilder]::new()
            while (-not $builder.ToString().Contains("`r`n`r`n")) {
                $read = $stream.Read($buffer, 0, $buffer.Length)
                if ($read -le 0) { break }
                [void]$builder.Append($utf8.GetString($buffer, 0, $read))
                if ($builder.Length -gt 65536) { break }
            }
            $request = $builder.ToString()
            $lines = $request.Split("`r`n")
            $requestLine = $lines[0].Split(' ')
            $method = $requestLine[0]
            $route = $requestLine[1]
            $headers = @{}
            foreach ($line in $lines | Select-Object -Skip 1) {
                $separator = $line.IndexOf(':')
                if ($separator -gt 0) {
                    $headers[$line.Substring(0, $separator).Trim().ToLowerInvariant()] = $line.Substring($separator + 1).Trim()
                }
            }
            if ($method -ceq 'GET' -and $route -ceq '/__perf/instance') {
                Send-Response -Stream $stream -Status 200 -Body (([ordered]@{
                    schema_version = 1
                    revision = $env:DYNAMO_PERF_REVISION
                    nonce = $env:DYNAMO_PERF_NONCE
                    pid = $PID
                    fixture_mode = $env:DYNAMO_PERF_FIXTURE_MODE
                    fixture = [ordered]@{
                        version = $env:DYNAMO_PERF_FIXTURE_VERSION
                        sha256 = $env:DYNAMO_PERF_FIXTURE_SHA256
                    }
                    outbound_calls = 0
                    browser_outbound_attempts = 0
                }) | ConvertTo-Json -Depth 8 -Compress)
            }
            elseif ($method -ceq 'GET' -and $route -ceq '/__perf/counters') {
                $counterRequests++
                $outbound = if ($scenario -ceq 'counters-drift' -and $counterRequests -gt 1) { 1 } else { 0 }
                Send-Response -Stream $stream -Status 200 -Body (([ordered]@{
                    schema_version = 1
                    denied_requests = 0
                    outbound_calls = $outbound
                    browser_outbound_attempts = 0
                    server_write_attempts = 0
                    repository_reads = 0
                    repository_mutations = 0
                    provider_guild_lookups = 0
                }) | ConvertTo-Json -Compress)
            }
            elseif ($method -ceq 'POST' -and $route -ceq '/__perf/shutdown') {
                if ($scenario -ceq 'shutdown-fail') {
                    Send-Response -Stream $stream -Status 500 -Body '{}'
                }
                elseif ($headers['x-dynamo-perf-control'] -cne $control) {
                    Send-Response -Stream $stream -Status 403 -Body '{}'
                }
                else {
                    Send-Response -Stream $stream -Status 204 -Body ''
                    $running = $false
                }
            }
            else {
                Send-Response -Stream $stream -Status 403 -Body '{}'
            }
        }
        finally { $client.Dispose() }
    }
}
finally { $listener.Stop() }
exit 0
'@
    Write-Utf8File -LiteralPath (Join-Path $repository 'tests\perf\contract-child.ps1') `
        -Value ($contractChild + "`n")

    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository -Arguments @('init', '-q'))
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository -Arguments @('config', 'user.name', 'Dynamo Contract'))
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository -Arguments @('config', 'user.email', 'contract@example.invalid'))
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository -Arguments @('config', 'core.autocrlf', 'false'))
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository -Arguments @('add', '--all'))
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository -Arguments @('commit', '-q', '-m', 'contract fixture'))

    foreach ($arguments in @(
        @('-FixtureMode', 'GuildDetail', '-Workload', 'Load', '-OutputRoot', 'output/perf', '-Label', 'closed'),
        @('-FixtureMode', 'ReadOnly', '-Workload', 'Load', '-OutputRoot', 'output/perf', '-Label', 'closed'),
        @('-FixtureMode', 'Public', '-Workload', 'Npm', '-OutputRoot', 'output/perf', '-Label', 'closed'),
        @('-FixtureMode', 'Public', '-Workload', 'Playwright', '-OutputRoot', 'output/perf', '-Label', 'closed'),
        @('-FixtureMode', 'Public', '-Workload', 'Load', '-OutputRoot', 'output/playwright', '-Label', 'closed')
    )) {
        $execution = Invoke-ContractRunner -Repository $repository -RunnerArguments $arguments
        Assert-SafeFailure -Execution $execution -ExpectedCode 'checkpoint-mode-not-implemented'
        Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $repository 'output'))) `
            -Message 'unsupported combination has no output side effect'
    }

    foreach ($arguments in @(
        @('-FixtureMode', 'Public', '-Workload', 'Load', '-OutputRoot', 'output/perf', '-Label', '../bad'),
        @('-FixtureMode', 'Public', '-Workload', 'Load', '-Path', '/?query=1', '-OutputRoot', 'output/perf', '-Label', 'bad-path'),
        @('-FixtureMode', 'Public', '-Workload', 'Load', '-Requests', '1', '-Concurrency', '2', '-OutputRoot', 'output/perf', '-Label', 'bad-concurrency'),
        @('-FixtureMode', $script:SecretSentinel, '-Workload', 'Load', '-OutputRoot', 'output/perf', '-Label', 'bad-fixture'),
        @('-FixtureMode', 'Public', '-Workload', 'Load', '-OutputRoot', '../outside', '-Label', 'bad-output')
    )) {
        $execution = Invoke-ContractRunner -Repository $repository -RunnerArguments $arguments
        Assert-SafeFailure -Execution $execution -ExpectedCode 'parameter-value-invalid'
        Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $repository 'output'))) `
            -Message 'invalid typed argument has no output side effect'
    }

    $forbiddenEnvironment = Invoke-ContractRunner -Repository $repository `
        -AdditionalEnvironment @{ PERF_BASE_URL = 'http://127.0.0.1:9' }
    Assert-SafeFailure -Execution $forbiddenEnvironment -ExpectedCode 'caller-environment-forbidden'
    Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $repository 'output'))) `
        -Message 'forbidden caller environment has no output side effect'

    [void][System.IO.Directory]::CreateDirectory((Join-Path $repository 'output'))
    [void][System.IO.Directory]::CreateDirectory($externalJunctionTarget)
    $junction = Join-Path $repository 'output\perf'
    [void](New-Item -ItemType Junction -Path $junction -Target $externalJunctionTarget)
    $junctionExecution = Invoke-ContractRunner -Repository $repository
    Assert-SafeFailure -Execution $junctionExecution -ExpectedCode 'output-root-invalid'
    Remove-Item -LiteralPath $junction -Force
    Assert-True -Condition (Test-Path -LiteralPath $externalJunctionTarget -PathType Container) `
        -Message 'junction rejection does not delete target'

    $dummyPath = Join-Path $repository 'scripts\perf\dashboard-load.cjs'
    $dummyOriginal = [System.IO.File]::ReadAllText($dummyPath, $script:Utf8NoBom)
    Write-Utf8File -LiteralPath $dummyPath -Value ($dummyOriginal + "// dirty`n")
    $dirtyExecution = Invoke-ContractRunner -Repository $repository
    Assert-SafeFailure -Execution $dirtyExecution -ExpectedCode 'source-not-clean'
    Write-Utf8File -LiteralPath $dummyPath -Value $dummyOriginal
    $cleanStatus = (& $gitPath -C $repository status --porcelain=v1 --untracked-files=all) -join ''
    Assert-Equal -Actual $cleanStatus -Expected '' -Message 'dirty-source test restores clean repository'

    $outputParent = Get-Item -LiteralPath (Join-Path $repository 'output') -Force
    $currentSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
    $systemSid = [System.Security.Principal.SecurityIdentifier]::new(
        [System.Security.Principal.WellKnownSidType]::LocalSystemSid,
        $null)
    $outputParentAcl = [System.IO.FileSystemAclExtensions]::GetAccessControl($outputParent)
    Assert-Equal -Actual $outputParentAcl.GetOwner(
        [System.Security.Principal.SecurityIdentifier]).Value -Expected $currentSid.Value `
        -Message 'ACL regression fixture parent is current-user owned'
    $outputParentAcl.SetAccessRuleProtection($true, $false)
    foreach ($rule in @($outputParentAcl.GetAccessRules(
        $true,
        $false,
        [System.Security.Principal.SecurityIdentifier]))) {
        [void]$outputParentAcl.RemoveAccessRuleSpecific($rule)
    }
    $allow = [System.Security.AccessControl.AccessControlType]::Allow
    $inherit = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
        [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
    $nonePropagation = [System.Security.AccessControl.PropagationFlags]::None
    [void]$outputParentAcl.AddAccessRule(
        [System.Security.AccessControl.FileSystemAccessRule]::new(
            $currentSid,
            [System.Security.AccessControl.FileSystemRights]::FullControl,
            [System.Security.AccessControl.InheritanceFlags]::None,
            $nonePropagation,
            $allow))
    [void]$outputParentAcl.AddAccessRule(
        [System.Security.AccessControl.FileSystemAccessRule]::new(
            $currentSid,
            [System.Security.AccessControl.FileSystemRights]::Modify,
            $inherit,
            $nonePropagation,
            $allow))
    [void]$outputParentAcl.AddAccessRule(
        [System.Security.AccessControl.FileSystemAccessRule]::new(
            $systemSid,
            [System.Security.AccessControl.FileSystemRights]::FullControl,
            $inherit,
            $nonePropagation,
            $allow))
    [System.IO.FileSystemAclExtensions]::SetAccessControl($outputParent, $outputParentAcl)
    $preexistingOutputRoot = [System.IO.Directory]::CreateDirectory(
        (Join-Path $repository 'output\perf'))
    [void][System.IO.Directory]::CreateDirectory(
        (Join-Path $preexistingOutputRoot.FullName 'attempts'))
    $preexistingAcl = [System.IO.FileSystemAclExtensions]::GetAccessControl($preexistingOutputRoot)
    $normalizedModifyRights = [System.Security.AccessControl.FileSystemRights]::Modify -bor
        [System.Security.AccessControl.FileSystemRights]::Synchronize
    $inheritedModify = @($preexistingAcl.GetAccessRules(
        $true,
        $true,
        [System.Security.Principal.SecurityIdentifier]) | Where-Object {
            $_.IsInherited -and $_.IdentityReference.Value -ceq $currentSid.Value -and
            $_.FileSystemRights -eq $normalizedModifyRights
        })
    Assert-True -Condition ($inheritedModify.Count -ge 1) `
        -Message 'ACL regression fixture has inherited current-user Modify ACE'

    $success = Invoke-ContractRunner -Repository $repository
    if ($success.ExitCode -ne 0) {
        throw "stubbed Public Load failed: stdout=$($success.Stdout.Trim()) stderr=$($success.Stderr.Trim())"
    }
    Assert-Equal -Actual $success.ExitCode -Expected 0 -Message 'stubbed Public Load succeeds'
    Assert-Equal -Actual $success.Stderr -Expected '' -Message 'successful runner stderr is empty'
    Assert-True -Condition ($success.Stdout -match '^\{[^\r\n]+\}\r?\n$') `
        -Message 'successful runner emits one compact JSON line'
    $published = $success.Stdout.Trim() | ConvertFrom-Json -Depth 8
    Assert-ExactKeys -Value $published -Expected @(
        'attempt_id', 'attempt_dir', 'result_path', 'report_path', 'summary_path'
    ) -Message 'success stdout ABI'
    Assert-True -Condition ($published.attempt_id -cmatch '^[0-9a-f]{64}$') `
        -Message 'attempt id is a 256-bit lower-hex nonce'
    Assert-True -Condition ([System.IO.Path]::IsPathFullyQualified($published.attempt_dir)) `
        -Message 'attempt path is absolute'
    Assert-ExactRunnerAcl -LiteralPath (Join-Path $repository 'output\perf') `
        -Message 'runner output root ACL'
    Assert-ExactRunnerAcl -LiteralPath (Join-Path $repository 'output\perf\attempts') `
        -Message 'runner attempts root ACL'
    Assert-ExactRunnerAcl -LiteralPath $published.attempt_dir `
        -Message 'runner attempt ACL'
    foreach ($path in @($published.result_path, $published.report_path, $published.summary_path)) {
        Assert-True -Condition (Test-Path -LiteralPath $path -PathType Leaf) `
            -Message "published artifact exists: $path"
        Assert-CanonicalJsonArtifact -LiteralPath $path -Message "published artifact $([System.IO.Path]::GetFileName($path))"
    }
    $retainedResultBody = [System.IO.File]::ReadAllText(
        $published.result_path, $script:Utf8NoBom)
    Assert-True -Condition ($retainedResultBody -cnotmatch '(?i)nonce|cookie|authorization') `
        -Message 'retained load result excludes private handoff material'
    $retainedResult = $retainedResultBody | ConvertFrom-Json -Depth 32
    Assert-ExactKeys -Value $retainedResult.instance -Expected @(
        'revision', 'pid', 'fixture_mode', 'outbound_calls_before',
        'outbound_calls_after', 'browser_outbound_attempts'
    ) -Message 'retained load result instance schema'
    $summary = [System.IO.File]::ReadAllText($published.summary_path, $script:Utf8NoBom) | ConvertFrom-Json -Depth 32
    Assert-Equal -Actual $summary.schema_version -Expected 1 -Message 'summary schema version'
    Assert-Equal -Actual $summary.runner_version -Expected 'with-isolated-dashboard-v1' `
        -Message 'summary runner version'
    Assert-Equal -Actual $summary.exit_classification -Expected 'green' -Message 'summary is green'
    Assert-Equal -Actual $summary.workload.kind -Expected 'Load' -Message 'summary workload kind'
    Assert-Equal -Actual $summary.instance.fixture_mode -Expected 'Public' -Message 'summary fixture mode'
    $resultItem = Get-Item -LiteralPath $published.result_path -Force
    $reportItem = Get-Item -LiteralPath $published.report_path -Force
    Assert-Equal -Actual $summary.result.leaf -Expected $resultItem.Name `
        -Message 'summary result leaf binds the retained result artifact'
    Assert-Equal -Actual $summary.result.sha256 `
        -Expected ((Get-FileHash -LiteralPath $published.result_path -Algorithm SHA256).Hash.ToLowerInvariant()) `
        -Message 'summary result SHA-256 matches retained result artifact'
    Assert-Equal -Actual $summary.result.bytes -Expected $resultItem.Length `
        -Message 'summary result byte count matches retained result artifact'
    Assert-Equal -Actual $summary.report.leaf -Expected $reportItem.Name `
        -Message 'summary report leaf binds the retained report artifact'
    Assert-Equal -Actual $summary.report.sha256 `
        -Expected ((Get-FileHash -LiteralPath $published.report_path -Algorithm SHA256).Hash.ToLowerInvariant()) `
        -Message 'summary report SHA-256 matches retained report artifact'
    Assert-Equal -Actual $summary.report.bytes -Expected $reportItem.Length `
        -Message 'summary report byte count matches retained report artifact'
    Assert-Equal -Actual $summary.build_descendant_cleanup.terminated_allowlisted -Expected $false `
        -Message 'normal build needs no allowlisted helper cleanup'
    Assert-Equal -Actual @($summary.build_descendant_cleanup.helpers).Count -Expected 0 `
        -Message 'normal build has no terminated helper identities'
    foreach ($rssPhase in @('after_ready', 'after_load', 'before_shutdown')) {
        Assert-True -Condition ([int64]$summary.process_rss_bytes.$rssPhase -gt 0) `
            -Message "RSS phase $rssPhase is a positive byte count"
    }
    Assert-True -Condition ($summary.job_evidence.Count -ge 11) `
        -Message 'summary retains start/exit Job evidence for every child phase'
    foreach ($row in $summary.job_evidence) {
        Assert-ExactKeys -Value $row -Expected @(
            'name', 'phase', 'direct_pid', 'creation_file_time_utc', 'is_process_in_job',
            'active_processes', 'total_processes', 'terminated_processes', 'active_process_ids'
        ) -Message "Job evidence $($row.name)/$($row.phase)"
        Assert-Equal -Actual $row.is_process_in_job -Expected $true `
            -Message "Job membership $($row.name)/$($row.phase)"
        if ($row.phase -ceq 'exited') {
            Assert-Equal -Actual $row.active_processes -Expected 0 `
                -Message "exited Job cardinality $($row.name)"
        }
    }
    Assert-Equal -Actual $summary.coverage.status -Expected 'partial' `
        -Message 'checkpoint does not claim unavailable instrumentation'
    Assert-Equal -Actual $summary.coverage.unavailable_metrics.Count -Expected 2 `
        -Message 'waiter and cache instrumentation gaps are explicit'
    Assert-Equal -Actual $summary.coverage.unsupported_boundary.readonly_playwright_proof.playwright_version `
        -Expected '1.58.2' -Message 'future browser proof pins Playwright ABI'
    Assert-Equal -Actual $summary.coverage.unsupported_boundary.readonly_playwright_proof.browser_artifacts_prelaunch `
        -Expected 'must-be-absent' -Message 'future browser artifacts are absent before launch'
    Assert-Equal -Actual $summary.coverage.unsupported_boundary.readonly_playwright_proof.node_environment `
        -Expected 'minimal-child-allowlist' -Message 'future browser Node starts from minimal environment'
    foreach ($revision in @(
        $summary.revision_binding.build_environment_revision,
        $summary.revision_binding.runtime_environment_revision,
        $summary.revision_binding.ready_revision,
        $summary.revision_binding.instance_before_revision,
        $summary.revision_binding.instance_after_revision
    )) {
        Assert-Equal -Actual $revision -Expected $summary.source_state.head `
            -Message 'build/runtime/ready/instance revision binding matches clean HEAD'
    }
    foreach ($property in @(
        'graceful', 'descendants_closed', 'original_process_absent', 'port_closed',
        'temporary_files_absent'
    )) {
        Assert-Equal -Actual $summary.teardown.$property -Expected $true `
            -Message "teardown proof $property"
    }
    Assert-Equal -Actual $summary.teardown.job_active_processes -Expected 0 `
        -Message 'Job has zero active processes after graceful shutdown'
    foreach ($phase in @('before', 'after')) {
        foreach ($counter in @(
            'denied_requests', 'server_write_attempts', 'repository_reads', 'repository_mutations',
            'outbound_calls', 'browser_outbound_attempts', 'provider_guild_lookups'
        )) {
            Assert-Equal -Actual $summary.counters.$phase.$counter -Expected 0 `
                -Message "$phase $counter is zero"
        }
    }
    $leaves = @(Get-ChildItem -LiteralPath $published.attempt_dir -Force)
    Assert-Equal -Actual $leaves.Count -Expected 4 -Message 'success attempt has exact four retained leaves'
    Assert-Equal -Actual (@($leaves.Name | Sort-Object) -join '|') -Expected (
        @(
            '.dynamo-perf-attempt-v1.json',
            'contract-public-budget.json',
            'contract-public-result.json',
            'contract-public-summary.json'
        ) -join '|'
    ) -Message 'success attempt retains only the expected artifact leaves'
    Assert-True -Condition (-not ($leaves.Name -match '\.tmp$')) `
        -Message 'success attempt retains no temporary file'
    $successAttemptCount = @(Get-ChildItem -LiteralPath (Join-Path $repository 'output\perf\attempts') `
        -Directory -Force).Count
    Assert-Equal -Actual $successAttemptCount -Expected 1 -Message 'one successful attempt retained'

    $shortDescendantScenario = 'short-job-descendant-' + [Guid]::NewGuid().ToString('N')
    $shortDescendantRecordPath = Get-DescendantRecordPath -Scenario $shortDescendantScenario
    try {
        $shortDescendantExecution = Invoke-ContractRunner -Repository $repository `
            -Scenario $shortDescendantScenario
        if ($shortDescendantExecution.ExitCode -ne 0) {
            throw "short descendant run failed: stdout=$($shortDescendantExecution.Stdout.Trim()) stderr=$($shortDescendantExecution.Stderr.Trim())"
        }
        Assert-Equal -Actual $shortDescendantExecution.ExitCode -Expected 0 `
            -Message 'short same-Job descendant drains naturally'
        Assert-Equal -Actual $shortDescendantExecution.Stderr -Expected '' `
            -Message 'short descendant success has empty stderr'
        $shortDescendantPublished = $shortDescendantExecution.Stdout.Trim() |
            ConvertFrom-Json -Depth 8
        Assert-True -Condition (Test-Path -LiteralPath $shortDescendantPublished.attempt_dir `
            -PathType Container) -Message 'short descendant success retains its attempt'
        $shortDescendantSummary = [System.IO.File]::ReadAllText(
            $shortDescendantPublished.summary_path, $script:Utf8NoBom) | ConvertFrom-Json -Depth 32
        $shortBuildStarted = @($shortDescendantSummary.job_evidence | Where-Object {
            $_.name -ceq 'build' -and $_.phase -ceq 'started'
        })
        $shortBuildExited = @($shortDescendantSummary.job_evidence | Where-Object {
            $_.name -ceq 'build' -and $_.phase -ceq 'exited'
        })
        Assert-Equal -Actual $shortBuildStarted.Count -Expected 1 `
            -Message 'short descendant summary has one build-started row'
        Assert-Equal -Actual $shortBuildExited.Count -Expected 1 `
            -Message 'short descendant summary has one build-exited row'
        Assert-Equal -Actual $shortBuildExited[0].direct_pid -Expected $shortBuildStarted[0].direct_pid `
            -Message 'short descendant drain preserves direct PID identity'
        Assert-Equal -Actual $shortBuildExited[0].creation_file_time_utc `
            -Expected $shortBuildStarted[0].creation_file_time_utc `
            -Message 'short descendant drain preserves direct process birth identity'
        Assert-Equal -Actual $shortBuildStarted[0].is_process_in_job -Expected $true `
            -Message 'short descendant build starts in the isolated Job'
        Assert-Equal -Actual $shortBuildExited[0].is_process_in_job -Expected $true `
            -Message 'short descendant final evidence retains Job membership proof'
        Assert-True -Condition ([int64]$shortBuildExited[0].total_processes -ge 2) `
            -Message 'short descendant was observed in the same Job accounting'
        Assert-Equal -Actual $shortBuildExited[0].active_processes -Expected 0 `
            -Message 'short descendant final Job evidence is active-zero'
        Assert-Equal -Actual @($shortBuildExited[0].active_process_ids).Count -Expected 0 `
            -Message 'short descendant final Job PID list is empty'
        $shortDescendantRecord = Read-DescendantRecord -LiteralPath $shortDescendantRecordPath
        Assert-Equal -Actual $shortDescendantRecord.delay_milliseconds -Expected 750 `
            -Message 'short descendant fixture uses a bounded natural lifetime'
        Assert-RecordedProcessAbsent -Record $shortDescendantRecord `
            -Message 'short descendant identity is absent after successful drain'
    }
    finally {
        if (Test-Path -LiteralPath $shortDescendantRecordPath) {
            Remove-Item -LiteralPath $shortDescendantRecordPath -Force
        }
    }
    Assert-True -Condition (-not (Test-Path -LiteralPath $shortDescendantRecordPath)) `
        -Message 'short descendant identity record leaves no residue'
    $successAttemptCount = @(Get-ChildItem -LiteralPath (Join-Path $repository 'output\perf\attempts') `
        -Directory -Force).Count
    Assert-Equal -Actual $successAttemptCount -Expected 2 `
        -Message 'normal and naturally drained successes are retained'
    Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $repository `
        'output\perf\diagnostics'))) -Message 'diagnostics are absent without explicit opt-in'

    $allowlistedScenario = 'allowlisted-job-descendant-' + [Guid]::NewGuid().ToString('N')
    $allowlistedRecordPath = Get-DescendantRecordPath -Scenario $allowlistedScenario
    $allowlistedClock = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        $allowlistedExecution = Invoke-ContractRunner -Repository $repository `
            -Scenario $allowlistedScenario
        $allowlistedClock.Stop()
        if ($allowlistedExecution.ExitCode -ne 0) {
            throw "allowlisted descendant run failed: stdout=$($allowlistedExecution.Stdout.Trim()) stderr=$($allowlistedExecution.Stderr.Trim())"
        }
        Assert-Equal -Actual $allowlistedExecution.Stderr -Expected '' `
            -Message 'allowlisted descendant success has empty stderr'
        Assert-True -Condition ($allowlistedClock.ElapsedMilliseconds -ge 4500 -and
            $allowlistedClock.ElapsedMilliseconds -lt 20000) `
            -Message 'allowlisted descendant cleanup is delayed and bounded'
        $allowlistedPublished = $allowlistedExecution.Stdout.Trim() | ConvertFrom-Json -Depth 8
        $allowlistedSummary = [System.IO.File]::ReadAllText(
            $allowlistedPublished.summary_path, $script:Utf8NoBom) | ConvertFrom-Json -Depth 32
        Assert-Equal -Actual $allowlistedSummary.build_descendant_cleanup.terminated_allowlisted `
            -Expected $true -Message 'allowlisted build helper is terminated explicitly'
        $allowlistedHelperNames = @($allowlistedSummary.build_descendant_cleanup.helpers)
        Assert-True -Condition ($allowlistedHelperNames.Count -ge 1 -and
            $allowlistedHelperNames.Count -le 2) `
            -Message 'allowlisted build helper list has bounded identities'
        Assert-True -Condition ($allowlistedHelperNames -ccontains 'pwsh') `
            -Message 'contract process helper identity is retained honestly'
        Assert-True -Condition (@($allowlistedHelperNames | Where-Object {
            $_ -cnotin @('pwsh', 'conhost')
        }).Count -eq 0) -Message 'contract helper identities are allowlisted'
        $allowlistedBuildExit = @($allowlistedSummary.job_evidence | Where-Object {
            $_.name -ceq 'build' -and $_.phase -ceq 'exited'
        })
        Assert-Equal -Actual $allowlistedBuildExit.Count -Expected 1 `
            -Message 'allowlisted build has one exit evidence row'
        Assert-Equal -Actual $allowlistedBuildExit[0].active_processes -Expected 0 `
            -Message 'allowlisted build cleanup reaches active zero'
        $allowlistedRecord = Read-DescendantRecord -LiteralPath $allowlistedRecordPath
        Assert-Equal -Actual $allowlistedRecord.delay_milliseconds -Expected 30000 `
            -Message 'allowlisted contract descendant requires explicit cleanup'
        Assert-RecordedProcessAbsent -Record $allowlistedRecord `
            -Message 'allowlisted contract descendant is absent after cleanup'
    }
    finally {
        $allowlistedClock.Stop()
        if (Test-Path -LiteralPath $allowlistedRecordPath) {
            Remove-Item -LiteralPath $allowlistedRecordPath -Force
        }
    }
    Assert-True -Condition (-not (Test-Path -LiteralPath $allowlistedRecordPath)) `
        -Message 'allowlisted descendant identity record leaves no residue'
    $successAttemptCount = @(Get-ChildItem -LiteralPath (Join-Path $repository `
        'output\perf\attempts') -Directory -Force).Count
    Assert-Equal -Actual $successAttemptCount -Expected 3 `
        -Message 'normal, natural drain, and allowlisted cleanup successes are retained'
    Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $repository `
        'output\perf\diagnostics'))) `
        -Message 'allowlisted cleanup does not publish a failure diagnostic'

    $longDescendantScenario = 'long-job-descendant-' + [Guid]::NewGuid().ToString('N')
    $longDescendantRecordPath = Get-DescendantRecordPath -Scenario $longDescendantScenario
    $longDescendantClock = [System.Diagnostics.Stopwatch]::StartNew()
    $diagnosticsRoot = Join-Path $repository 'output\perf\diagnostics'
    try {
        $longDescendantExecution = Invoke-ContractRunner -Repository $repository `
            -Scenario $longDescendantScenario `
            -AdditionalEnvironment @{ DYNAMO_PERF_DIAGNOSTICS = '1' }
        $longDescendantClock.Stop()
        Assert-SafeFailure -Execution $longDescendantExecution `
            -ExpectedCode 'child-descendants-survived'
        Assert-True -Condition ($longDescendantClock.ElapsedMilliseconds -ge 4500) `
            -Message 'long descendant is given the natural drain interval'
        Assert-True -Condition ($longDescendantClock.ElapsedMilliseconds -lt 20000) `
            -Message 'long descendant failure is bounded'
        $longDescendantRecord = Read-DescendantRecord -LiteralPath $longDescendantRecordPath
        Assert-Equal -Actual $longDescendantRecord.delay_milliseconds -Expected 30000 `
            -Message 'long descendant outlives the drain interval without cleanup'
        Assert-RecordedProcessAbsent -Record $longDescendantRecord `
            -Message 'long descendant identity is absent after failure cleanup'
        $postLongAttemptCount = @(Get-ChildItem `
            -LiteralPath (Join-Path $repository 'output\perf\attempts') -Directory -Force).Count
        Assert-Equal -Actual $postLongAttemptCount -Expected $successAttemptCount `
            -Message 'long descendant failure removes its owned attempt'
        Assert-True -Condition (Test-Path -LiteralPath $diagnosticsRoot -PathType Container) `
            -Message 'opt-in descendant diagnostics root exists'
        Assert-ExactRunnerAcl -LiteralPath $diagnosticsRoot `
            -Message 'descendant diagnostics root ACL'
        $diagnosticLeaves = @(Get-ChildItem -LiteralPath $diagnosticsRoot -File -Force)
        Assert-Equal -Actual $diagnosticLeaves.Count -Expected 1 `
            -Message 'one descendant diagnostic is retained'
        Assert-True -Condition ($diagnosticLeaves[0].Name -cmatch `
            '^[0-9a-f]{64}-build-descendants\.json$') `
            -Message 'descendant diagnostic leaf is attempt-bound'
        Assert-SafeDiagnosticLeaf -File $diagnosticLeaves[0] `
            -Message 'descendant diagnostic leaf'
        $diagnosticBody = [System.IO.File]::ReadAllText(
            $diagnosticLeaves[0].FullName, $script:Utf8NoBom)
        Assert-True -Condition (-not $diagnosticBody.Contains($script:SecretSentinel)) `
            -Message 'descendant diagnostic excludes caller secret sentinel'
        Assert-True -Condition ($diagnosticBody -cnotmatch '(?i)command.?line|environment') `
            -Message 'descendant diagnostic excludes command line and environment'
        $diagnostic = $diagnosticBody | ConvertFrom-Json -Depth 16
        Assert-ExactKeys -Value $diagnostic -Expected @(
            'schema_version', 'failure_code', 'process_role', 'direct_pid',
            'direct_creation_file_time_utc', 'samples'
        ) -Message 'descendant diagnostic schema'
        Assert-Equal -Actual $diagnostic.schema_version -Expected 1 `
            -Message 'descendant diagnostic version'
        Assert-Equal -Actual $diagnostic.failure_code -Expected 'child-descendants-survived' `
            -Message 'descendant diagnostic failure code'
        Assert-Equal -Actual $diagnostic.process_role -Expected 'build' `
            -Message 'descendant diagnostic process role'
        Assert-Equal -Actual @($diagnostic.samples).Count -Expected 2 `
            -Message 'descendant diagnostic has direct-exit and deadline samples'
        foreach ($sample in @($diagnostic.samples)) {
            Assert-ExactKeys -Value $sample -Expected @(
                'phase', 'observed_elapsed_ms', 'active_processes',
                'captured_processes', 'truncated', 'processes'
            ) -Message 'descendant diagnostic sample schema'
            Assert-Equal -Actual $sample.truncated -Expected $false `
                -Message 'long fixture diagnostic is complete'
            Assert-Equal -Actual $sample.captured_processes `
                -Expected @($sample.processes).Count `
                -Message 'captured process count matches rows'
        }
        Assert-Equal -Actual $diagnostic.samples[0].phase -Expected 'direct-exit' `
            -Message 'descendant diagnostic first phase'
        Assert-Equal -Actual $diagnostic.samples[1].phase -Expected 'drain-deadline' `
            -Message 'descendant diagnostic final phase'
        $diagnosedProcesses = @($diagnostic.samples | ForEach-Object { @($_.processes) })
        Assert-True -Condition ($diagnosedProcesses.Count -ge 2) `
            -Message 'descendant diagnostic records the survivor in both samples'
        foreach ($diagnosed in $diagnosedProcesses) {
            Assert-ExactKeys -Value $diagnosed -Expected @(
                'pid', 'creation_file_time_utc', 'name', 'executable_path', 'parent_pid',
                'observed_elapsed_ms', 'phase', 'observed_in_initial_job_snapshot',
                'job_member', 'query_status', 'parent_query_status'
            ) -Message 'descendant process diagnostic schema'
            Assert-Equal -Actual $diagnosed.observed_in_initial_job_snapshot -Expected $true `
                -Message 'diagnosed process came from the initial Job snapshot'
            Assert-Equal -Actual $diagnosed.job_member -Expected $true `
                -Message 'diagnosed process is a Job member'
        }
        $queryableProcesses = @($diagnosedProcesses | Where-Object { $_.query_status -clike 'ok*' })
        Assert-True -Condition ($queryableProcesses.Count -ge 1) `
            -Message 'at least one descendant identity is queryable'
        Assert-True -Condition (@($queryableProcesses | Where-Object {
            $_.name -ceq 'pwsh' -and $_.executable_path -cmatch '(?i)\\pwsh\.exe$' -and
            [uint64]$_.creation_file_time_utc -gt 0
        }).Count -ge 1) -Message 'long fixture identifies its pwsh descendant safely'
    }
    finally {
        $longDescendantClock.Stop()
        if (Test-Path -LiteralPath $longDescendantRecordPath) {
            Remove-Item -LiteralPath $longDescendantRecordPath -Force
        }
        if (Test-Path -LiteralPath $diagnosticsRoot -PathType Container) {
            Remove-Item -LiteralPath $diagnosticsRoot -Recurse -Force
        }
    }
    Assert-True -Condition (-not (Test-Path -LiteralPath $longDescendantRecordPath)) `
        -Message 'long descendant identity record leaves no residue'
    Assert-True -Condition (-not (Test-Path -LiteralPath $diagnosticsRoot)) `
        -Message 'contract descendant diagnostic leaves no residue'

    $harnessShortScenario = 'harness-short-descendant-' + [Guid]::NewGuid().ToString('N')
    $harnessShortRecordPath = Get-DescendantRecordPath -Scenario $harnessShortScenario
    try {
        $harnessShortExecution = Invoke-ContractRunner -Repository $repository `
            -Scenario $harnessShortScenario
        if ($harnessShortExecution.ExitCode -ne 0) {
            throw "short harness descendant run failed: stdout=$($harnessShortExecution.Stdout.Trim()) stderr=$($harnessShortExecution.Stderr.Trim())"
        }
        Assert-Equal -Actual $harnessShortExecution.Stderr -Expected '' `
            -Message 'short harness descendant success has empty stderr'
        $harnessShortPublished = $harnessShortExecution.Stdout.Trim() | ConvertFrom-Json -Depth 8
        $harnessShortSummary = [System.IO.File]::ReadAllText(
            $harnessShortPublished.summary_path, $script:Utf8NoBom) | ConvertFrom-Json -Depth 32
        $harnessShortExit = @($harnessShortSummary.job_evidence | Where-Object {
            $_.name -ceq 'harness' -and $_.phase -ceq 'exited'
        })
        Assert-Equal -Actual $harnessShortExit.Count -Expected 1 `
            -Message 'short harness descendant has one exit evidence row'
        Assert-Equal -Actual $harnessShortExit[0].active_processes -Expected 0 `
            -Message 'short harness descendant drains to active zero'
        $harnessShortRecord = Read-DescendantRecord -LiteralPath $harnessShortRecordPath
        Assert-Equal -Actual $harnessShortRecord.delay_milliseconds -Expected 750 `
            -Message 'short harness descendant has a bounded natural lifetime'
        Assert-RecordedProcessAbsent -Record $harnessShortRecord `
            -Message 'short harness descendant is absent after successful drain'
    }
    finally {
        if (Test-Path -LiteralPath $harnessShortRecordPath) {
            Remove-Item -LiteralPath $harnessShortRecordPath -Force
        }
    }
    Assert-True -Condition (-not (Test-Path -LiteralPath $harnessShortRecordPath)) `
        -Message 'short harness descendant identity record leaves no residue'
    $successAttemptCount = @(Get-ChildItem -LiteralPath (Join-Path $repository `
        'output\perf\attempts') -Directory -Force).Count
    Assert-Equal -Actual $successAttemptCount -Expected 4 `
        -Message 'short harness drain adds one retained success'

    $harnessDescendantScenario = 'harness-long-descendant-' + [Guid]::NewGuid().ToString('N')
    $harnessDescendantRecordPath = Get-DescendantRecordPath -Scenario $harnessDescendantScenario
    $harnessDescendantClock = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        $harnessDescendantExecution = Invoke-ContractRunner -Repository $repository `
            -Scenario $harnessDescendantScenario `
            -AdditionalEnvironment @{ DYNAMO_PERF_DIAGNOSTICS = '1' }
        $harnessDescendantClock.Stop()
        Assert-SafeFailure -Execution $harnessDescendantExecution `
            -ExpectedCode 'teardown-descendants-survived'
        Assert-True -Condition ($harnessDescendantClock.ElapsedMilliseconds -ge 4500 -and
            $harnessDescendantClock.ElapsedMilliseconds -lt 20000) `
            -Message 'harness descendant receives a bounded natural drain interval'
        $harnessDescendantRecord = Read-DescendantRecord `
            -LiteralPath $harnessDescendantRecordPath
        Assert-RecordedProcessAbsent -Record $harnessDescendantRecord `
            -Message 'harness descendant identity is absent after failure cleanup'
        $harnessDiagnosticLeaves = @(Get-ChildItem -LiteralPath $diagnosticsRoot -File -Force)
        Assert-Equal -Actual $harnessDiagnosticLeaves.Count -Expected 1 `
            -Message 'one harness descendant diagnostic is retained'
        Assert-True -Condition ($harnessDiagnosticLeaves[0].Name -cmatch `
            '^[0-9a-f]{64}-harness-descendants\.json$') `
            -Message 'harness descendant diagnostic is role-bound'
        Assert-SafeDiagnosticLeaf -File $harnessDiagnosticLeaves[0] `
            -Message 'harness descendant diagnostic leaf'
        $harnessDiagnosticBody = [System.IO.File]::ReadAllText(
            $harnessDiagnosticLeaves[0].FullName, $script:Utf8NoBom)
        Assert-True -Condition (-not $harnessDiagnosticBody.Contains($script:SecretSentinel)) `
            -Message 'harness descendant diagnostic excludes caller secret sentinel'
        $harnessDiagnostic = $harnessDiagnosticBody | ConvertFrom-Json -Depth 16
        Assert-Equal -Actual $harnessDiagnostic.failure_code `
            -Expected 'teardown-descendants-survived' `
            -Message 'harness descendant diagnostic failure code'
        Assert-Equal -Actual $harnessDiagnostic.process_role -Expected 'harness' `
            -Message 'harness descendant diagnostic process role'
        Assert-Equal -Actual @($harnessDiagnostic.samples).Count -Expected 1 `
            -Message 'harness descendant diagnostic has one shutdown sample'
        Assert-Equal -Actual $harnessDiagnostic.samples[0].phase -Expected 'graceful-shutdown' `
            -Message 'harness descendant diagnostic phase'
        Assert-True -Condition ([int64]$harnessDiagnostic.samples[0].observed_elapsed_ms -ge 4500) `
            -Message 'harness descendant diagnostic records the drain deadline'
        $postHarnessAttemptCount = @(Get-ChildItem -LiteralPath (Join-Path $repository `
            'output\perf\attempts') -Directory -Force).Count
        Assert-Equal -Actual $postHarnessAttemptCount -Expected $successAttemptCount `
            -Message 'harness descendant failure removes its owned attempt'
    }
    finally {
        $harnessDescendantClock.Stop()
        if (Test-Path -LiteralPath $harnessDescendantRecordPath) {
            Remove-Item -LiteralPath $harnessDescendantRecordPath -Force
        }
        if (Test-Path -LiteralPath $diagnosticsRoot -PathType Container) {
            Remove-Item -LiteralPath $diagnosticsRoot -Recurse -Force
        }
    }
    Assert-True -Condition (-not (Test-Path -LiteralPath $harnessDescendantRecordPath)) `
        -Message 'harness descendant identity record leaves no residue'
    Assert-True -Condition (-not (Test-Path -LiteralPath $diagnosticsRoot)) `
        -Message 'harness descendant diagnostic fixture leaves no residue'

    $invalidDiagnosticSetting = Invoke-ContractRunner -Repository $repository `
        -AdditionalEnvironment @{ DYNAMO_PERF_DIAGNOSTICS = '0' }
    Assert-SafeFailure -Execution $invalidDiagnosticSetting `
        -ExpectedCode 'diagnostic-setting-invalid'
    Assert-True -Condition (-not (Test-Path -LiteralPath $diagnosticsRoot)) `
        -Message 'invalid diagnostic setting has no output side effect'

    $launchFailureModulePath = Join-Path $repository 'scripts\perf\isolated-process-job.psm1'
    $launchFailureModuleOriginal = [System.IO.File]::ReadAllText(
        $launchFailureModulePath, $script:Utf8NoBom)
    $startFailureInjection = @'

function Start-DynamoIsolatedProcess {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $ExecutablePath,
        [Parameter(Mandatory)][object[]] $ArgumentList,
        [Parameter(Mandatory)][string] $WorkingDirectory,
        [Parameter(Mandatory)][object] $Environment,
        [Parameter(Mandatory)][string] $StandardOutputPath,
        [Parameter(Mandatory)][string] $StandardErrorPath
    )
    throw [System.InvalidOperationException]::new('contract launch failure')
}
Export-ModuleMember -Function Start-DynamoIsolatedProcess
'@
    Write-Utf8File -LiteralPath $launchFailureModulePath `
        -Value ($launchFailureModuleOriginal + $startFailureInjection + "`n")
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository `
        -Arguments @('add', '--', 'scripts/perf/isolated-process-job.psm1'))
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository `
        -Arguments @('commit', '-q', '-m', 'inject child launch failure'))
    $launchFailure = Invoke-ContractRunner -Repository $repository `
        -AdditionalEnvironment @{ DYNAMO_PERF_DIAGNOSTICS = '1' }
    Assert-SafeFailure -Execution $launchFailure -ExpectedCode 'child-launch-failed'
    $launchDiagnosticLeaves = @(Get-ChildItem -LiteralPath $diagnosticsRoot -File -Force)
    Assert-Equal -Actual $launchDiagnosticLeaves.Count -Expected 1 `
        -Message 'one child launch diagnostic is retained'
    Assert-True -Condition ($launchDiagnosticLeaves[0].Name -cmatch `
        '^[0-9a-f]{64}-build-launch\.json$') `
        -Message 'child launch diagnostic is role-bound'
    Assert-SafeDiagnosticLeaf -File $launchDiagnosticLeaves[0] `
        -Message 'child launch diagnostic leaf'
    $launchDiagnosticBody = [System.IO.File]::ReadAllText(
        $launchDiagnosticLeaves[0].FullName, $script:Utf8NoBom)
    Assert-True -Condition (-not $launchDiagnosticBody.Contains($script:SecretSentinel)) `
        -Message 'child launch diagnostic excludes caller secret sentinel'
    Assert-True -Condition ($launchDiagnosticBody -cnotmatch '(?i)command.?line|environment') `
        -Message 'child launch diagnostic excludes command line and environment'
    $launchDiagnostic = $launchDiagnosticBody | ConvertFrom-Json -Depth 8
    Assert-ExactKeys -Value $launchDiagnostic -Expected @(
        'schema_version', 'failure_code', 'process_role', 'exception_type',
        'exception_hresult', 'inner_exception_type', 'inner_exception_hresult'
    ) -Message 'child launch diagnostic schema'
    Assert-Equal -Actual $launchDiagnostic.failure_code -Expected 'child-launch-failed' `
        -Message 'child launch diagnostic failure code'
    Assert-Equal -Actual $launchDiagnostic.process_role -Expected 'build' `
        -Message 'child launch diagnostic process role'
    Remove-Item -LiteralPath $diagnosticsRoot -Recurse -Force
    Assert-True -Condition (-not (Test-Path -LiteralPath $diagnosticsRoot)) `
        -Message 'child launch diagnostic fixture is removed'
    Write-Utf8File -LiteralPath $launchFailureModulePath -Value $launchFailureModuleOriginal
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository `
        -Arguments @('add', '--', 'scripts/perf/isolated-process-job.psm1'))
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository `
        -Arguments @('commit', '-q', '-m', 'restore child launch implementation'))

    $negativeScenarios = [ordered]@{
        'wrong-ready-nonce' = 'ready-identity-mismatch'
        'counters-drift' = 'counter-drift-detected'
        'budget-fail' = 'budget-failed'
        'ready-timeout' = 'ready-timeout'
        'shutdown-fail' = 'control-status-mismatch'
    }
    foreach ($entry in $negativeScenarios.GetEnumerator()) {
        $execution = Invoke-ContractRunner -Repository $repository -Scenario $entry.Key
        Assert-SafeFailure -Execution $execution -ExpectedCode $entry.Value
        $attemptCount = @(Get-ChildItem -LiteralPath (Join-Path $repository 'output\perf\attempts') `
            -Directory -Force).Count
        Assert-Equal -Actual $attemptCount -Expected $successAttemptCount `
            -Message "failed scenario $($entry.Key) removes its owned attempt"
    }

    $junctionCleanup = Invoke-ContractRunner -Repository $repository -Scenario 'cleanup-junction'
    Assert-SafeFailure -Execution $junctionCleanup -ExpectedCode 'attempt-cleanup-failed'
    $attemptDirectories = @(Get-ChildItem -LiteralPath (Join-Path $repository 'output\perf\attempts') `
        -Directory -Force)
    Assert-Equal -Actual $attemptDirectories.Count -Expected ($successAttemptCount + 1) `
        -Message 'unsafe child junction preserves the failed attempt instead of recursive deletion'
    $successfulAttemptPaths = @(
        $published.attempt_dir,
        $shortDescendantPublished.attempt_dir,
        $allowlistedPublished.attempt_dir,
        $harnessShortPublished.attempt_dir
    )
    $preservedCandidates = @($attemptDirectories | Where-Object {
        $successfulAttemptPaths -cnotcontains $_.FullName
    })
    Assert-Equal -Actual $preservedCandidates.Count -Expected 1 `
        -Message 'exactly one unsafe failed attempt is preserved'
    $preserved = $preservedCandidates[0]
    $preservedJunction = Join-Path $preserved.FullName '.contract-child-junction'
    Assert-True -Condition (([System.IO.File]::GetAttributes($preservedJunction) -band `
        [System.IO.FileAttributes]::ReparsePoint) -ne 0) -Message 'preserved fixture is a reparse point'
    Assert-True -Condition (Test-Path -LiteralPath ([System.IO.Path]::GetTempPath()) -PathType Container) `
        -Message 'reparse target survives rejected recursive cleanup'
    Remove-Item -LiteralPath $preservedJunction -Force
    Assert-True -Condition (-not (Test-Path -LiteralPath $preservedJunction)) `
        -Message 'contract removes only the junction leaf after proof'
    Remove-Item -LiteralPath $preserved.FullName -Recurse -Force
    $restoredAttemptCount = @(Get-ChildItem -LiteralPath (Join-Path $repository 'output\perf\attempts') `
        -Directory -Force).Count
    Assert-Equal -Actual $restoredAttemptCount -Expected $successAttemptCount `
        -Message 'contract fixture cleanup restores retained-attempt count'

    $modulePath = Join-Path $repository 'scripts\perf\isolated-process-job.psm1'
    $moduleOriginal = [System.IO.File]::ReadAllText($modulePath, $script:Utf8NoBom)
    $stopFailureInjection = @'

function Stop-DynamoIsolatedProcess {
    [CmdletBinding()]
    param([Parameter(Mandatory)][object] $Process, [uint32] $ExitCode = 3758161936)
    throw 'contract injected Stop-DynamoIsolatedProcess failure'
}
Export-ModuleMember -Function Stop-DynamoIsolatedProcess
'@
    Write-Utf8File -LiteralPath $modulePath -Value ($moduleOriginal + $stopFailureInjection + "`n")
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository `
        -Arguments @('add', '--', 'scripts/perf/isolated-process-job.psm1'))
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository `
        -Arguments @('commit', '-q', '-m', 'inject teardown API fault'))
    $teardownApiFailure = Invoke-ContractRunner -Repository $repository -Scenario 'ready-timeout'
    Assert-SafeFailure -Execution $teardownApiFailure -ExpectedCode 'teardown-child-cleanup-failed'
    $postFaultAttemptCount = @(Get-ChildItem -LiteralPath (Join-Path $repository 'output\perf\attempts') `
        -Directory -Force).Count
    Assert-Equal -Actual $postFaultAttemptCount -Expected $successAttemptCount `
        -Message 'teardown API fault uses direct Job fallback and removes owned attempt'
    Write-Utf8File -LiteralPath $modulePath -Value $moduleOriginal
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository `
        -Arguments @('add', '--', 'scripts/perf/isolated-process-job.psm1'))
    [void](Invoke-GitChecked -GitPath $gitPath -Repository $repository `
        -Arguments @('commit', '-q', '-m', 'restore teardown API'))
    $postFaultStatus = (& $gitPath -C $repository status --porcelain=v1 --untracked-files=all) -join ''
    Assert-Equal -Actual $postFaultStatus -Expected '' `
        -Message 'teardown fault injection restores a clean source state'

    foreach ($file in Get-ChildItem -LiteralPath $repository -Recurse -File -Force) {
        $bytes = [System.IO.File]::ReadAllBytes($file.FullName)
        $text = [System.Text.Encoding]::UTF8.GetString($bytes)
        Assert-True -Condition (-not $text.Contains($script:SecretSentinel)) `
            -Message "caller secret sentinel absent from $($file.FullName)"
    }
    $retainedText = (Get-ChildItem -LiteralPath $published.attempt_dir -File -Force | ForEach-Object {
        [System.IO.File]::ReadAllText($_.FullName, $script:Utf8NoBom)
    }) -join "`n"
    Assert-True -Condition ($retainedText -notmatch 'perf_[0-9a-f]{64}') `
        -Message 'control cookie is absent from retained artifacts'
}
finally {
    if (Test-Path -LiteralPath $contractRoot) {
        $fullContractRoot = [System.IO.Path]::GetFullPath($contractRoot)
        $safePrefix = "$temporaryBase\"
        if (-not $fullContractRoot.StartsWith($safePrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
            (Split-Path -Leaf $fullContractRoot) -notmatch '^dynamo-isolated-dashboard-[0-9a-f]{32}$') {
            throw 'refusing unsafe contract temp cleanup'
        }
        Remove-Item -LiteralPath $fullContractRoot -Recurse -Force
    }
}

[Console]::Out.WriteLine("isolated-dashboard-contract: PASS ($($script:Assertions) assertions)")
