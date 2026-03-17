param(
  [string]$OutputDir = ""
)

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$TemplateRoot = Join-Path $RepoRoot "templates\\rust-template"
if (-not $OutputDir) {
  $OutputDir = Join-Path $RepoRoot "output\\rust-template"
}

$IncludePaths = @(
  ".cargo",
  ".github",
  "Cargo.toml",
  "Cargo.lock",
  "LICENSE",
  "crates",
  "docs\\dev-smoke-checklist.md",
  "playwright.dashboard.config.cjs",
  "scripts\\dev-up.ps1",
  "scripts\\dev-down.ps1",
  "scripts\\dev-up.sh",
  "scripts\\dev-down.sh",
  "tests\\playwright"
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
Copy-Item (Join-Path $TemplateRoot ".env.example") (Join-Path $OutputDir ".env.example") -Force
Copy-Item (Join-Path $TemplateRoot "package.json") (Join-Path $OutputDir "package.json") -Force
Copy-Item (Join-Path $TemplateRoot ".gitignore") (Join-Path $OutputDir ".gitignore") -Force

Write-Host "Exported fresh Rust-only template staging repo to $OutputDir"
