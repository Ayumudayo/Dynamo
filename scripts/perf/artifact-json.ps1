Set-StrictMode -Version Latest

function Get-Sha256HexFromBytes {
    param([Parameter(Mandatory)][AllowEmptyCollection()][byte[]] $Bytes)
    $hash = [System.Security.Cryptography.SHA256]::HashData($Bytes)
    return [Convert]::ToHexString($hash).ToLowerInvariant()
}

function Get-Sha256HexFromString {
    param([Parameter(Mandatory)][AllowEmptyString()][string] $Value)
    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    return Get-Sha256HexFromBytes -Bytes $utf8NoBom.GetBytes($Value)
}

function Get-FileSha256Hex {
    param([Parameter(Mandatory)][string] $LiteralPath)
    try {
        $stream = [System.IO.FileStream]::new(
            $LiteralPath,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::Read)
        try {
            return [Convert]::ToHexString(
                [System.Security.Cryptography.SHA256]::HashData($stream)
            ).ToLowerInvariant()
        }
        finally {
            $stream.Dispose()
        }
    }
    catch {
        Throw-RunnerFailure 'artifact-hash-failed'
    }
}

function Get-RandomHex {
    param([ValidateRange(16, 128)][int] $ByteCount = 32)
    $bytes = [byte[]]::new($ByteCount)
    [System.Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
    return [Convert]::ToHexString($bytes).ToLowerInvariant()
}

function Assert-ExactJsonKeys {
    param(
        [Parameter(Mandatory)][object] $Value,
        [Parameter(Mandatory)][string[]] $Keys,
        [Parameter(Mandatory)][string] $FailureCode
    )
    if ($null -eq $Value -or $Value -is [string] -or $Value -is [System.Collections.IEnumerable] -and $Value -isnot [pscustomobject]) {
        Throw-RunnerFailure $FailureCode
    }
    $actual = @($Value.PSObject.Properties.Name | Sort-Object)
    $expected = @($Keys | Sort-Object)
    if ($actual.Count -ne $expected.Count) { Throw-RunnerFailure $FailureCode }
    for ($index = 0; $index -lt $expected.Count; $index++) {
        if ($actual[$index] -cne $expected[$index]) { Throw-RunnerFailure $FailureCode }
    }
}

function Write-ExclusiveJson {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [Parameter(Mandatory)][object] $Value
    )
    $parent = Split-Path -LiteralPath $LiteralPath
    [void](Assert-RegularPath -LiteralPath $parent -Kind Container -FailureCode 'artifact-parent-invalid')
    if (Test-Path -LiteralPath $LiteralPath) { Throw-RunnerFailure 'artifact-leaf-exists' }
    $body = ($Value | ConvertTo-Json -Depth 32 -Compress) + "`n"
    $temporary = Join-Path $parent ('.publish-' + (Get-RandomHex -ByteCount 16) + '.tmp')
    $published = $false
    try {
        $stream = [System.IO.FileStream]::new(
            $temporary,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None,
            4096,
            [System.IO.FileOptions]::WriteThrough)
        try {
            $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
            $bytes = $utf8NoBom.GetBytes($body)
            $stream.Write($bytes, 0, $bytes.Length)
            $stream.Flush($true)
        }
        finally {
            $stream.Dispose()
        }
        if ([System.IO.File]::ReadAllText($temporary, $utf8NoBom) -cne $body) {
            Throw-RunnerFailure 'artifact-temporary-readback-failed'
        }
        [System.IO.File]::Move($temporary, $LiteralPath, $false)
        $published = $true
        if ([System.IO.File]::ReadAllText($LiteralPath, $utf8NoBom) -cne $body) {
            Throw-RunnerFailure 'artifact-final-readback-failed'
        }
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure 'artifact-publication-failed'
    }
    finally {
        if (-not $published -and (Test-Path -LiteralPath $temporary)) {
            Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
        }
    }
}

function Read-BoundedUtf8File {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [ValidateRange(1, 16777216)][int] $MaximumBytes = 2097152,
        [switch] $AllowEmpty
    )
    try {
        $item = Assert-RegularPath -LiteralPath $LiteralPath -Kind Leaf -FailureCode 'child-log-invalid'
        if ($item.Length -gt $MaximumBytes -or (-not $AllowEmpty -and $item.Length -eq 0)) {
            Throw-RunnerFailure 'child-log-size-invalid'
        }
        $stream = [System.IO.FileStream]::new(
            $LiteralPath,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::ReadWrite)
        try {
            if ($stream.Length -gt $MaximumBytes) { Throw-RunnerFailure 'child-log-size-invalid' }
            $buffer = [byte[]]::new([Math]::Min(8192, $MaximumBytes))
            $content = [System.IO.MemoryStream]::new()
            try {
                while ($true) {
                    $read = $stream.Read($buffer, 0, $buffer.Length)
                    if ($read -le 0) { break }
                    $content.Write($buffer, 0, $read)
                    if ($content.Length -gt $MaximumBytes) {
                        Throw-RunnerFailure 'child-log-size-invalid'
                    }
                }
                if (-not $AllowEmpty -and $content.Length -eq 0) {
                    Throw-RunnerFailure 'child-log-size-invalid'
                }
                return [System.Text.UTF8Encoding]::new($false).GetString($content.ToArray())
            }
            finally { $content.Dispose() }
        }
        finally { $stream.Dispose() }
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure 'child-log-read-failed'
    }
}

function Read-ExactJsonFile {
    param(
        [Parameter(Mandatory)][string] $LiteralPath,
        [ValidateRange(1, 16777216)][int] $MaximumBytes = 2097152,
        [Parameter(Mandatory)][string] $FailureCode
    )
    try {
        $item = Assert-RegularPath -LiteralPath $LiteralPath -Kind Leaf -FailureCode $FailureCode
        if ($item.Length -le 0 -or $item.Length -gt $MaximumBytes) { Throw-RunnerFailure $FailureCode }
        $body = [System.IO.File]::ReadAllText($LiteralPath, [System.Text.UTF8Encoding]::new($false))
        return $body | ConvertFrom-Json -Depth 32
    }
    catch {
        if ($_.Exception.Data.Contains('DynamoRunnerCode')) { throw }
        Throw-RunnerFailure $FailureCode
    }
}

function ConvertFrom-ExactJsonLine {
    param(
        [Parameter(Mandatory)][string] $Body,
        [Parameter(Mandatory)][string] $FailureCode
    )
    if ($Body -notmatch '^\{[^\r\n]*\}\r?\n$') { Throw-RunnerFailure $FailureCode }
    try { return $Body.TrimEnd("`r", "`n") | ConvertFrom-Json -Depth 32 }
    catch { Throw-RunnerFailure $FailureCode }
}
