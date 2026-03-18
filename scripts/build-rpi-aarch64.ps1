param()

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$TargetTriple = "aarch64-unknown-linux-gnu"
$StageDir = Join-Path $RepoRoot "output\rpi-aarch64"
$ReleaseDir = Join-Path $RepoRoot "target\$TargetTriple\release"

function Require-Command {
  param([string]$Name)
  if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
    throw "Missing required command: $Name"
  }
}

function Invoke-NativeQuiet {
  param(
    [string]$Command,
    [string[]]$Arguments = @()
  )

  $stdoutPath = Join-Path $env:TEMP ("dynamo-native-" + [guid]::NewGuid().ToString() + ".stdout.log")
  $stderrPath = Join-Path $env:TEMP ("dynamo-native-" + [guid]::NewGuid().ToString() + ".stderr.log")
  try {
    $process = Start-Process -FilePath $Command `
      -ArgumentList $Arguments `
      -NoNewWindow `
      -Wait `
      -PassThru `
      -RedirectStandardOutput $stdoutPath `
      -RedirectStandardError $stderrPath
    return $process.ExitCode
  }
  finally {
    if (Test-Path $stdoutPath) {
      Remove-Item $stdoutPath -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path $stderrPath) {
      Remove-Item $stderrPath -Force -ErrorAction SilentlyContinue
    }
  }
}

Require-Command rustup
Require-Command cargo
Require-Command zig
Require-Command cargo-zigbuild

if ((Invoke-NativeQuiet "cargo-zigbuild" @("--version")) -ne 0) {
  throw "cargo-zigbuild is not installed. Run: cargo install cargo-zigbuild"
}

if ((Invoke-NativeQuiet "rustup" @("target", "add", $TargetTriple)) -ne 0) {
  throw "Failed to ensure Rust target $TargetTriple is installed."
}

Push-Location $RepoRoot
try {
  & cargo zigbuild --release --target $TargetTriple -p dynamo-bootstrap -p dynamo-dashboard -p dynamo-bot
  if ($LASTEXITCODE -ne 0) {
    throw "cargo zigbuild failed."
  }
}
finally {
  Pop-Location
}

if (Test-Path $StageDir) {
  Remove-Item $StageDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "target\release") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "scripts") | Out-Null

Copy-Item (Join-Path $RepoRoot "ecosystem.config.js") (Join-Path $StageDir "ecosystem.config.js") -Force
Copy-Item (Join-Path $RepoRoot ".env.example") (Join-Path $StageDir ".env.example") -Force
Copy-Item (Join-Path $RepoRoot "scripts\prod-bootstrap.sh") (Join-Path $StageDir "scripts\prod-bootstrap.sh") -Force
Copy-Item (Join-Path $RepoRoot "scripts\prod-dashboard.sh") (Join-Path $StageDir "scripts\prod-dashboard.sh") -Force
Copy-Item (Join-Path $RepoRoot "scripts\prod-bot.sh") (Join-Path $StageDir "scripts\prod-bot.sh") -Force
Copy-Item (Join-Path $RepoRoot "scripts\remote-rpi-postdeploy.sh") (Join-Path $StageDir "scripts\remote-rpi-postdeploy.sh") -Force
Copy-Item (Join-Path $ReleaseDir "dynamo-bootstrap") (Join-Path $StageDir "target\release\dynamo-bootstrap") -Force
Copy-Item (Join-Path $ReleaseDir "dynamo-dashboard") (Join-Path $StageDir "target\release\dynamo-dashboard") -Force
Copy-Item (Join-Path $ReleaseDir "dynamo-bot") (Join-Path $StageDir "target\release\dynamo-bot") -Force

Write-Host "Staged Raspberry Pi deployment bundle at $StageDir"
