param(
  [switch]$SkipBootstrap,
  [switch]$EnableGiveaway,
  [switch]$EnableMusic,
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
if ($EnableMusic) {
  $EnvOverrides["DYNAMO_ENABLE_MUSIC"] = "true"
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

  $StdoutPath = Join-Path $LogsDir "$Name.stdout.log"
  $StderrPath = Join-Path $LogsDir "$Name.stderr.log"
  $PidPath = Join-Path $LogsDir "$Name.pid"

  $Commands = @()
  foreach ($Item in $EnvOverrides.GetEnumerator()) {
    $Commands += "`$env:$($Item.Key)='$($Item.Value)'"
  }
  $Commands += "Set-Location '$RepoRoot'"
  $Commands += "cargo run -p $Crate"
  $Command = Join-CommandParts -Parts $Commands

  if ($DryRun) {
    Write-Host "[dry-run] $($Shell.Source) -NoLogo -NoProfile -Command $Command"
    return
  }

  $Process = Start-Process `
    -FilePath $Shell.Source `
    -ArgumentList @("-NoLogo", "-NoProfile", "-Command", $Command) `
    -WorkingDirectory $RepoRoot `
    -RedirectStandardOutput $StdoutPath `
    -RedirectStandardError $StderrPath `
    -WindowStyle Hidden `
    -PassThru

  Set-Content -Path $PidPath -Value $Process.Id
  Write-Host "$Name started (pid=$($Process.Id))"
  Write-Host "  stdout: $StdoutPath"
  Write-Host "  stderr: $StderrPath"
  Write-Host "  pid:    $PidPath"

  Start-Sleep -Seconds 2
  $Running = Get-Process -Id $Process.Id -ErrorAction SilentlyContinue
  if (-not $Running) {
    Write-Warning "$Name exited immediately."
    if (Test-Path $StdoutPath) {
      Write-Host "---- $Name stdout ----"
      Get-Content $StdoutPath -Tail 40
    }
    if (Test-Path $StderrPath) {
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

if (-not $SkipBootstrap) {
  Write-Host "Running Mongo bootstrap..."
  Invoke-Bootstrap
}

Write-Host "Starting dashboard..."
Start-RustProcess -Name "dashboard" -Crate "dynamo-dashboard"

Write-Host "Starting bot..."
Start-RustProcess -Name "bot" -Crate "dynamo-bot"

Write-Host "Done."
