[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$runnerPath = Join-Path $repoRoot 'scripts/test-isolated-mongo.ps1'
$rustPath = Join-Path $repoRoot 'crates/persistence-mongo/src/lib.rs'
$rustTestSupportPath = Join-Path $repoRoot 'crates/persistence-mongo/src/tests/support.rs'
$rustDashboardAuditTestPath = Join-Path $repoRoot 'crates/persistence-mongo/src/tests/dashboard_audit.rs'
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("dynamo-mongo-contract-{0}" -f [Guid]::NewGuid().ToString('N'))
$cargoStubCommand = 'Invoke-DynamoMongoCargoStub'
$logPath = Join-Path $tempRoot 'cargo-calls.jsonl'

$managedEnvironment = @(
    'DYNAMO_MONGO_TEST',
    'MONGODB_TEST_URI',
    'MONGODB_URI_FOR_ISOLATED_TEST',
    'MONGODB_URI',
    'MONGO_CONNECTION',
    'MONGODB_DATABASE',
    'DYNAMO_PRODUCTION_MONGODB_URI',
    'MONGODB_PRODUCTION_URI',
    'DYNAMO_STUB_SCENARIO',
    'DYNAMO_STUB_LOG'
)

function Assert-Contract {
    param(
        [Parameter(Mandatory)] [bool] $Condition,
        [Parameter(Mandatory)] [string] $Message
    )

    if (-not $Condition) {
        throw "isolated Mongo contract failed: $Message"
    }
}

function Get-EnvironmentSnapshot {
    param([Parameter(Mandatory)] [string[]] $Names)

    $snapshot = [ordered]@{}
    foreach ($name in $Names) {
        $snapshot[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    }
    return $snapshot
}

function Restore-EnvironmentSnapshot {
    param([Parameter(Mandatory)] [Collections.IDictionary] $Snapshot)

    foreach ($entry in $Snapshot.GetEnumerator()) {
        $environmentPath = "Env:$([string]$entry.Key)"
        if ($null -eq $entry.Value) {
            Remove-Item -LiteralPath $environmentPath -Force -ErrorAction SilentlyContinue
        }
        else {
            Set-Item -LiteralPath $environmentPath -Value ([string]$entry.Value)
        }
    }
}

function Set-CaseEnvironment {
    param([Parameter(Mandatory)] [Collections.IDictionary] $Values)

    foreach ($name in $managedEnvironment) {
        Remove-Item -LiteralPath "Env:$name" -Force -ErrorAction SilentlyContinue
    }
    foreach ($entry in $Values.GetEnumerator()) {
        Set-Item -LiteralPath "Env:$([string]$entry.Key)" -Value ([string]$entry.Value)
    }
}

function Copy-CaseEnvironment {
    param(
        [Parameter(Mandatory)] [Collections.IDictionary] $Base,
        [Parameter(Mandatory)] [Collections.IDictionary] $Overrides
    )

    $copy = [ordered]@{}
    foreach ($entry in $Base.GetEnumerator()) {
        $copy[[string]$entry.Key] = $entry.Value
    }
    foreach ($entry in $Overrides.GetEnumerator()) {
        $copy[[string]$entry.Key] = $entry.Value
    }
    return $copy
}

function Assert-EnvironmentEquals {
    param(
        [Parameter(Mandatory)] [Collections.IDictionary] $Expected,
        [Parameter(Mandatory)] [string] $Case
    )

    foreach ($entry in $Expected.GetEnumerator()) {
        $actual = [Environment]::GetEnvironmentVariable([string]$entry.Key, 'Process')
        Assert-Contract ($actual -ceq $entry.Value) "$Case did not restore environment variable $($entry.Key)"
    }
}

function Get-Sha256Text {
    param([Parameter(Mandatory)] [string] $Value)

    $bytes = [Text.Encoding]::UTF8.GetBytes($Value)
    try {
        return [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
    }
    finally {
        [Array]::Clear($bytes, 0, $bytes.Length)
    }
}

function Read-StubCalls {
    if (-not (Test-Path -LiteralPath $logPath -PathType Leaf)) {
        return @()
    }

    return @(
        Get-Content -LiteralPath $logPath |
            Where-Object { $_.Length -gt 0 } |
            ForEach-Object { $_ | ConvertFrom-Json -Depth 16 }
    )
}

function Invoke-RunnerCase {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [Collections.IDictionary] $Environment,
        [Parameter(Mandatory)] [bool] $ShouldSucceed,
        [string] $Filter = 'against_mongo',
        [int] $ExpectedCargoCalls = -1,
        [string] $ForbiddenOutput = ''
    )

    Remove-Item -LiteralPath $logPath -Force -ErrorAction SilentlyContinue
    Set-CaseEnvironment $Environment
    $expectedEnvironment = Get-EnvironmentSnapshot -Names $managedEnvironment
    $succeeded = $false
    $output = @()
    try {
        try {
            $output = @(& $runnerPath -Filter $Filter -CargoCommand $cargoStubCommand 2>&1)
            $succeeded = $true
        }
        catch {
            $output = @($_.Exception.Message)
        }

        Assert-Contract ($succeeded -eq $ShouldSucceed) "$Name success state was $succeeded"
        Assert-EnvironmentEquals -Expected $expectedEnvironment -Case $Name

        $joinedOutput = ($output | ForEach-Object { $_.ToString() }) -join "`n"
        if ($ForbiddenOutput.Length -gt 0) {
            Assert-Contract (-not $joinedOutput.Contains($ForbiddenOutput, [StringComparison]::Ordinal)) "$Name leaked the Mongo URI"
        }
        Assert-Contract ($joinedOutput -notmatch '(?i)mongodb(?:\+srv)?://') "$Name emitted a credential-shaped Mongo URI"

        $calls = @(Read-StubCalls)
        if ($ExpectedCargoCalls -ge 0) {
            $callSummary = $calls | ConvertTo-Json -Compress -Depth 8
            Assert-Contract ($calls.Count -eq $ExpectedCargoCalls) "$Name invoked Cargo $($calls.Count) times; expected $ExpectedCargoCalls; runner said: $joinedOutput; calls: $callSummary"
        }
        return $calls
    }
    finally {
        Restore-EnvironmentSnapshot $expectedEnvironment
    }
}

$originalEnvironment = Get-EnvironmentSnapshot -Names $managedEnvironment

try {
    Assert-Contract (Test-Path -LiteralPath $runnerPath -PathType Leaf) 'missing scripts/test-isolated-mongo.ps1'
    Assert-Contract (Test-Path -LiteralPath $rustPath -PathType Leaf) 'missing Mongo persistence source'
    Assert-Contract (Test-Path -LiteralPath $rustTestSupportPath -PathType Leaf) 'missing isolated Mongo test support'
    Assert-Contract (Test-Path -LiteralPath $rustDashboardAuditTestPath -PathType Leaf) 'missing dashboard audit Mongo test'

    New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null
    function global:Invoke-DynamoMongoCargoStub {
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$isList = $args -contains '--list'
$ordinaryNames = @(
    'MONGODB_TEST_URI',
    'MONGODB_URI',
    'MONGO_CONNECTION',
    'MONGODB_DATABASE',
    'DYNAMO_PRODUCTION_MONGODB_URI',
    'MONGODB_PRODUCTION_URI'
)
$ordinaryPresent = @($ordinaryNames | Where-Object {
    $null -ne [Environment]::GetEnvironmentVariable($_, 'Process')
})
$isolated = [Environment]::GetEnvironmentVariable('MONGODB_URI_FOR_ISOLATED_TEST', 'Process')
$isolatedHash = if ($null -eq $isolated) {
    $null
}
else {
    [Convert]::ToHexString(
        [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($isolated))
    ).ToLowerInvariant()
}

[ordered]@{
    argv = @($args)
    phase = if ($isList) { 'list' } else { 'run' }
    isolated_present = $null -ne $isolated
    isolated_sha256 = $isolatedHash
    ordinary_present = @($ordinaryPresent)
} | ConvertTo-Json -Compress -Depth 8 | Add-Content -LiteralPath $env:DYNAMO_STUB_LOG -Encoding utf8

if ($ordinaryPresent.Count -gt 0 -or $null -eq $isolated) {
    $global:LASTEXITCODE = 86
    return
}

if ($isList) {
    if ($env:DYNAMO_STUB_SCENARIO -ceq 'cargo-list-failure') {
        Write-Error 'synthetic list failure'
        $global:LASTEXITCODE = 42
        return
    }
    if ($env:DYNAMO_STUB_SCENARIO -ceq 'zero-selection') {
        '0 tests, 0 benchmarks'
        $global:LASTEXITCODE = 0
        return
    }

    'tests::settings_round_trip_against_mongo: test'
    $global:LASTEXITCODE = 0
    return
}

if ($env:DYNAMO_STUB_SCENARIO -in @('test-error', 'test-panic')) {
    Write-Error ("synthetic failure contained {0}" -f $isolated)
    $global:LASTEXITCODE = 101
    return
}

$global:LASTEXITCODE = 0
    }

    $secretUri = 'mongodb://contract-user:contract-password@example.invalid:27017/?authSource=admin'
    $expectedHash = Get-Sha256Text $secretUri
    $baseEnvironment = [ordered]@{
        DYNAMO_MONGO_TEST = '1'
        MONGODB_TEST_URI = $secretUri
        MONGODB_URI_FOR_ISOLATED_TEST = 'sentinel-special-value'
        MONGODB_URI = 'mongodb://production-one.invalid:27017'
        MONGO_CONNECTION = 'mongodb://production-two.invalid:27017'
        MONGODB_DATABASE = 'production-database'
        DYNAMO_PRODUCTION_MONGODB_URI = 'mongodb://production-three.invalid:27017'
        MONGODB_PRODUCTION_URI = 'mongodb://production-four.invalid:27017'
        DYNAMO_STUB_SCENARIO = 'success'
        DYNAMO_STUB_LOG = $logPath
    }

    [void](Invoke-RunnerCase -Name 'opt-in is required' -Environment ([ordered]@{
        DYNAMO_MONGO_TEST = '0'
        MONGODB_TEST_URI = $secretUri
        DYNAMO_STUB_LOG = $logPath
    }) -ShouldSucceed $false -ExpectedCargoCalls 0 -ForbiddenOutput $secretUri)

    [void](Invoke-RunnerCase -Name 'test URI is required' -Environment ([ordered]@{
        DYNAMO_MONGO_TEST = '1'
        DYNAMO_STUB_LOG = $logPath
    }) -ShouldSucceed $false -ExpectedCargoCalls 0)

    [void](Invoke-RunnerCase -Name 'production URI equality is rejected' -Environment ([ordered]@{
        DYNAMO_MONGO_TEST = '1'
        MONGODB_TEST_URI = $secretUri
        MONGODB_URI = $secretUri
        DYNAMO_STUB_LOG = $logPath
    }) -ShouldSucceed $false -ExpectedCargoCalls 0 -ForbiddenOutput $secretUri)

    [void](Invoke-RunnerCase -Name 'credentials database and query do not change seed identity' -Environment ([ordered]@{
        DYNAMO_MONGO_TEST = '1'
        MONGODB_TEST_URI = 'mongodb://test-user:test-password@shared.example.invalid/test-db?retryWrites=false'
        MONGODB_URI = 'mongodb://prod-user:prod-password@shared.example.invalid:27017/prod-db?replicaSet=production'
        DYNAMO_STUB_LOG = $logPath
    }) -ShouldSucceed $false -ExpectedCargoCalls 0)

    [void](Invoke-RunnerCase -Name 'host case trailing dot and default port normalize' -Environment ([ordered]@{
        DYNAMO_MONGO_TEST = '1'
        MONGODB_TEST_URI = 'mongodb://SHARED.EXAMPLE.INVALID.:27017/test'
        MONGO_CONNECTION = 'mongodb://shared.example.invalid/production'
        DYNAMO_STUB_LOG = $logPath
    }) -ShouldSucceed $false -ExpectedCargoCalls 0)

    [void](Invoke-RunnerCase -Name 'overlapping multi-seed clusters are rejected' -Environment ([ordered]@{
        DYNAMO_MONGO_TEST = '1'
        MONGODB_TEST_URI = 'mongodb://test-a.example.invalid,overlap.example.invalid:27017/test'
        MONGODB_URI = 'mongodb://prod-a.example.invalid:27017,OVERLAP.EXAMPLE.INVALID./production'
        DYNAMO_STUB_LOG = $logPath
    }) -ShouldSucceed $false -ExpectedCargoCalls 0)

    [void](Invoke-RunnerCase -Name 'bracketed IPv6 seeds normalize' -Environment ([ordered]@{
        DYNAMO_MONGO_TEST = '1'
        MONGODB_TEST_URI = 'mongodb://[2001:0db8:0:0:0:0:0:1]/test'
        MONGODB_URI = 'mongodb://[2001:db8::1]:27017/production'
        DYNAMO_STUB_LOG = $logPath
    }) -ShouldSucceed $false -ExpectedCargoCalls 0)

    [void](Invoke-RunnerCase -Name 'SRV cluster identity normalizes' -Environment ([ordered]@{
        DYNAMO_MONGO_TEST = '1'
        MONGODB_TEST_URI = 'mongodb+srv://test-user:test-password@SRV.EXAMPLE.INVALID./test?retryWrites=true'
        MONGODB_URI = 'mongodb+srv://prod-user:prod-password@srv.example.invalid/production?tls=true'
        DYNAMO_STUB_LOG = $logPath
    }) -ShouldSucceed $false -ExpectedCargoCalls 0)

    [void](Invoke-RunnerCase -Name 'malformed test URI fails closed' -Environment ([ordered]@{
        DYNAMO_MONGO_TEST = '1'
        MONGODB_TEST_URI = 'mongodb://[2001:db8::1/test'
        DYNAMO_STUB_LOG = $logPath
    }) -ShouldSucceed $false -ExpectedCargoCalls 0)

    [void](Invoke-RunnerCase -Name 'malformed production URI fails closed' -Environment ([ordered]@{
        DYNAMO_MONGO_TEST = '1'
        MONGODB_TEST_URI = $secretUri
        MONGODB_URI = 'mongodb://production.example.invalid:not-a-port/production'
        DYNAMO_STUB_LOG = $logPath
    }) -ShouldSucceed $false -ExpectedCargoCalls 0 -ForbiddenOutput $secretUri)

    [void](Invoke-RunnerCase -Name 'unsafe filter is rejected' -Environment $baseEnvironment -ShouldSucceed $false -Filter 'against_mongo -- --nocapture' -ExpectedCargoCalls 0 -ForbiddenOutput $secretUri)
    [void](Invoke-RunnerCase -Name 'leading dash filter is rejected' -Environment $baseEnvironment -ShouldSucceed $false -Filter '-against_mongo' -ExpectedCargoCalls 0 -ForbiddenOutput $secretUri)
    [void](Invoke-RunnerCase -Name '--all-targets cannot become a Cargo option' -Environment $baseEnvironment -ShouldSucceed $false -Filter '--all-targets' -ExpectedCargoCalls 0 -ForbiddenOutput $secretUri)

    [void](Invoke-RunnerCase -Name 'zero selection is rejected' -Environment (Copy-CaseEnvironment $baseEnvironment @{ DYNAMO_STUB_SCENARIO = 'zero-selection' }) -ShouldSucceed $false -ExpectedCargoCalls 1 -ForbiddenOutput $secretUri)
    [void](Invoke-RunnerCase -Name 'Cargo list failure is rejected' -Environment (Copy-CaseEnvironment $baseEnvironment @{ DYNAMO_STUB_SCENARIO = 'cargo-list-failure' }) -ShouldSucceed $false -ExpectedCargoCalls 1 -ForbiddenOutput $secretUri)

    foreach ($scenario in @('test-error', 'test-panic')) {
        $calls = @(Invoke-RunnerCase -Name $scenario -Environment (Copy-CaseEnvironment $baseEnvironment @{ DYNAMO_STUB_SCENARIO = $scenario }) -ShouldSucceed $false -ExpectedCargoCalls 2 -ForbiddenOutput $secretUri)
        Assert-Contract (@($calls | Where-Object phase -CEQ 'run').Count -eq 1) "$scenario did not reach the isolated test run"
    }

    $successCalls = @(Invoke-RunnerCase -Name 'distinct endpoint succeeds' -Environment $baseEnvironment -ShouldSucceed $true -ExpectedCargoCalls 2 -ForbiddenOutput $secretUri)
    Assert-Contract ($successCalls.Count -eq 2) 'success must list once and run once'
    foreach ($call in $successCalls) {
        Assert-Contract ($call.isolated_present -eq $true) "$($call.phase) did not receive the isolated URI variable"
        Assert-Contract ($call.isolated_sha256 -ceq $expectedHash) "$($call.phase) received the wrong isolated URI"
        Assert-Contract (@($call.ordinary_present).Count -eq 0) "$($call.phase) received an ordinary Mongo environment variable"
        Assert-Contract (@($call.argv | Where-Object { $_ -ceq 'against_mongo' }).Count -eq 1) "$($call.phase) did not receive the filter as one argv element"
        Assert-Contract (@($call.argv | Where-Object { $_ -ceq '--ignored' }).Count -eq 1) "$($call.phase) did not explicitly select ignored tests"
    }
    $listCall = @($successCalls | Where-Object phase -CEQ 'list')[0]
    $runCall = @($successCalls | Where-Object phase -CEQ 'run')[0]
    Assert-Contract (@($listCall.argv | Where-Object { $_ -ceq '--list' }).Count -eq 1) 'selection did not use --list'
    Assert-Contract (@($runCall.argv | Where-Object { $_ -ceq '--test-threads=1' }).Count -eq 1) 'execution was not serial'
    Assert-Contract (@($runCall.argv | Where-Object { $_ -ceq '--list' }).Count -eq 0) 'execution retained --list'

    [void](Invoke-RunnerCase -Name 'same host with distinct direct port succeeds' -Environment ([ordered]@{
        DYNAMO_MONGO_TEST = '1'
        MONGODB_TEST_URI = 'mongodb://shared.example.invalid:27018/test'
        MONGODB_URI = 'mongodb://shared.example.invalid:27017/production'
        DYNAMO_STUB_SCENARIO = 'success'
        DYNAMO_STUB_LOG = $logPath
    }) -ShouldSucceed $true -ExpectedCargoCalls 2)

    [void](Invoke-RunnerCase -Name 'success restores absent variables' -Environment ([ordered]@{
        DYNAMO_MONGO_TEST = '1'
        MONGODB_TEST_URI = $secretUri
        DYNAMO_STUB_SCENARIO = 'success'
        DYNAMO_STUB_LOG = $logPath
    }) -ShouldSucceed $true -ExpectedCargoCalls 2 -ForbiddenOutput $secretUri)

    $rust = Get-Content -LiteralPath $rustPath -Raw
    $testModuleStart = $rust.IndexOf('#[cfg(test)]', [StringComparison]::Ordinal)
    Assert-Contract ($testModuleStart -ge 0) 'missing Mongo persistence test module'
    $testModule = $rust.Substring($testModuleStart) + [Environment]::NewLine + (Get-Content -LiteralPath $rustTestSupportPath -Raw) + [Environment]::NewLine + (Get-Content -LiteralPath $rustDashboardAuditTestPath -Raw)
    Assert-Contract ($testModule -cnotmatch 'dotenvy::dotenv|MongoPersistenceConfig::try_from_env') 'ignored tests still load dotenv or general Mongo environment'
    Assert-Contract ($testModule.Contains('MONGODB_URI_FOR_ISOLATED_TEST', [StringComparison]::Ordinal)) 'Rust tests do not consume the runner-only URI variable'
    Assert-Contract ($testModule.Contains('catch_unwind', [StringComparison]::Ordinal)) 'panic is not captured before cleanup'
    Assert-Contract ($testModule -cmatch '\.drop\(\)\s*\.await') 'isolated database is not dropped'
    Assert-Contract ($testModule -cmatch 'list_database_names\(\)\s*\.await') 'database absence is not confirmed after cleanup'
    Assert-Contract ($testModule.Contains('"dynmongo_{}_{}"', [StringComparison]::Ordinal)) 'database name does not use the fixed dynmongo_<pid>_<24 hex> shape'
    Assert-Contract ($testModule.Contains('cleanup_failure_outranks_test_error', [StringComparison]::Ordinal)) 'cleanup-vs-error precedence is not tested'
    Assert-Contract ($testModule.Contains('cleanup_failure_outranks_test_panic', [StringComparison]::Ordinal)) 'cleanup-vs-panic precedence is not tested'
    Assert-Contract ($testModule.Contains('successful_cleanup_preserves_test_error', [StringComparison]::Ordinal)) 'test error preservation is not tested'
    Assert-Contract ($testModule.Contains('successful_cleanup_resumes_test_panic', [StringComparison]::Ordinal)) 'panic resumption is not tested'
    foreach ($outcome in @('success', 'error', 'panic')) {
        Assert-Contract ($testModule.Contains("cleanup_runs_once_after_test_$outcome", [StringComparison]::Ordinal)) "cleanup execution after test $outcome is not stub-tested"
    }
    Assert-Contract ($testModule.Contains('cleanup_runs_once_after_synchronous_test_factory_panic', [StringComparison]::Ordinal)) 'synchronous test factory panic cleanup is not tested'
    Assert-Contract ($testModule -cnotmatch 'isolated_mongo_test_config|require_mongo_test_config') 'legacy caller-supplied database configuration remains'
    Assert-Contract (([regex]::Matches($testModule, '#\[ignore\s*=')).Count -eq 2) 'unexpected ignored Mongo test count'
    Assert-Contract (([regex]::Matches($testModule, 'run_isolated_mongo_test\((?:settings|dashboard)')).Count -eq 2) 'every ignored Mongo test must use exactly one isolation guard'
    Assert-Contract ($testModule -cnotmatch 'MongoPersistence::connect') 'ignored tests can still supply a connection string or database name directly'

    'isolated Mongo contract passed'
}
finally {
    Restore-EnvironmentSnapshot $originalEnvironment
    Remove-Item -LiteralPath 'Function:\Invoke-DynamoMongoCargoStub' -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
