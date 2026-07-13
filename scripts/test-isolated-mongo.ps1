[CmdletBinding()]
param(
    [ValidateNotNullOrEmpty()]
    [string] $Filter = 'against_mongo',

    [ValidateNotNullOrEmpty()]
    [string] $CargoCommand = 'cargo'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function ConvertTo-CanonicalDnsHost {
    param([Parameter(Mandatory)] [string] $HostName)

    if ($HostName.Length -eq 0 -or $HostName.StartsWith('.', [StringComparison]::Ordinal) -or
        $HostName.EndsWith('..', [StringComparison]::Ordinal)) {
        throw 'invalid Mongo URI'
    }

    $candidate = $HostName.TrimEnd('.')
    if ($candidate.Length -eq 0) {
        throw 'invalid Mongo URI'
    }

    try {
        $asciiHost = [Globalization.IdnMapping]::new().GetAscii($candidate)
    }
    catch {
        throw 'invalid Mongo URI'
    }
    if ([Uri]::CheckHostName($asciiHost) -ne [UriHostNameType]::Dns) {
        throw 'invalid Mongo URI'
    }

    return $asciiHost.ToLowerInvariant()
}

function ConvertTo-CanonicalMongoSeeds {
    param([Parameter(Mandatory)] [string] $ConnectionString)

    if ($ConnectionString.Length -eq 0 -or
        $ConnectionString -match '[\x00-\x20\x7f]' -or
        $ConnectionString -match '%(?![0-9A-Fa-f]{2})' -or
        $ConnectionString.Contains('#', [StringComparison]::Ordinal)) {
        throw 'invalid Mongo URI'
    }

    if ($ConnectionString.StartsWith('mongodb+srv://', [StringComparison]::OrdinalIgnoreCase)) {
        $transport = 'srv'
        $afterScheme = $ConnectionString.Substring('mongodb+srv://'.Length)
    }
    elseif ($ConnectionString.StartsWith('mongodb://', [StringComparison]::OrdinalIgnoreCase)) {
        $transport = 'direct'
        $afterScheme = $ConnectionString.Substring('mongodb://'.Length)
    }
    else {
        throw 'invalid Mongo URI'
    }

    $authorityEnd = $afterScheme.Length
    foreach ($delimiter in @('/', '?')) {
        $index = $afterScheme.IndexOf($delimiter, [StringComparison]::Ordinal)
        if ($index -ge 0 -and $index -lt $authorityEnd) {
            $authorityEnd = $index
        }
    }
    $authority = $afterScheme.Substring(0, $authorityEnd)
    if ($authority.Length -eq 0) {
        throw 'invalid Mongo URI'
    }

    $lastAt = $authority.LastIndexOf('@')
    if ($lastAt -ge 0) {
        $userInfo = $authority.Substring(0, $lastAt)
        if ($userInfo.Length -eq 0 -or $userInfo.Contains('@', [StringComparison]::Ordinal)) {
            throw 'invalid Mongo URI'
        }
        $authority = $authority.Substring($lastAt + 1)
    }
    if ($authority.Length -eq 0) {
        throw 'invalid Mongo URI'
    }

    if ($transport -ceq 'srv') {
        if ($authority.Contains(',', [StringComparison]::Ordinal) -or
            $authority.Contains(':', [StringComparison]::Ordinal) -or
            $authority.Contains('[', [StringComparison]::Ordinal) -or
            $authority.Contains(']', [StringComparison]::Ordinal)) {
            throw 'invalid Mongo URI'
        }

        return [pscustomobject]@{
            Transport = 'srv'
            Host = ConvertTo-CanonicalDnsHost $authority
            Port = $null
        }
    }

    $seedStrings = @($authority.Split(',', [StringSplitOptions]::None))
    if ($seedStrings.Count -eq 0 -or @($seedStrings | Where-Object { $_.Length -eq 0 }).Count -gt 0) {
        throw 'invalid Mongo URI'
    }

    $seenSeeds = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($seed in $seedStrings) {
        $port = 27017
        if ($seed.StartsWith('[', [StringComparison]::Ordinal)) {
            $closeBracket = $seed.IndexOf(']')
            if ($closeBracket -le 1) {
                throw 'invalid Mongo URI'
            }

            $addressText = $seed.Substring(1, $closeBracket - 1)
            $address = $null
            if (-not [Net.IPAddress]::TryParse($addressText, [ref]$address) -or
                $address.AddressFamily -ne [Net.Sockets.AddressFamily]::InterNetworkV6) {
                throw 'invalid Mongo URI'
            }
            $canonicalHost = $address.ToString().ToLowerInvariant()

            $remainder = $seed.Substring($closeBracket + 1)
            if ($remainder.Length -gt 0) {
                if (-not $remainder.StartsWith(':', [StringComparison]::Ordinal)) {
                    throw 'invalid Mongo URI'
                }
                $portText = $remainder.Substring(1)
                if ($portText -cnotmatch '^\d{1,5}$' -or
                    -not [int]::TryParse($portText, [ref]$port) -or
                    $port -lt 1 -or $port -gt 65535) {
                    throw 'invalid Mongo URI'
                }
            }
        }
        else {
            if ($seed.Contains('[', [StringComparison]::Ordinal) -or
                $seed.Contains(']', [StringComparison]::Ordinal)) {
                throw 'invalid Mongo URI'
            }

            $colonCount = @($seed.ToCharArray() | Where-Object { $_ -ceq ':' }).Count
            if ($colonCount -gt 1) {
                throw 'invalid Mongo URI'
            }
            if ($colonCount -eq 1) {
                $colonIndex = $seed.LastIndexOf(':')
                $hostText = $seed.Substring(0, $colonIndex)
                $portText = $seed.Substring($colonIndex + 1)
                if ($portText -cnotmatch '^\d{1,5}$' -or
                    -not [int]::TryParse($portText, [ref]$port) -or
                    $port -lt 1 -or $port -gt 65535) {
                    throw 'invalid Mongo URI'
                }
            }
            else {
                $hostText = $seed
            }
            if ($hostText.Length -eq 0) {
                throw 'invalid Mongo URI'
            }

            $address = $null
            if ([Net.IPAddress]::TryParse($hostText, [ref]$address)) {
                if ($address.AddressFamily -ne [Net.Sockets.AddressFamily]::InterNetwork) {
                    throw 'invalid Mongo URI'
                }
                $canonicalHost = $address.ToString()
            }
            else {
                $canonicalHost = ConvertTo-CanonicalDnsHost $hostText
            }
        }

        $seedKey = "$canonicalHost`:$port"
        if ($seenSeeds.Add($seedKey)) {
            [pscustomobject]@{
                Transport = 'direct'
                Host = $canonicalHost
                Port = $port
            }
        }
    }
}

function Test-MongoSeedOverlap {
    param(
        [Parameter(Mandatory)] [object[]] $Left,
        [Parameter(Mandatory)] [object[]] $Right
    )

    foreach ($leftSeed in $Left) {
        foreach ($rightSeed in $Right) {
            if (-not [string]::Equals($leftSeed.Host, $rightSeed.Host, [StringComparison]::Ordinal)) {
                continue
            }

            if ($leftSeed.Transport -ceq 'srv' -or $rightSeed.Transport -ceq 'srv' -or
                $leftSeed.Port -eq $rightSeed.Port) {
                return $true
            }
        }
    }
    return $false
}

if ($env:DYNAMO_MONGO_TEST -cne '1') {
    throw 'Isolated Mongo tests are disabled. Set DYNAMO_MONGO_TEST=1 to opt in.'
}

$testUri = [Environment]::GetEnvironmentVariable('MONGODB_TEST_URI', 'Process')
if ([string]::IsNullOrWhiteSpace($testUri)) {
    throw 'MONGODB_TEST_URI must be set for isolated Mongo tests.'
}

if ($Filter.Length -gt 128 -or
    $Filter.StartsWith('-', [StringComparison]::Ordinal) -or
    $Filter -cnotmatch '^[A-Za-z0-9_.:-]+$') {
    throw 'The isolated Mongo test filter contains unsupported characters.'
}

try {
    $testSeeds = @(ConvertTo-CanonicalMongoSeeds $testUri)
}
catch {
    throw 'MONGODB_TEST_URI cannot be safely parsed; refusing to run.'
}
if ($testSeeds.Count -eq 0) {
    throw 'MONGODB_TEST_URI contains no usable seed; refusing to run.'
}

$productionUriNames = @(
    'MONGODB_URI',
    'MONGO_CONNECTION',
    'DYNAMO_PRODUCTION_MONGODB_URI',
    'MONGODB_PRODUCTION_URI'
)
foreach ($name in $productionUriNames) {
    $productionUri = [Environment]::GetEnvironmentVariable($name, 'Process')
    if ([string]::IsNullOrWhiteSpace($productionUri)) {
        continue
    }

    try {
        $productionSeeds = @(ConvertTo-CanonicalMongoSeeds $productionUri)
    }
    catch {
        throw "Configured production Mongo URI $name cannot be safely parsed; refusing to run."
    }
    if ($productionSeeds.Count -eq 0) {
        throw "Configured production Mongo URI $name contains no usable seed; refusing to run."
    }
    if (Test-MongoSeedOverlap $testSeeds $productionSeeds) {
        throw 'The isolated Mongo test URI overlaps a configured production cluster seed; refusing to run.'
    }
}

$isolatedEnvironmentNames = @(
    'MONGODB_TEST_URI',
    'MONGODB_URI_FOR_ISOLATED_TEST',
    'MONGODB_URI',
    'MONGO_CONNECTION',
    'MONGODB_DATABASE',
    'DYNAMO_PRODUCTION_MONGODB_URI',
    'MONGODB_PRODUCTION_URI'
)
$environmentSnapshot = [ordered]@{}
foreach ($name in $isolatedEnvironmentNames) {
    $environmentSnapshot[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

$cargoBaseArguments = @(
    'test',
    '--locked',
    '-p',
    'dynamo-persistence-mongo',
    $Filter,
    '--',
    '--ignored'
)

try {
    foreach ($name in $isolatedEnvironmentNames) {
        Remove-Item -LiteralPath "Env:$name" -Force -ErrorAction SilentlyContinue
    }
    Set-Item -LiteralPath 'Env:MONGODB_URI_FOR_ISOLATED_TEST' -Value $testUri
    foreach ($name in $isolatedEnvironmentNames | Where-Object { $_ -cne 'MONGODB_URI_FOR_ISOLATED_TEST' }) {
        if ($null -ne [Environment]::GetEnvironmentVariable($name, 'Process')) {
            throw "Failed to remove ordinary Mongo environment variable $name before Cargo execution."
        }
    }

    try {
        $listOutput = @(& $CargoCommand @cargoBaseArguments '--list' 2>&1)
        $listExitCode = $LASTEXITCODE
    }
    catch {
        throw 'Cargo could not enumerate isolated Mongo tests.'
    }

    if ($listExitCode -ne 0) {
        throw "Cargo failed while enumerating isolated Mongo tests (exit code $listExitCode)."
    }

    $selectedTests = @(
        $listOutput |
            ForEach-Object { $_.ToString() } |
            Where-Object { $_ -cmatch '^\S.*:\s+test$' }
    )
    if ($selectedTests.Count -eq 0) {
        throw 'The isolated Mongo filter selected zero ignored tests.'
    }

    try {
        [void]@(& $CargoCommand @cargoBaseArguments '--test-threads=1' 2>&1)
        $runExitCode = $LASTEXITCODE
    }
    catch {
        throw 'Cargo could not execute isolated Mongo tests.'
    }

    if ($runExitCode -ne 0) {
        throw "Isolated Mongo tests failed (exit code $runExitCode); child output was suppressed to protect credentials."
    }

    "Isolated Mongo test gate passed ($($selectedTests.Count) selected)."
}
finally {
    foreach ($entry in $environmentSnapshot.GetEnumerator()) {
        $environmentPath = "Env:$([string]$entry.Key)"
        if ($null -eq $entry.Value) {
            Remove-Item -LiteralPath $environmentPath -Force -ErrorAction SilentlyContinue
        }
        else {
            Set-Item -LiteralPath $environmentPath -Value ([string]$entry.Value)
        }
    }
}
