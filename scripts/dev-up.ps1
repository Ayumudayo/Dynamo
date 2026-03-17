param(
  [switch]$SkipBootstrap,
  [switch]$SkipBuild,
  [switch]$EnableGiveaway,
  [switch]$Headless,
  [switch]$DryRun
)

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$EnvPath = Join-Path $RepoRoot ".env"
$LogsDir = Join-Path $RepoRoot "logs"

if (-not (Test-Path $EnvPath)) {
  throw "Missing .env at $EnvPath. Copy .env.example first."
}

$null = New-Item -ItemType Directory -Force -Path $LogsDir

$Cargo = Get-Command cargo -ErrorAction Stop
$Shell = Get-Command pwsh -ErrorAction SilentlyContinue
if (-not $Shell) {
  $Shell = Get-Command powershell -ErrorAction Stop
}

$EnvOverrides = @{}

function Get-DotenvValue {
  param([string]$Key)

  if (-not (Test-Path $EnvPath)) {
    return $null
  }

  $Pattern = "^\s*$([regex]::Escape($Key))\s*=\s*(.*)$"
  foreach ($Line in Get-Content $EnvPath) {
    if ($Line -match '^\s*#') {
      continue
    }
    if ($Line -match $Pattern) {
      return $Matches[1].Trim().Trim('"').Trim("'")
    }
  }

  return $null
}

function ConvertTo-BoolSetting {
  param(
    [string]$Key,
    [string]$Value
  )

  switch ($Value.Trim().ToLowerInvariant()) {
    "1" { return $true }
    "true" { return $true }
    "yes" { return $true }
    "on" { return $true }
    "0" { return $false }
    "false" { return $false }
    "no" { return $false }
    "off" { return $false }
    default { throw "$Key in .env must be one of true/false/1/0/yes/no/on/off." }
  }
}

function Resolve-BoolSetting {
  param(
    [string]$Key,
    [bool]$Default,
    [bool]$CliEnable
  )

  if ($CliEnable) {
    return $true
  }

  $RawValue = Get-DotenvValue -Key $Key
  if ($null -eq $RawValue -or $RawValue -eq "") {
    return $Default
  }

  return ConvertTo-BoolSetting -Key $Key -Value $RawValue
}

function Get-DashboardPublicBaseUrl {
  $BaseUrl = Get-DotenvValue -Key "DASHBOARD_BASE_URL"
  if ($BaseUrl) {
    return $BaseUrl.TrimEnd('/')
  }

  $DashboardHost = Get-DotenvValue -Key "DASHBOARD_HOST"
  if (-not $DashboardHost) {
    $DashboardHost = "127.0.0.1"
  }
  $DashboardPort = Get-DotenvValue -Key "DASHBOARD_PORT"
  if (-not $DashboardPort) {
    $DashboardPort = "3000"
  }

  return "http://$DashboardHost`:$DashboardPort"
}

function Get-DashboardHealthUrl {
  $DashboardHost = Get-DotenvValue -Key "DASHBOARD_HOST"
  if (-not $DashboardHost) {
    $DashboardHost = "127.0.0.1"
  }
  if ($DashboardHost -eq "0.0.0.0" -or $DashboardHost -eq "::") {
    $DashboardHost = "127.0.0.1"
  }
  $DashboardPort = Get-DotenvValue -Key "DASHBOARD_PORT"
  if (-not $DashboardPort) {
    $DashboardPort = "3000"
  }

  return "http://$DashboardHost`:$DashboardPort/healthz"
}

function Join-CommandParts {
  param([string[]]$Parts)
  return ($Parts -join "; ")
}

function Get-BinaryPath {
  param([string]$Crate)
  return (Join-Path $RepoRoot "target\debug\$Crate.exe")
}

function Get-ManagedProcessIds {
  param([string]$Crate)

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
    Write-Host "No managed pid file for $Name. Scanning for lingering processes..."
  } else {
    $RawPid = Get-Content $PidPath -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $RawPid) {
      Remove-Item $PidPath -Force -ErrorAction SilentlyContinue
    } else {
      $ManagedPid = 0
      if (-not [int]::TryParse($RawPid, [ref]$ManagedPid)) {
        Remove-Item $PidPath -Force -ErrorAction SilentlyContinue
      } else {
        $Process = Get-Process -Id $ManagedPid -ErrorAction SilentlyContinue
        if ($Process) {
          Write-Host "Stopping existing $Name wrapper (pid=$ManagedPid)..."
          Stop-Process -Id $ManagedPid -Force
          $StoppedAny = $true
          Start-Sleep -Seconds 1
        }
      }
    }
  }

  Remove-Item $PidPath -Force -ErrorAction SilentlyContinue

  $LingeringPids = Get-ManagedProcessIds -Crate $Crate
  foreach ($ProcessId in $LingeringPids) {
    $Process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($Process) {
      Write-Host "Stopping lingering $Name process (pid=$ProcessId)..."
      Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue
      $StoppedAny = $true
    }
  }

  if ($StoppedAny) {
    Start-Sleep -Seconds 1
  }
}

