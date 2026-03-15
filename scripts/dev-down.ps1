param()

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$LogsDir = Join-Path $RepoRoot "logs"

function Get-BinaryPath {
  param([string]$Crate)
  return (Join-Path $RepoRoot "target\debug\$Crate.exe")
}

function Get-ManagedProcessIds {
  param(
    [string]$Name,
    [string]$Crate
  )

  $BinaryPath = Get-BinaryPath -Crate $Crate
  $ShellNames = @("pwsh.exe", "powershell.exe")
  $BinaryName = [System.IO.Path]::GetFileName($BinaryPath)

  $Candidates = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
    ($_.Name -ieq $BinaryName -and ($_.ExecutablePath -eq $BinaryPath -or $_.CommandLine -like "*$BinaryPath*")) -or
    ($ShellNames -contains $_.Name -and $_.CommandLine -like "*$BinaryPath*")
  } | Select-Object -ExpandProperty ProcessId -Unique

  return @($Candidates)
}

function Stop-ManagedProcess {
  param(
    [string]$Name,
    [string]$Crate
  )

  $PidPath = Join-Path $LogsDir "$Name.pid"
  $StoppedAny = $false

  if (-not (Test-Path $PidPath)) {
    Write-Host "$Name pid file was not found. Scanning for lingering processes..."
  } else {
    $RawPid = Get-Content $PidPath -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $RawPid) {
      Remove-Item $PidPath -Force -ErrorAction SilentlyContinue
      Write-Host "$Name pid file was empty and has been removed."
    } else {
      $ManagedPid = 0
      if (-not [int]::TryParse($RawPid, [ref]$ManagedPid)) {
        Remove-Item $PidPath -Force -ErrorAction SilentlyContinue
        Write-Host "$Name pid file was invalid and has been removed."
      } else {
        $Process = Get-Process -Id $ManagedPid -ErrorAction SilentlyContinue
        if ($Process) {
          Stop-Process -Id $ManagedPid -Force
          Write-Host "Stopped $Name wrapper (pid=$ManagedPid)."
          $StoppedAny = $true
        } else {
          Write-Host "$Name wrapper process was already stopped."
        }
      }
    }
  }

  Remove-Item $PidPath -Force -ErrorAction SilentlyContinue

  $LingeringPids = Get-ManagedProcessIds -Name $Name -Crate $Crate
  foreach ($Pid in $LingeringPids) {
    $Process = Get-Process -Id $Pid -ErrorAction SilentlyContinue
    if ($Process) {
      Stop-Process -Id $Pid -Force -ErrorAction SilentlyContinue
      Write-Host "Stopped $Name lingering process (pid=$Pid)."
      $StoppedAny = $true
    }
  }

  if (-not $StoppedAny) {
    Write-Host "$Name was not running."
  }
}

Write-Host "Repo root: $RepoRoot"
Write-Host "Logs dir:  $LogsDir"

Stop-ManagedProcess -Name "dashboard" -Crate "dynamo-dashboard"
Stop-ManagedProcess -Name "bot" -Crate "dynamo-bot"
