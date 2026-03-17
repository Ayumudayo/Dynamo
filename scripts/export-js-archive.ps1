param(
  [string]$OutputDir = ""
)

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$TemplateRoot = Join-Path $RepoRoot "templates\\js-archive"
if (-not $OutputDir) {
  $OutputDir = Join-Path $RepoRoot "output\\js-archive"
}

$IncludePaths = @(
  "bot.js",
  "config.js",
  "dashboard",
  "docs\\commands",
  "jsconfig.json",
  "package.json",
  "package-lock.json",
  "scripts\\db-v4-to-v5.js",
  "src",
  ".eslintrc.json",
  ".prettierrc.json",
  "LICENSE"
)

Write-Host "Repo root:   $RepoRoot"
Write-Host "Template dir:$TemplateRoot"
Write-Host "Output dir:  $OutputDir"

if (Test-Path $OutputDir) {
  Remove-Item $OutputDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

foreach ($Path in $IncludePaths) {
  $Source = Join-Path $RepoRoot $Path
  $Target = Join-Path $OutputDir $Path
  $Parent = Split-Path $Target -Parent
  if ($Parent) {
    New-Item -ItemType Directory -Force -Path $Parent | Out-Null
  }
  Copy-Item $Source $Target -Recurse -Force
}

Copy-Item (Join-Path $TemplateRoot "README.md") (Join-Path $OutputDir "README.md") -Force
Copy-Item (Join-Path $TemplateRoot ".gitignore") (Join-Path $OutputDir ".gitignore") -Force

Write-Host "Exported JS archive staging repo to $OutputDir"
