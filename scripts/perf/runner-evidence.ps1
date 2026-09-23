# Runner evidence, source snapshot, and sanitized diagnostics.
# Dot-sourced by with-isolated-dashboard.ps1 so runner failure/path contracts remain in the caller scope.

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
