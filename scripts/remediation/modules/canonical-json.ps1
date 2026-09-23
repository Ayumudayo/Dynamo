Set-StrictMode -Version Latest

$script:RemediationCanonicalUtf8 = [Text.UTF8Encoding]::new($false, $true)

function Get-Sha256Bytes([byte[]] $Bytes) {
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($Bytes)).ToLowerInvariant()
}

function Get-DomainHash([string] $Domain, [byte[]] $CanonicalBytes) {
    $prefix = $script:RemediationCanonicalUtf8.GetBytes($Domain + [char]0)
    $all = [byte[]]::new($prefix.Length + $CanonicalBytes.Length)
    [Array]::Copy($prefix, 0, $all, 0, $prefix.Length)
    [Array]::Copy($CanonicalBytes, 0, $all, $prefix.Length, $CanonicalBytes.Length)
    Get-Sha256Bytes $all
}

function Get-Sha256Text([string] $Domain, [byte[]] $CanonicalBytes) {
    Get-DomainHash -Domain $Domain -CanonicalBytes $CanonicalBytes
}

function Test-BytesEqual([byte[]] $Left, [byte[]] $Right) {
    if ($null -eq $Left -or $null -eq $Right -or $Left.Length -ne $Right.Length) { return $false }
    [Security.Cryptography.CryptographicOperations]::FixedTimeEquals($Left, $Right)
}

function Write-CanonicalJsonValue {
    param([Text.Json.Utf8JsonWriter] $Writer, [AllowNull()][object] $Value)
    if ($null -eq $Value) { $Writer.WriteNullValue(); return }
    if ($Value -is [string]) { $Writer.WriteStringValue([string]$Value); return }
    if ($Value -is [bool]) { $Writer.WriteBooleanValue([bool]$Value); return }
    if ($Value -is [byte] -or $Value -is [int16] -or $Value -is [int32] -or $Value -is [int64]) {
        $Writer.WriteNumberValue([int64]$Value); return
    }
    if ($Value -is [uint16] -or $Value -is [uint32] -or $Value -is [uint64]) {
        $Writer.WriteNumberValue([uint64]$Value); return
    }
    if ($Value -is [Collections.IDictionary]) {
        $Writer.WriteStartObject()
        foreach ($key in $Value.Keys) {
            $Writer.WritePropertyName([string]$key)
            Write-CanonicalJsonValue -Writer $Writer -Value $Value[$key]
        }
        $Writer.WriteEndObject()
        return
    }
    if (($Value -is [Collections.IEnumerable]) -and -not ($Value -is [string])) {
        $Writer.WriteStartArray()
        foreach ($item in $Value) { Write-CanonicalJsonValue -Writer $Writer -Value $item }
        $Writer.WriteEndArray()
        return
    }
    $Writer.WriteStartObject()
    foreach ($property in $Value.PSObject.Properties) {
        $Writer.WritePropertyName($property.Name)
        Write-CanonicalJsonValue -Writer $Writer -Value $property.Value
    }
    $Writer.WriteEndObject()
}

function ConvertTo-CanonicalBytes([object] $Value) {
    $stream = [IO.MemoryStream]::new()
    try {
        $writer = [Text.Json.Utf8JsonWriter]::new($stream, [Text.Json.JsonWriterOptions]@{
            Indented = $false
            SkipValidation = $false
            Encoder = [Text.Encodings.Web.JavaScriptEncoder]::UnsafeRelaxedJsonEscaping
        })
        try {
            Write-CanonicalJsonValue -Writer $writer -Value $Value
            $writer.Flush()
            $json = $stream.ToArray()
            $bytes = [byte[]]::new($json.Length + 1)
            [Array]::Copy($json, $bytes, $json.Length)
            $bytes[$json.Length] = 10
            $bytes
        } finally { $writer.Dispose() }
    } finally { $stream.Dispose() }
}

function Convert-JsonElement([Text.Json.JsonElement] $Element) {
    switch ($Element.ValueKind) {
        'Object' {
            $value = [ordered]@{}
            foreach ($property in $Element.EnumerateObject()) {
                if ($value.Contains($property.Name)) { throw "Duplicate JSON key: $($property.Name)" }
                $value[$property.Name] = Convert-JsonElement $property.Value
            }
            return $value
        }
        'Array' {
            $items = [Collections.Generic.List[object]]::new()
            foreach ($item in $Element.EnumerateArray()) { $items.Add((Convert-JsonElement $item)) }
            return ,$items.ToArray()
        }
        'String' { return $Element.GetString() }
        'Number' {
            [int64]$number = 0
            if (-not $Element.TryGetInt64([ref]$number)) { throw 'Only signed 64-bit integers are accepted.' }
            return $number
        }
        'True' { return $true }
        'False' { return $false }
        'Null' { return $null }
        default { throw "Unsupported JSON token: $($Element.ValueKind)" }
    }
}
