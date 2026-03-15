param(
  [switch]$SkipBootstrap,
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
if ($EnableGiveaway) {
  $EnvOverrides["DYNAMO_ENABLE_GIVEAWAY"] = "true"
}

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

function Stop-ManagedProcess {
  param([string]$Name)

  $PidPath = Join-Path $LogsDir "$Name.pid"
  if (-not (Test-Path $PidPath)) {
    return
  }

  $RawPid = Get-Content $PidPath -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $RawPid) {
    Remove-Item $PidPath -Force -ErrorAction SilentlyContinue
    return
  }

  $ManagedPid = 0
  if (-not [int]::TryParse($RawPid, [ref]$ManagedPid)) {
    Remove-Item $PidPath -Force -ErrorAction SilentlyContinue
    return
  }

  $Process = Get-Process -Id $ManagedPid -ErrorAction SilentlyContinue
  if ($Process) {
    Write-Host "Stopping existing $Name process (pid=$ManagedPid)..."
    Stop-Process -Id $ManagedPid -Force
    Start-Sleep -Seconds 1
  }

  Remove-Item $PidPath -Force -ErrorAction SilentlyContinue
}

function Join-CommandParts {
  param([string[]]$Parts)
  return ($Parts -join "; ")
}

function Start-RustProcess {
  param(
    [string]$Name,
    [string]$Crate
  )

  Stop-ManagedProcess -Name $Name

  $ConsoleLogPath = Join-Path $LogsDir "$Name.console.log"
  $PidPath = Join-Path $LogsDir "$Name.pid"

  $Commands = @()
  foreach ($Item in $EnvOverrides.GetEnumerator()) {
    $Commands += "`$env:$($Item.Key)='$($Item.Value)'"
  }
  $Commands += "Set-Location '$RepoRoot'"
  if ($Headless) {
    $Commands += "cargo run -p $Crate"
  } else {
    $Commands += "cargo run -p $Crate 2>&1 | Tee-Object -FilePath '$ConsoleLogPath'"
  }
  $Command = Join-CommandParts -Parts $Commands

  if ($DryRun) {
    Write-Host "[dry-run] $($Shell.Source) -NoLogo -NoProfile $($(if ($Headless) { '' } else { '-NoExit ' })) -Command $Command"
    return
  }

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
}

function Invoke-Bootstrap {
  $Commands = @()
  foreach ($Item in $EnvOverrides.GetEnumerator()) {
    $Commands += "`$env:$($Item.Key)='$($Item.Value)'"
  }
  $Commands += "Set-Location '$RepoRoot'"
  $Commands += "cargo run -p dynamo-bootstrap"
  $Command = Join-CommandParts -Parts $Commands

  if ($DryRun) {
    Write-Host "[dry-run] $($Shell.Source) -NoLogo -NoProfile -Command $Command"
    return
  }

  & $Shell.Source -NoLogo -NoProfile -Command $Command
}

Write-Host "Repo root: $RepoRoot"
Write-Host "Logs dir:  $LogsDir"
Write-Host "Mode:      $($(if ($Headless) { 'headless' } else { 'visible windows' }))"
$EffectiveGiveaway = Resolve-BoolSetting -Key "DYNAMO_ENABLE_GIVEAWAY" -Default $false -CliEnable $EnableGiveaway.IsPresent
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
if ($EffectiveGiveaway) {
  Write-Host "Giveaway module override: enabled"
}

if (-not $SkipBootstrap) {
  Write-Host "Running Mongo bootstrap..."
  Invoke-Bootstrap
}

Write-Host "Starting dashboard..."
Start-RustProcess -Name "dashboard" -Crate "dynamo-dashboard"

Write-Host "Starting bot..."
Start-RustProcess -Name "bot" -Crate "dynamo-bot"

Write-Host "Done."
