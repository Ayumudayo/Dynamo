Set-StrictMode -Version Latest

function Resolve-ReparseFreeExistingPath([string] $Path) {
    $full = [IO.Path]::GetFullPath($Path)
    $root = [IO.Path]::GetPathRoot($full)
    $relative = $full.Substring($root.Length)
    $current = $root.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
    if ([string]::IsNullOrEmpty($current)) { $current = $root }
    foreach ($part in $relative.Split(@([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar), [StringSplitOptions]::RemoveEmptyEntries)) {
        $current = Join-Path $current $part
        if (-not (Test-Path -LiteralPath $current)) { throw "Missing path ancestor: $current" }
        $item = Get-Item -LiteralPath $current -Force
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Reparse ancestor rejected: $current" }
    }
    $full
}
