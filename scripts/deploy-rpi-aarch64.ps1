param(
  [string]$RemoteHost = "",
  [string]$RemoteUser = "",
  [int]$Port = 22,
  [string]$AppDir = "",
  [switch]$SkipBuild,
  [switch]$SkipBootstrap
)

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$StageDir = Join-Path $RepoRoot "output\rpi-aarch64"

if (-not $RemoteHost) { $RemoteHost = $env:RPI_HOST }
if (-not $RemoteUser) { $RemoteUser = $env:RPI_USER }
if (-not $AppDir) { $AppDir = $env:RPI_APP_DIR }
if (-not $AppDir -and $RemoteUser) { $AppDir = "/home/$RemoteUser/dynamo" }

if (-not $RemoteHost -or -not $RemoteUser) {
  throw "RemoteHost and RemoteUser are required. Use -RemoteHost/-RemoteUser or set RPI_HOST and RPI_USER."
}

if (-not $SkipBuild) {
  & (Join-Path $RepoRoot "scripts\build-rpi-aarch64.ps1")
  if ($LASTEXITCODE -ne 0) {
    throw "Raspberry Pi cross-build failed."
  }
}

if (-not (Test-Path $StageDir)) {
  throw "Missing staged bundle at $StageDir. Run build-rpi-aarch64 first."
}

$Target = "$RemoteUser@$RemoteHost"

& ssh -p $Port $Target "mkdir -p '$AppDir' '$AppDir/scripts' '$AppDir/target/release' '$AppDir/logs'"
if ($LASTEXITCODE -ne 0) {
  throw "Failed to prepare remote directories."
}

Push-Location $RepoRoot
try {
  & scp -P $Port "output/rpi-aarch64/ecosystem.pm2.cjs" "output/rpi-aarch64/.env.example" "${Target}:${AppDir}/"
  if ($LASTEXITCODE -ne 0) { throw "Failed to copy root deployment assets." }

  & scp -P $Port "output/rpi-aarch64/scripts/prod-bootstrap.sh" "output/rpi-aarch64/scripts/prod-dashboard.sh" "output/rpi-aarch64/scripts/prod-bot.sh" "${Target}:${AppDir}/scripts/"
  if ($LASTEXITCODE -ne 0) { throw "Failed to copy runtime scripts." }

  & scp -P $Port "output/rpi-aarch64/target/release/dynamo-bootstrap" "output/rpi-aarch64/target/release/dynamo-dashboard" "output/rpi-aarch64/target/release/dynamo-bot" "${Target}:${AppDir}/target/release/"
  if ($LASTEXITCODE -ne 0) { throw "Failed to copy release binaries." }
}
finally {
  Pop-Location
}

$RunBootstrap = if ($SkipBootstrap) { "false" } else { "true" }
$RemoteScript = @'
set -euo pipefail
APP_DIR="$1"
RUN_BOOTSTRAP="$2"

chmod +x "$APP_DIR"/scripts/*.sh

if [[ ! -f "$APP_DIR/.env" ]]; then
  cp "$APP_DIR/.env.example" "$APP_DIR/.env"
  echo "Created $APP_DIR/.env from .env.example. Fill it with real values and rerun deployment." >&2
  exit 1
fi

cd "$APP_DIR"

if [[ "$RUN_BOOTSTRAP" == "true" ]]; then
  ./scripts/prod-bootstrap.sh
fi

if ! command -v pm2 >/dev/null 2>&1; then
  echo "pm2 is not installed on the target host." >&2
  exit 1
fi

pm2 startOrRestart ecosystem.pm2.cjs --update-env
pm2 save
'@

$RemoteScript | & ssh -p $Port $Target "bash -s -- '$AppDir' '$RunBootstrap'"
if ($LASTEXITCODE -ne 0) {
  throw "Remote bootstrap/start sequence failed."
}

Write-Host "Deployed bundle to ${Target}:$AppDir"
