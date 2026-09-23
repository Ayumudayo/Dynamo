[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$dashboardRoot = Join-Path $repoRoot 'crates/dashboard'
$fontRoot = Join-Path $dashboardRoot 'assets/fonts'
$lockPath = Join-Path $fontRoot 'fonts.lock.json'
$mainPath = Join-Path $dashboardRoot 'src/main.rs'
$libraryPath = Join-Path $dashboardRoot 'src/lib.rs'
$fontAssetsPath = Join-Path $dashboardRoot 'src/font_assets.rs'
$browserAssetsPath = Join-Path $dashboardRoot 'src/browser_assets.rs'
$buildPath = Join-Path $dashboardRoot 'build.rs'

function Assert-Contract {
    param(
        [Parameter(Mandatory)] [bool] $Condition,
        [Parameter(Mandatory)] [string] $Message
    )

    if (-not $Condition) {
        throw "dashboard font contract failed: $Message"
    }
}

foreach ($requiredPath in @($lockPath, $mainPath, $libraryPath, $fontAssetsPath, $browserAssetsPath, $buildPath)) {
    Assert-Contract (Test-Path -LiteralPath $requiredPath -PathType Leaf) "missing $requiredPath"
}

$lock = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json -Depth 32
Assert-Contract ($lock.schema_version -eq 1) 'fonts.lock.json schema_version must be 1'
Assert-Contract ($lock.runtime_downloads -eq $false) 'runtime_downloads must be false'
Assert-Contract ($lock.build_downloads -eq $false) 'build_downloads must be false'
Assert-Contract (@($lock.assets).Count -eq 6) 'exactly six reviewed font assets are required'
Assert-Contract (@($lock.licenses).Count -eq 2) 'both upstream OFL-1.1 license files are required'

$seenFiles = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
foreach ($entry in @($lock.assets) + @($lock.licenses)) {
    Assert-Contract ($entry.file -is [string] -and $entry.file -match '^[A-Za-z0-9._-]+$') 'lock file names must be simple relative names'
    Assert-Contract ($seenFiles.Add([string]$entry.file)) "duplicate lock entry: $($entry.file)"
    Assert-Contract ($entry.sha256 -is [string] -and $entry.sha256 -cmatch '^[0-9a-f]{64}$') "invalid sha256 for $($entry.file)"
    Assert-Contract ($entry.bytes -is [long] -or $entry.bytes -is [int]) "invalid byte count for $($entry.file)"
    Assert-Contract ([long]$entry.bytes -gt 0) "empty asset: $($entry.file)"
    Assert-Contract ($entry.source_url -is [string] -and $entry.source_url -cmatch '^https://') "source URL must use HTTPS for $($entry.file)"
    Assert-Contract ($entry.source_url -cnotmatch '/(main|master|latest)(/|$)') "source URL is not immutable for $($entry.file)"

    $path = Join-Path $fontRoot ([string]$entry.file)
    Assert-Contract (Test-Path -LiteralPath $path -PathType Leaf) "missing locked file $($entry.file)"
    $item = Get-Item -LiteralPath $path
    $actualHash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    Assert-Contract ($item.Length -eq [long]$entry.bytes) "byte count mismatch for $($entry.file)"
    Assert-Contract ($actualHash -ceq [string]$entry.sha256) "SHA-256 mismatch for $($entry.file)"
}

$sans = @($lock.assets | Where-Object family -CEQ 'Fira Sans' | Sort-Object { [int]$_.served_weight })
$code = @($lock.assets | Where-Object family -CEQ 'Fira Code')
Assert-Contract ($sans.Count -eq 5) 'Fira Sans must have five static weights'
Assert-Contract ((@($sans.served_weight) -join ',') -ceq '300,400,500,600,700') 'Fira Sans weights must be exactly 300,400,500,600,700'
Assert-Contract (@($sans | Where-Object format -CNE 'woff2').Count -eq 0) 'Fira Sans files must be WOFF2'
Assert-Contract (@($sans | Where-Object style -CNE 'normal').Count -eq 0) 'Fira Sans files must be normal style'
Assert-Contract (@($sans | Where-Object variable).Count -eq 0) 'Fira Sans files must not claim to be variable'
Assert-Contract ($code.Count -eq 1) 'Fira Code must have one variable asset'
Assert-Contract ($code[0].format -ceq 'woff2' -and $code[0].style -ceq 'normal' -and $code[0].variable -eq $true) 'Fira Code must be a normal variable WOFF2'
Assert-Contract ($code[0].weight_axis.min -eq 300 -and $code[0].weight_axis.max -eq 700) 'Fira Code source weight axis must be 300..700'
Assert-Contract ($code[0].served_weight_range.min -eq 500 -and $code[0].served_weight_range.max -eq 700) 'Fira Code served weights must remain 500..700'
Assert-Contract (@($sans | Where-Object upstream_repository -CNE 'https://github.com/bBoxType/FiraSans').Count -eq 0) 'Fira Sans repository must be the official bBoxType source'
Assert-Contract (@($sans | Where-Object { $_.upstream_commit -cnotmatch '^[0-9a-f]{40}$' }).Count -eq 0) 'Fira Sans entries must pin a commit'
Assert-Contract (@($sans | Where-Object { $_.source_url -cnotmatch '^https://raw\.githubusercontent\.com/bBoxType/FiraSans/[0-9a-f]{40}/' }).Count -eq 0) 'Fira Sans source URLs must be commit-pinned official raw URLs'
Assert-Contract ($code[0].upstream_repository -ceq 'https://github.com/tonsky/FiraCode') 'Fira Code repository must be the official tonsky source'
Assert-Contract ($code[0].upstream_commit -cmatch '^[0-9a-f]{40}$') 'Fira Code must pin a commit'
Assert-Contract ($code[0].release_tag -ceq '6.2') 'Fira Code release tag must be 6.2'
Assert-Contract ($code[0].source_url -ceq 'https://github.com/tonsky/FiraCode/releases/download/6.2/Fira_Code_v6.2.zip') 'Fira Code must use the official immutable release archive'
Assert-Contract ($code[0].release_archive_sha256 -cmatch '^[0-9a-f]{64}$' -and $code[0].release_archive_bytes -gt 0) 'Fira Code release archive hash and bytes are required'
Assert-Contract (@($lock.assets | Where-Object { $_.license_file -notin @($lock.licenses.file) }).Count -eq 0) 'every font must reference a locked license file'
foreach ($font in @($lock.assets)) {
    $bytes = [IO.File]::ReadAllBytes((Join-Path $fontRoot ([string]$font.file)))
    Assert-Contract ($bytes.Length -ge 4) "font file is truncated: $($font.file)"
    $magic = [Text.Encoding]::ASCII.GetString($bytes, 0, 4)
    Assert-Contract ($magic -ceq 'wOF2') "font file is not WOFF2: $($font.file)"
}

foreach ($license in @($lock.licenses)) {
    Assert-Contract ($license.spdx -ceq 'OFL-1.1') "unexpected license for $($license.file)"
}

$sansLicense = @($lock.licenses | Where-Object file -CEQ 'OFL-FiraSans.txt')
Assert-Contract ($sansLicense.Count -eq 1) 'Fira Sans license lock entry must be unique'
Assert-Contract ($sansLicense[0].source_bytes -eq 4512) 'Fira Sans upstream license byte count changed'
Assert-Contract ($sansLicense[0].source_sha256 -ceq '5c29650250730778eccb5475b112d32a8e0c9dd1860d9509693329652bf8e9eb') 'Fira Sans upstream license SHA-256 changed'
Assert-Contract ($sansLicense[0].normalization -ceq 'LF line endings and trailing ASCII whitespace removed') 'Fira Sans license normalization must remain explicit'

$build = Get-Content -LiteralPath $buildPath -Raw
foreach ($familyKey in @('fira_sans', 'fira_code')) {
    $metrics = $lock.fallback_metrics.$familyKey
    Assert-Contract ($null -ne $metrics) "missing fallback metrics for $familyKey"
    Assert-Contract ($metrics.measured_from -is [string] -and $metrics.measured_from.Length -gt 0) "missing metric provenance for $familyKey"
    Assert-Contract (@($metrics.faces).Count -gt 0) "missing fallback faces for $familyKey"
    foreach ($face in @($metrics.faces)) {
        Assert-Contract ($face.local_family -is [string] -and $face.local_family.Length -gt 0) "missing local fallback family for $familyKey"
        Assert-Contract ($face.weight -is [int] -or $face.weight -is [long]) "invalid fallback weight for $familyKey"
        foreach ($property in @('size_adjust', 'ascent_override', 'descent_override', 'line_gap_override')) {
            Assert-Contract ($face.$property -is [string] -and $face.$property -cmatch '^\d+(\.\d+)?%$') "invalid $property for $familyKey weight $($face.weight)"
            Assert-Contract ($build.Contains([string]$face.$property)) "build.rs metric $property drifted for $familyKey weight $($face.weight)"
        }
        Assert-Contract ($build.Contains([string]$face.local_family)) "build.rs local fallback drifted for $familyKey weight $($face.weight)"
    }
}
Assert-Contract ((@($lock.fallback_metrics.fira_sans.faces.weight) -join ',') -ceq '300,400,500,600,700') 'Fira Sans fallback weights must be exact'
Assert-Contract ((@($lock.fallback_metrics.fira_code.faces.weight) -join ',') -ceq '500,600,700') 'Fira Code fallback weights must be exact'

$main = Get-Content -LiteralPath $mainPath -Raw
$library = Get-Content -LiteralPath $libraryPath -Raw
$fontAssets = Get-Content -LiteralPath $fontAssetsPath -Raw
$browserAssets = Get-Content -LiteralPath $browserAssetsPath -Raw
Assert-Contract ($main -cnotmatch 'fonts\.googleapis\.com|fonts\.gstatic\.com') 'runtime Google Fonts reference remains'
Assert-Contract ($browserAssets -cnotmatch '@import\s+url\s*\(\s*["'']?https?://') 'remote CSS import remains'
Assert-Contract ($fontAssets -cmatch 'include!\(concat!\(env!\("OUT_DIR"\), "/font_assets\.rs"\)\)') 'generated font asset constants are not included'
Assert-Contract ($library -cmatch '\.merge\(font_asset_router\(\)\)') 'font asset router is not mounted'
Assert-Contract ($browserAssets -cmatch 'font-synthesis:\s*none') 'font synthesis must be disabled'
Assert-Contract ($build -cnotmatch 'https?://|reqwest|Invoke-WebRequest|curl') 'build.rs must never download font assets'
foreach ($entry in @($lock.assets) + @($lock.licenses)) {
    Assert-Contract ($build.Contains([string]$entry.sha256)) "build.rs does not pin $($entry.file)"
}

Write-Output 'dashboard font asset contract: PASS'
