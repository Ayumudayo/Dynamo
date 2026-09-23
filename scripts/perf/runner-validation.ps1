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
