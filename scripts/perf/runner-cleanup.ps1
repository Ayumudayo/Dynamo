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
