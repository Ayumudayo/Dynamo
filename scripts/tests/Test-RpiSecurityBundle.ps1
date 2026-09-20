[CmdletBinding(DefaultParameterSetName = "Validate")]
param(
  [Parameter(Mandatory = $true, ParameterSetName = "Validate")]
  [string]$StageDir,

  [Parameter(Mandatory = $true, ParameterSetName = "SelfTest")]
  [switch]$SelfTest
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$SourceHelper = Join-Path $RepoRoot "scripts\lib\secure-env.sh"
$RunningOnWindows = [Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT

function Assert-RegularNonLinkFile {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path,

    [Parameter(Mandatory = $true)]
    [string]$Description
  )

  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "Missing ${Description}: $Path"
  }
  $item = Get-Item -LiteralPath $Path -Force
  if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "$Description must be a regular non-symlink file: $Path"
  }
}

function Assert-ContainsLiteral {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path,

    [Parameter(Mandatory = $true)]
    [string]$Expected,

    [Parameter(Mandatory = $true)]
    [string]$Message
  )

  $content = Get-Content -LiteralPath $Path -Raw
  if ($content.IndexOf($Expected, [StringComparison]::Ordinal) -lt 0) {
    throw $Message
  }
}

function Assert-RpiSecurityBundle {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path
  )

  $resolvedStage = (Resolve-Path -LiteralPath $Path).Path
  $stagedHelper = Join-Path $resolvedStage "scripts\lib\secure-env.sh"
  Assert-RegularNonLinkFile -Path $stagedHelper -Description "exact helper path scripts/lib/secure-env.sh"

  $sourceHash = (Get-FileHash -LiteralPath $SourceHelper -Algorithm SHA256).Hash.ToLowerInvariant()
  $stagedHash = (Get-FileHash -LiteralPath $stagedHelper -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($stagedHash -cne $sourceHash) {
    throw "Staged helper SHA-256 mismatch (expected $sourceHash, found $stagedHash)."
  }

  if (-not $RunningOnWindows) {
    $mode = [IO.File]::GetUnixFileMode($stagedHelper)
    if (($mode -band [IO.UnixFileMode]::UserExecute) -eq 0) {
      throw "Staged helper is not executable: $stagedHelper"
    }
  }

  foreach ($launcherName in @("prod-bootstrap.sh", "prod-dashboard.sh", "prod-bot.sh")) {
    $launcher = Join-Path $resolvedStage "scripts\$launcherName"
    Assert-RegularNonLinkFile -Path $launcher -Description "production entrypoint $launcherName"
    Assert-ContainsLiteral -Path $launcher `
      -Expected 'source "$ROOT_DIR/scripts/lib/secure-env.sh"' `
      -Message "$launcherName does not source scripts/lib/secure-env.sh."
    Assert-ContainsLiteral -Path $launcher `
      -Expected 'assert_secure_env "$ROOT_DIR/.env"' `
      -Message "$launcherName does not verify .env before launch."
  }

  $postdeploy = Join-Path $resolvedStage "scripts\remote-rpi-postdeploy.sh"
  Assert-RegularNonLinkFile -Path $postdeploy -Description "remote-rpi-postdeploy.sh"
  Assert-ContainsLiteral -Path $postdeploy `
    -Expected '"$APP_DIR"/scripts/lib/*.sh' `
    -Message "Remote postdeploy does not restore helper executable mode."
  Assert-ContainsLiteral -Path $postdeploy `
    -Expected 'source "$APP_DIR/scripts/lib/secure-env.sh"' `
    -Message "Remote postdeploy does not source the exact helper path."
  Assert-ContainsLiteral -Path $postdeploy `
    -Expected 'create_secure_env "$APP_DIR/.env.example" "$APP_DIR/.env"' `
    -Message "Remote postdeploy does not use create_secure_env."
}