function Assert-BinaryExists {
  param([string]$Crate)

  $BinaryPath = Get-BinaryPath -Crate $Crate
  if (-not (Test-Path $BinaryPath)) {
    throw "Missing built binary at $BinaryPath. Run without -SkipBuild first."
  }
}

function Invoke-Build {
  $Commands = @()
  foreach ($Item in $EnvOverrides.GetEnumerator()) {
    $Commands += "`$env:$($Item.Key)='$($Item.Value)'"
  }
  $Commands += "Set-Location '$RepoRoot'"
  $Commands += "cargo build -p dynamo-bootstrap -p dynamo-dashboard -p dynamo-bot"
  $Command = Join-CommandParts -Parts $Commands

  if ($DryRun) {
    Write-Host "[dry-run] $($Shell.Source) -NoLogo -NoProfile -Command $Command"
    return [pscustomobject]@{
      Status = "dry-run"
      ArtifactsPath = (Join-Path $RepoRoot "target\debug")
    }
  }

  & $Shell.Source -NoLogo -NoProfile -Command $Command
  return [pscustomobject]@{
    Status = "ok"
    ArtifactsPath = (Join-Path $RepoRoot "target\debug")
  }
}

function Start-RustProcess {
  param(
    [string]$Name,
    [string]$Crate
  )

  $ConsoleLogPath = Join-Path $LogsDir "$Name.console.log"
  $PidPath = Join-Path $LogsDir "$Name.pid"
  $BinaryPath = Get-BinaryPath -Crate $Crate
  if (-not $DryRun) {
    Assert-BinaryExists -Crate $Crate
  }

  $Commands = @()
  foreach ($Item in $EnvOverrides.GetEnumerator()) {
    $Commands += "`$env:$($Item.Key)='$($Item.Value)'"
  }
  $Commands += "Set-Location '$RepoRoot'"
  if ($Headless) {
    $Commands += "& '$BinaryPath'"
  } else {
    $Commands += "Write-Host '[launcher] starting $Name from $BinaryPath'"
    $Commands += "& '$BinaryPath' 2>&1 | Tee-Object -FilePath '$ConsoleLogPath'"
  }
  $Command = Join-CommandParts -Parts $Commands

  if ($DryRun) {
    Write-Host "[dry-run] $($Shell.Source) -NoLogo -NoProfile $($(if ($Headless) { '' } else { '-NoExit ' })) -Command $Command"
    return [pscustomobject]@{
      Name = $Name
      Crate = $Crate
      BinaryPath = $BinaryPath
      Pid = "-"
      Running = $true
      LogPath = $ConsoleLogPath
      StdoutPath = $null
      StderrPath = $null
      Status = "dry-run"
    }
  }

  Stop-ManagedProcess -Name $Name -Crate $Crate

  $ArgumentList = @("-NoLogo", "-NoProfile")
  if (-not $Headless) {
    $ArgumentList += "-NoExit"
  }
  $ArgumentList += @("-Command", $Command)

  if ($Headless) {
    $StdoutPath = Join-Path $LogsDir "$Name.stdout.log"
    $StderrPath = Join-Path $LogsDir "$Name.stderr.log"
    $Process = Start-Process `
      -FilePath $Shell.Source `
      -ArgumentList $ArgumentList `
      -WorkingDirectory $RepoRoot `
      -RedirectStandardOutput $StdoutPath `
      -RedirectStandardError $StderrPath `
      -WindowStyle Hidden `
      -PassThru
  } else {
    $Process = Start-Process `
      -FilePath $Shell.Source `
      -ArgumentList $ArgumentList `
      -WorkingDirectory $RepoRoot `
      -PassThru
  }

  Set-Content -Path $PidPath -Value $Process.Id
  Write-Host "$Name started (pid=$($Process.Id))"
  if ($Headless) {
    Write-Host "  stdout: $StdoutPath"
    Write-Host "  stderr: $StderrPath"
  } else {
    Write-Host "  console: $ConsoleLogPath"
  }
  Write-Host "  pid:    $PidPath"

  Start-Sleep -Seconds 2
  $Running = Get-Process -Id $Process.Id -ErrorAction SilentlyContinue
  if (-not $Running) {
    Write-Warning "$Name exited immediately."
    if ($Headless -and (Test-Path $StdoutPath)) {
      Write-Host "---- $Name stdout ----"
      Get-Content $StdoutPath -Tail 40
    }
    if ($Headless -and (Test-Path $StderrPath)) {
      Write-Host "---- $Name stderr ----"
      Get-Content $StderrPath -Tail 40
      $ErrorText = (Get-Content $StderrPath -Raw)
      if ($ErrorText -match "Disallowed gateway intents") {
        Write-Warning "Discord bot intents are not enabled in the developer portal. Enable the required privileged intents, especially Server Members Intent."
      }
    }
  }

  return [pscustomobject]@{
    Name = $Name
    Crate = $Crate
    BinaryPath = $BinaryPath
    Pid = $Process.Id
    Running = [bool]$Running
    LogPath = $(if ($Headless) { $StdoutPath } else { $ConsoleLogPath })
    StdoutPath = $(if ($Headless) { $StdoutPath } else { $null })
    StderrPath = $(if ($Headless) { $StderrPath } else { $null })
    Status = $(if ($Running) { "ok" } else { "degraded" })
  }
}

function Invoke-Bootstrap {
  $BinaryPath = Get-BinaryPath -Crate "dynamo-bootstrap"
  if (-not $DryRun) {
    Assert-BinaryExists -Crate "dynamo-bootstrap"
  }

  $Commands = @()
  foreach ($Item in $EnvOverrides.GetEnumerator()) {
    $Commands += "`$env:$($Item.Key)='$($Item.Value)'"
  }
  $Commands += "Set-Location '$RepoRoot'"
  $Commands += "& '$BinaryPath'"
  $Command = Join-CommandParts -Parts $Commands

  if ($DryRun) {
    Write-Host "[dry-run] $($Shell.Source) -NoLogo -NoProfile -Command $Command"
    return [pscustomobject]@{ Status = "dry-run" }
  }

  & $Shell.Source -NoLogo -NoProfile -Command $Command
  return [pscustomobject]@{ Status = "ok" }
}

