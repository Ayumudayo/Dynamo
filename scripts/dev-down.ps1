param()

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$LogsDir = Join-Path $RepoRoot "logs"

function Stop-ManagedProcess {
  param([string]$Name)

  $PidPath = Join-Path $LogsDir "$Name.pid"
  if (-not (Test-Path $PidPath)) {
    Write-Host "$Name is not running (no pid file)."
    return
  }

  $RawPid = Get-Content $PidPath -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $RawPid) {
    Remove-Item $PidPath -Force -ErrorAction SilentlyContinue
    Write-Host "$Name pid file was empty and has been removed."
    return
  }

  $ManagedPid = 0
  if (-not [int]::TryParse($RawPid, [ref]$ManagedPid)) {
    Remove-Item $PidPath -Force -ErrorAction SilentlyContinue
    Write-Host "$Name pid file was invalid and has been removed."
    return
  }

  $Process = Get-Process -Id $ManagedPid -ErrorAction SilentlyContinue
  if ($Process) {
    Stop-Process -Id $ManagedPid -Force
    Write-Host "Stopped $Name (pid=$ManagedPid)."
  } else {
    Write-Host "$Name process was already stopped."
  }

  Remove-Item $PidPath -Force -ErrorAction SilentlyContinue
}

Write-Host "Repo root: $RepoRoot"
Write-Host "Logs dir:  $LogsDir"

Stop-ManagedProcess -Name "dashboard"
Stop-ManagedProcess -Name "bot"