function New-RpiSecurityBundleFixture {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path
  )

  if (Test-Path -LiteralPath $Path) {
    Remove-Item -LiteralPath $Path -Recurse -Force
  }
  $scriptsDir = New-Item -ItemType Directory -Force -Path (Join-Path $Path "scripts")
  $libDir = New-Item -ItemType Directory -Force -Path (Join-Path $Path "scripts\lib")
  $null = $scriptsDir, $libDir

  Copy-Item -LiteralPath $SourceHelper -Destination (Join-Path $Path "scripts\lib\secure-env.sh")
  foreach ($name in @("prod-bootstrap.sh", "prod-dashboard.sh", "prod-bot.sh", "remote-rpi-postdeploy.sh")) {
    Copy-Item -LiteralPath (Join-Path $RepoRoot "scripts\$name") -Destination (Join-Path $Path "scripts\$name")
  }

  if (-not $RunningOnWindows) {
    foreach ($file in Get-ChildItem -LiteralPath (Join-Path $Path "scripts") -Filter "*.sh" -Recurse) {
      $mode = [IO.File]::GetUnixFileMode($file.FullName)
      [IO.File]::SetUnixFileMode($file.FullName, $mode -bor [IO.UnixFileMode]::UserExecute)
    }
  }
}

function Assert-ContractFailure {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Name,

    [Parameter(Mandatory = $true)]
    [scriptblock]$Action
  )

  $failed = $false
  try {
    & $Action
  }
  catch {
    $failed = $true
  }
  if (-not $failed) {
    throw "$Name fixture unexpectedly passed."
  }
}

if ($SelfTest) {
  $tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("dynamo-rpi-security-" + [guid]::NewGuid().ToString("N"))
  $fixture = Join-Path $tempRoot "stage"
  try {
    New-RpiSecurityBundleFixture -Path $fixture
    Assert-RpiSecurityBundle -Path $fixture

    Remove-Item -LiteralPath (Join-Path $fixture "scripts\lib\secure-env.sh")
    Assert-ContractFailure -Name "missing helper" -Action { Assert-RpiSecurityBundle -Path $fixture }

    New-RpiSecurityBundleFixture -Path $fixture
    Move-Item -LiteralPath (Join-Path $fixture "scripts\lib\secure-env.sh") `
      -Destination (Join-Path $fixture "scripts\secure-env.sh")
    Assert-ContractFailure -Name "flattened helper" -Action { Assert-RpiSecurityBundle -Path $fixture }

    New-RpiSecurityBundleFixture -Path $fixture
    Add-Content -LiteralPath (Join-Path $fixture "scripts\lib\secure-env.sh") -Value "`n# fixture hash mismatch"
    Assert-ContractFailure -Name "hash mismatch" -Action { Assert-RpiSecurityBundle -Path $fixture }

    if (-not $RunningOnWindows) {
      New-RpiSecurityBundleFixture -Path $fixture
      $helper = Join-Path $fixture "scripts\lib\secure-env.sh"
      $mode = [IO.File]::GetUnixFileMode($helper)
      $nonExecutableMode = [IO.UnixFileMode](
        ([int]$mode) -band (-bnot [int][IO.UnixFileMode]::UserExecute)
      )
      [IO.File]::SetUnixFileMode($helper, $nonExecutableMode)
      Assert-ContractFailure -Name "non-executable helper" -Action { Assert-RpiSecurityBundle -Path $fixture }
    }

    New-RpiSecurityBundleFixture -Path $fixture
    $botPath = Join-Path $fixture "scripts\prod-bot.sh"
    $botContent = (Get-Content -LiteralPath $botPath -Raw).Replace(
      'source "$ROOT_DIR/scripts/lib/secure-env.sh"',
      'source "$ROOT_DIR/scripts/secure-env.sh"'
    )
    Set-Content -LiteralPath $botPath -Value $botContent -NoNewline
    Assert-ContractFailure -Name "entrypoint source drift" -Action { Assert-RpiSecurityBundle -Path $fixture }

    Write-Host "Raspberry Pi security bundle PowerShell contract tests passed"
  }
  finally {
    if (Test-Path -LiteralPath $tempRoot) {
      Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
  }
}
else {
  Assert-RpiSecurityBundle -Path $StageDir
  Write-Host "Raspberry Pi security bundle PowerShell contract passed: $StageDir"
}