function Test-DashboardHealth {
  param([string]$HealthUrl)

  if ($DryRun) {
    return [pscustomobject]@{
      Status = "dry-run"
      Url = $HealthUrl
      Healthy = $true
    }
  }

  try {
    $Response = Invoke-WebRequest -Uri $HealthUrl -UseBasicParsing -TimeoutSec 5
    return [pscustomobject]@{
      Status = if ($Response.StatusCode -eq 200) { "ok" } else { "degraded" }
      Url = $HealthUrl
      Healthy = ($Response.StatusCode -eq 200)
    }
  } catch {
    return [pscustomobject]@{
      Status = "degraded"
      Url = $HealthUrl
      Healthy = $false
    }
  }
}

Write-Host "Repo root: $RepoRoot"
Write-Host "Logs dir:  $LogsDir"
Write-Host "Mode:      $($(if ($Headless) { 'headless' } else { 'visible windows' }))"
if ($EnableGiveaway) {
  Write-Warning "-EnableGiveaway is no longer needed. Giveaway is a built-in core module."
}
$DevGuildId = Get-DotenvValue -Key "DISCORD_DEV_GUILD_ID"
if (-not $DevGuildId) {
  $DevGuildId = Get-DotenvValue -Key "GUILD_ID"
}
$RegisterGloballyDefault = (-not $DevGuildId)
$RegisterGlobally = Resolve-BoolSetting -Key "DISCORD_REGISTER_GLOBALLY" -Default $RegisterGloballyDefault -CliEnable $false
$CommandScope = if ($RegisterGlobally) {
  "global"
} elseif ($DevGuildId) {
  "guild ($DevGuildId)"
} else {
  "guild (missing DISCORD_DEV_GUILD_ID/GUILD_ID)"
}
Write-Host "Command scope: $CommandScope"

if (-not $SkipBuild) {
  Write-Host "Prebuilding shared Rust artifacts..."
  $BuildResult = Invoke-Build
} else {
  $BuildResult = [pscustomobject]@{
    Status = "skipped"
    ArtifactsPath = (Join-Path $RepoRoot "target\debug")
  }
}

if (-not $SkipBootstrap) {
  Write-Host "Running Mongo bootstrap..."
  $BootstrapResult = Invoke-Bootstrap
} else {
  $BootstrapResult = [pscustomobject]@{ Status = "skipped" }
}

Write-Host "Starting dashboard..."
$DashboardResult = Start-RustProcess -Name "dashboard" -Crate "dynamo-dashboard"

Write-Host "Starting bot..."
$BotResult = Start-RustProcess -Name "bot" -Crate "dynamo-bot"

$DashboardPublicBaseUrl = Get-DashboardPublicBaseUrl
$DashboardHealth = Test-DashboardHealth -HealthUrl (Get-DashboardHealthUrl)
$OverallStatus = if ($DashboardResult.Status -eq "degraded" -or $BotResult.Status -eq "degraded" -or $DashboardHealth.Status -eq "degraded") {
  "degraded"
} else {
  "ok"
}

Write-Host ""
Write-Host "Startup summary:"
Write-Host "  artifacts: $($BuildResult.ArtifactsPath) [$($BuildResult.Status)]"
Write-Host "  bootstrap: $($BootstrapResult.Status)"
Write-Host "  dashboard: pid=$($DashboardResult.Pid) url=$DashboardPublicBaseUrl health=$($DashboardHealth.Status) log=$($DashboardResult.LogPath)"
Write-Host "  bot: pid=$($BotResult.Pid) scope=$CommandScope log=$($BotResult.LogPath)"
Write-Host "  overall: $OverallStatus"

Write-Host "Done."
