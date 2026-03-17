$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Fail([string]$Message) {
    throw "workspace-structure: $Message"
}

$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargo) {
    $cargo = Get-Command cargo.exe -ErrorAction SilentlyContinue
}
if (-not $cargo) {
    Fail 'cargo was not found on PATH'
}

$metadata = & $cargo.Source metadata --no-deps --format-version 1

$retiredMembers = @(
    '"crates/contracts"',
    '"crates/runtime"',
    '"crates/core"',
    '"crates/modules/music"',
    '"crates/providers/music-songbird"'
)

$cargoToml = Get-Content Cargo.toml -Raw
foreach ($member in $retiredMembers) {
    if ($cargoToml.Contains($member)) {
        Fail "retired workspace member still present in Cargo.toml: $member"
    }
}

$retiredPackages = @(
    '"name":"dynamo-contracts"',
    '"name":"dynamo-runtime"',
    '"name":"dynamo-core"',
    '"name":"dynamo-module-music"',
    '"name":"dynamo-provider-music-songbird"'
)

foreach ($package in $retiredPackages) {
    if ($metadata.Contains($package)) {
        Fail "retired package still present in cargo metadata: $package"
    }
}

$activeTargets = @(
    'Cargo.toml',
    'README.md',
    '.github/workflows',
    'crates',
    'scripts',
    'docs/dev-smoke-checklist.md',
    'docs/workspace-architecture.md'
)

$patterns = @(
    'dynamo-core',
    'dynamo-contracts',
    '(?<![A-Za-z0-9_-])dynamo-runtime(?![A-Za-z0-9_-])',
    'dynamo-module-music',
    'crates/modules/music',
    'dynamo-provider-music-songbird',
    'crates/providers/music-songbird',
    'songbird',
    'DAVE',
    'lavalink'
)

foreach ($pattern in $patterns) {
    $null = rg -n -P $pattern @activeTargets `
        -g '!scripts/check-workspace-structure.sh' `
        -g '!scripts/check-workspace-structure.ps1' `
        -g '!docs/cutover/**' `
        -g '!docs/rust-template/**'
    if ($LASTEXITCODE -eq 0) {
        Fail "retired pattern reintroduced into active surface: $pattern"
    }
    if ($LASTEXITCODE -gt 1) {
        throw "rg failed while checking pattern: $pattern"
    }
}

Write-Host 'workspace-structure: ok'
