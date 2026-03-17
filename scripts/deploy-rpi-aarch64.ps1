param(
  [string]$RemoteHost = "",
  [string]$RemoteUser = "",
  [int]$Port = 22,
  [string]$AppDir = "",
  [string]$KeyPath = "",
  [switch]$SkipBuild,
  [switch]$SkipBootstrap,
  [switch]$ForceBootstrap
)

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$StageDir = Join-Path $RepoRoot "output\rpi-aarch64"
$ArchivePath = Join-Path $RepoRoot "output\rpi-aarch64.tar"

if (-not $RemoteHost) { $RemoteHost = $env:RPI_HOST }
if (-not $RemoteUser) { $RemoteUser = $env:RPI_USER }
if (-not $AppDir) { $AppDir = $env:RPI_APP_DIR }
if (-not $KeyPath) { $KeyPath = $env:RPI_SSH_KEY }
if (-not $AppDir -and $RemoteUser) { $AppDir = "/home/$RemoteUser/bot/Dynamo-Prebuilt" }

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
$SshArgs = @("-p", "$Port")
$ScpArgs = @("-P", "$Port")
if ($KeyPath) {
  $SshArgs += @("-i", $KeyPath)
  $ScpArgs += @("-i", $KeyPath)
}

if (Test-Path $ArchivePath) {
  Remove-Item $ArchivePath -Force
}

Push-Location $StageDir
try {
  & tar.exe -cf $ArchivePath .
  if ($LASTEXITCODE -ne 0) {
    throw "Failed to create deployment archive."
  }
}
finally {
  Pop-Location
}

Push-Location $RepoRoot
try {
  & scp @ScpArgs "output/rpi-aarch64.tar" "${Target}:${AppDir}.deploy.tar"
  if ($LASTEXITCODE -ne 0) { throw "Failed to copy deployment archive." }

  & scp @ScpArgs "scripts/remote-rpi-postdeploy.sh" "${Target}:${AppDir}.postdeploy.sh"
  if ($LASTEXITCODE -ne 0) { throw "Failed to copy remote postdeploy script." }
}
finally {
  Pop-Location
}

$BootstrapMode = "auto"
if ($SkipBootstrap) {
  $BootstrapMode = "skip"
}
elseif ($ForceBootstrap) {
  $BootstrapMode = "force"
}

& ssh @SshArgs $Target "chmod +x '$AppDir.postdeploy.sh' && bash '$AppDir.postdeploy.sh' '$AppDir' '$BootstrapMode' '${AppDir}.deploy.tar'"
if ($LASTEXITCODE -ne 0) {
  throw "Remote bootstrap/start sequence failed."
}

if (Test-Path $ArchivePath) {
  Remove-Item $ArchivePath -Force
}

Write-Host "Deployed bundle to ${Target}:$AppDir"
