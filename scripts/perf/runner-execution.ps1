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

