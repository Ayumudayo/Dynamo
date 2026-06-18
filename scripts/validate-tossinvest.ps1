param(
  [string]$KeyPath = $null,
  [string]$BaseUrl = "https://openapi.tossinvest.com",
  [string[]]$Symbols = @("SOXL", "TQQQ", "VOO")
)

$ErrorActionPreference = "Stop"
$AllowedTossHosts = @("openapi.tossinvest.com")

function Resolve-TossBaseUri {
  param(
    [string]$Value
  )

  try {
    $Uri = [System.Uri]$Value
  } catch {
    throw "Invalid Toss Invest base URL: $Value"
  }

  if (-not $Uri.IsAbsoluteUri) {
    throw "Toss Invest base URL must be absolute."
  }
  if ($Uri.Scheme -ne [System.Uri]::UriSchemeHttps) {
    throw "Toss Invest base URL must use https."
  }
  if (-not [string]::IsNullOrEmpty($Uri.UserInfo)) {
    throw "Toss Invest base URL must not include user info."
  }
  if ($Value -match '^[A-Za-z][A-Za-z0-9+.-]*://[^/?#]*:\d+(?:[/?#]|$)') {
    throw "Toss Invest base URL must not include an explicit port."
  }
  if ($Uri.AbsolutePath -ne "/") {
    throw "Toss Invest base URL must not include a path."
  }
  if (-not [string]::IsNullOrEmpty($Uri.Query) -or -not [string]::IsNullOrEmpty($Uri.Fragment)) {
    throw "Toss Invest base URL must not include a query or fragment."
  }

  $Allowed = $AllowedTossHosts | ForEach-Object { $_.Trim().ToLowerInvariant() }
  if ($Allowed -notcontains $Uri.Host.ToLowerInvariant()) {
    throw "Toss Invest base URL host is not allowed: $($Uri.Host)"
  }

  return [System.Uri]::new("https://$($Uri.Host.ToLowerInvariant())/")
}

function Join-TossUri {
  param(
    [System.Uri]$BaseUri,
    [string]$Path
  )

  if ([string]::IsNullOrWhiteSpace($Path) -or -not $Path.StartsWith("/")) {
    throw "Toss Invest API path must start with '/'."
  }

  return [System.Uri]::new($BaseUri, $Path)
}

function Get-BacktickSecretAfterLabel {
  param(
    [string[]]$Lines,
    [string]$Label
  )

  for ($Index = 0; $Index -lt $Lines.Count; $Index++) {
    if ($Lines[$Index].Trim() -ieq $Label) {
      for ($Next = $Index + 1; $Next -lt $Lines.Count; $Next++) {
        if ($Lines[$Next] -match '`([^`]+)`') {
          return $Matches[1].Trim()
        }
      }
    }
  }

  return $null
}

function Invoke-TossGet {
  param(
    [System.Uri]$BaseUri,
    [string]$Path,
    [string]$AccessToken
  )

  Invoke-RestMethod `
    -Method Get `
    -Uri (Join-TossUri -BaseUri $BaseUri -Path $Path) `
    -Headers @{
      Authorization = "Bearer $AccessToken"
      Accept = "application/json"
    }
}

$BaseUri = Resolve-TossBaseUri -Value $BaseUrl

$ClientId = $env:TOSSINVEST_CLIENT_ID
$ClientSecret = $env:TOSSINVEST_CLIENT_SECRET

if ([string]::IsNullOrWhiteSpace($ClientId) -or [string]::IsNullOrWhiteSpace($ClientSecret)) {
  if ([string]::IsNullOrWhiteSpace($KeyPath)) {
    throw "Set TOSSINVEST_CLIENT_ID/TOSSINVEST_CLIENT_SECRET or pass -KeyPath to a protected local secret file."
  }
  if (-not (Test-Path $KeyPath)) {
    throw "Missing Toss Invest API key file: $KeyPath"
  }

  $KeyLines = Get-Content $KeyPath
  $ClientId = Get-BacktickSecretAfterLabel -Lines $KeyLines -Label "API Key"
  $ClientSecret = Get-BacktickSecretAfterLabel -Lines $KeyLines -Label "Secret Key"
}

if ([string]::IsNullOrWhiteSpace($ClientId) -or [string]::IsNullOrWhiteSpace($ClientSecret)) {
  throw "Could not resolve Toss Invest API credentials."
}

$Token = Invoke-RestMethod `
  -Method Post `
  -Uri (Join-TossUri -BaseUri $BaseUri -Path "/oauth2/token") `
  -ContentType "application/x-www-form-urlencoded" `
  -Body @{
    grant_type = "client_credentials"
    client_id = $ClientId
    client_secret = $ClientSecret
  }

if ([string]::IsNullOrWhiteSpace($Token.access_token)) {
  throw "Toss OAuth token response did not include access_token"
}

$AccessToken = $Token.access_token
Write-Host "auth: ok expires_in=$($Token.expires_in)"

$Exchange = Invoke-TossGet -BaseUri $BaseUri -Path "/api/v1/exchange-rate?baseCurrency=USD&quoteCurrency=KRW" -AccessToken $AccessToken
Write-Host "exchange: USD/KRW midRate=$($Exchange.result.midRate)"

$Calendar = Invoke-TossGet -BaseUri $BaseUri -Path "/api/v1/market-calendar/US" -AccessToken $AccessToken
$Today = $Calendar.result.today
$OpenSessions = @("dayMarket", "preMarket", "regularMarket", "afterMarket") |
  Where-Object { $null -ne $Today.$_ }
Write-Host "calendar: today=$($Today.date) openSessions=$($OpenSessions -join ',')"

$NormalizedSymbols = $Symbols |
  ForEach-Object { $_.Trim().ToUpperInvariant() } |
  Where-Object { $_ -match '^[A-Z0-9.-]+$' } |
  Select-Object -Unique

if (-not $NormalizedSymbols -or $NormalizedSymbols.Count -eq 0) {
  throw "No valid symbols. Symbols may contain only letters, numbers, '.', and '-'."
}

$SymbolList = $NormalizedSymbols -join ","
$Prices = Invoke-TossGet -BaseUri $BaseUri -Path "/api/v1/prices?symbols=$SymbolList" -AccessToken $AccessToken
Write-Host "prices: requested=$($NormalizedSymbols.Count) returned=$($Prices.result.Count)"
foreach ($Price in $Prices.result) {
  Write-Host "price: $($Price.symbol) $($Price.lastPrice) $($Price.currency)"
}

$Stocks = Invoke-TossGet -BaseUri $BaseUri -Path "/api/v1/stocks?symbols=$SymbolList" -AccessToken $AccessToken
Write-Host "stocks: returned=$($Stocks.result.Count)"

$FirstSymbol = $NormalizedSymbols[0]
$Candles = Invoke-TossGet -BaseUri $BaseUri -Path "/api/v1/candles?symbol=$FirstSymbol&interval=1d&count=5&adjusted=true" -AccessToken $AccessToken
Write-Host "candles: symbol=$FirstSymbol returned=$($Candles.result.candles.Count)"
if ($Candles.result.candles.Count -gt 0) {
  $Latest = $Candles.result.candles[0]
  Write-Host "candle: latest=$($Latest.timestamp) close=$($Latest.closePrice) $($Latest.currency)"
}

Write-Host "tossinvest validation: ok"
