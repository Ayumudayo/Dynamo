#[derive(Debug, Clone, Copy)]
pub struct CurrencySpec {
    pub code: &'static str,
    pub label: &'static str,
}

pub const CACHED_EXCHANGE_CURRENCIES: [&str; 18] = [
    "USD", "KRW", "EUR", "GBP", "JPY", "CAD", "CHF", "HKD", "TWD", "AUD", "NZD", "INR", "BRL",
    "PLN", "RUB", "TRY", "CNY", "UAH",
];

pub const SUPPORTED_CURRENCY_SPECS: &[CurrencySpec] = &[
    CurrencySpec {
        code: "AED",
        label: "🇦🇪 AED",
    },
    CurrencySpec {
        code: "AFN",
        label: "🇦🇫 AFN",
    },
    CurrencySpec {
        code: "ALL",
        label: "🇦🇱 ALL",
    },
    CurrencySpec {
        code: "AMD",
        label: "🇦🇲 AMD",
    },
    CurrencySpec {
        code: "ANG",
        label: "🇨🇼 ANG",
    },
    CurrencySpec {
        code: "AOA",
        label: "🇦🇴 AOA",
    },
    CurrencySpec {
        code: "ARS",
        label: "🇦🇷 ARS",
    },
    CurrencySpec {
        code: "AUD",
        label: "🇦🇺 AUD",
    },
    CurrencySpec {
        code: "AWG",
        label: "🇦🇼 AWG",
    },
    CurrencySpec {
        code: "AZN",
        label: "🇦🇿 AZN",
    },
    CurrencySpec {
        code: "BAM",
        label: "🇧🇦 BAM",
    },
    CurrencySpec {
        code: "BBD",
        label: "🇧🇧 BBD",
    },
    CurrencySpec {
        code: "BDT",
        label: "🇧🇩 BDT",
    },
    CurrencySpec {
        code: "BGN",
        label: "🇧🇬 BGN",
    },
    CurrencySpec {
        code: "BHD",
        label: "🇧🇭 BHD",
    },
    CurrencySpec {
        code: "BIF",
        label: "🇧🇮 BIF",
    },
    CurrencySpec {
        code: "BMD",
        label: "🇧🇲 BMD",
    },
    CurrencySpec {
        code: "BND",
        label: "🇧🇳 BND",
    },
    CurrencySpec {
        code: "BOB",
        label: "🇧🇴 BOB",
    },
    CurrencySpec {
        code: "BRL",
        label: "🇧🇷 BRL",
    },
    CurrencySpec {
        code: "BSD",
        label: "🇧🇸 BSD",
    },
    CurrencySpec {
        code: "BTN",
        label: "🇧🇹 BTN",
    },
    CurrencySpec {
        code: "BWP",
        label: "🇧🇼 BWP",
    },
    CurrencySpec {
        code: "BYN",
        label: "🇧🇾 BYN",
    },
    CurrencySpec {
        code: "BZD",
        label: "🇧🇿 BZD",
    },
    CurrencySpec {
        code: "CAD",
        label: "🇨🇦 CAD",
    },
    CurrencySpec {
        code: "CDF",
        label: "🇨🇩 CDF",
    },
    CurrencySpec {
        code: "CHF",
        label: "🇨🇭 CHF",
    },
    CurrencySpec {
        code: "CLP",
        label: "🇨🇱 CLP",
    },
    CurrencySpec {
        code: "CNY",
        label: "🇨🇳 CNY",
    },
    CurrencySpec {
        code: "COP",
        label: "🇨🇴 COP",
    },
    CurrencySpec {
        code: "CRC",
        label: "🇨🇷 CRC",
    },
    CurrencySpec {
        code: "CUP",
        label: "🇨🇺 CUP",
    },
    CurrencySpec {
        code: "CVE",
        label: "🇨🇻 CVE",
    },
    CurrencySpec {
        code: "CZK",
        label: "🇨🇿 CZK",
    },
    CurrencySpec {
        code: "DJF",
        label: "🇩🇯 DJF",
    },
    CurrencySpec {
        code: "DKK",
        label: "🇩🇰 DKK",
    },
    CurrencySpec {
        code: "DOP",
        label: "🇩🇴 DOP",
    },
    CurrencySpec {
        code: "DZD",
        label: "🇩🇿 DZD",
    },
    CurrencySpec {
        code: "EGP",
        label: "🇪🇬 EGP",
    },
    CurrencySpec {
        code: "ERN",
        label: "🇪🇷 ERN",
    },
    CurrencySpec {
        code: "ETB",
        label: "🇪🇹 ETB",
    },
    CurrencySpec {
        code: "EUR",
        label: "🇪🇺 EUR",
    },
    CurrencySpec {
        code: "FJD",
        label: "🇫🇯 FJD",
    },
    CurrencySpec {
        code: "FKP",
        label: "🇫🇰 FKP",
    },
    CurrencySpec {
        code: "GBP",
        label: "🇬🇧 GBP",
    },
    CurrencySpec {
        code: "GEL",
        label: "🇬🇪 GEL",
    },
    CurrencySpec {
        code: "GHS",
        label: "🇬🇭 GHS",
    },
    CurrencySpec {
        code: "GIP",
        label: "🇬🇮 GIP",
    },
    CurrencySpec {
        code: "GMD",
        label: "🇬🇲 GMD",
    },
    CurrencySpec {
        code: "GNF",
        label: "🇬🇳 GNF",
    },
    CurrencySpec {
        code: "GTQ",
        label: "🇬🇹 GTQ",
    },
    CurrencySpec {
        code: "GYD",
        label: "🇬🇾 GYD",
    },
    CurrencySpec {
        code: "HKD",
        label: "🇭🇰 HKD",
    },
    CurrencySpec {
        code: "HNL",
        label: "🇭🇳 HNL",
    },
    CurrencySpec {
        code: "HTG",
        label: "🇭🇹 HTG",
    },
    CurrencySpec {
        code: "HUF",
        label: "🇭🇺 HUF",
    },
    CurrencySpec {
        code: "IDR",
        label: "🇮🇩 IDR",
    },
    CurrencySpec {
        code: "ILS",
        label: "🇮🇱 ILS",
    },
    CurrencySpec {
        code: "INR",
        label: "🇮🇳 INR",
    },
    CurrencySpec {
        code: "IQD",
        label: "🇮🇶 IQD",
    },
    CurrencySpec {
        code: "IRR",
        label: "🇮🇷 IRR",
    },
    CurrencySpec {
        code: "ISK",
        label: "🇮🇸 ISK",
    },
    CurrencySpec {
        code: "JMD",
        label: "🇯🇲 JMD",
    },
    CurrencySpec {
        code: "JOD",
        label: "🇯🇴 JOD",
    },
    CurrencySpec {
        code: "JPY",
        label: "🇯🇵 JPY",
    },
    CurrencySpec {
        code: "KES",
        label: "🇰🇪 KES",
    },
    CurrencySpec {
        code: "KGS",
        label: "🇰🇬 KGS",
    },
    CurrencySpec {
        code: "KHR",
        label: "🇰🇭 KHR",
    },
    CurrencySpec {
        code: "KMF",
        label: "🇰🇲 KMF",
    },
    CurrencySpec {
        code: "KRW",
        label: "🇰🇷 KRW",
    },
    CurrencySpec {
        code: "KWD",
        label: "🇰🇼 KWD",
    },
    CurrencySpec {
        code: "KYD",
        label: "🇰🇾 KYD",
    },
    CurrencySpec {
        code: "KZT",
        label: "🇰🇿 KZT",
    },
    CurrencySpec {
        code: "LAK",
        label: "🇱🇦 LAK",
    },
    CurrencySpec {
        code: "LBP",
        label: "🇱🇧 LBP",
    },
    CurrencySpec {
        code: "LKR",
        label: "🇱🇰 LKR",
    },
    CurrencySpec {
        code: "LRD",
        label: "🇱🇷 LRD",
    },
    CurrencySpec {
        code: "LSL",
        label: "🇱🇸 LSL",
    },
    CurrencySpec {
        code: "LYD",
        label: "🇱🇾 LYD",
    },
    CurrencySpec {
        code: "MAD",
        label: "🇲🇦 MAD",
    },
    CurrencySpec {
        code: "MDL",
        label: "🇲🇩 MDL",
    },
    CurrencySpec {
        code: "MGA",
        label: "🇲🇬 MGA",
    },
    CurrencySpec {
        code: "MKD",
        label: "🇲🇰 MKD",
    },
    CurrencySpec {
        code: "MMK",
        label: "🇲🇲 MMK",
    },
    CurrencySpec {
        code: "MNT",
        label: "🇲🇳 MNT",
    },
    CurrencySpec {
        code: "MOP",
        label: "🇲🇴 MOP",
    },
    CurrencySpec {
        code: "MRU",
        label: "🇲🇷 MRU",
    },
    CurrencySpec {
        code: "MUR",
        label: "🇲🇺 MUR",
    },
    CurrencySpec {
        code: "MVR",
        label: "🇲🇻 MVR",
    },
    CurrencySpec {
        code: "MWK",
        label: "🇲🇼 MWK",
    },
    CurrencySpec {
        code: "MXN",
        label: "🇲🇽 MXN",
    },
    CurrencySpec {
        code: "MYR",
        label: "🇲🇾 MYR",
    },
    CurrencySpec {
        code: "MZN",
        label: "🇲🇿 MZN",
    },
    CurrencySpec {
        code: "NAD",
        label: "🇳🇦 NAD",
    },
    CurrencySpec {
        code: "NGN",
        label: "🇳🇬 NGN",
    },
    CurrencySpec {
        code: "NIO",
        label: "🇳🇮 NIO",
    },
    CurrencySpec {
        code: "NOK",
        label: "🇳🇴 NOK",
    },
    CurrencySpec {
        code: "NPR",
        label: "🇳🇵 NPR",
    },
    CurrencySpec {
        code: "NZD",
        label: "🇳🇿 NZD",
    },
    CurrencySpec {
        code: "OMR",
        label: "🇴🇲 OMR",
    },
    CurrencySpec {
        code: "PAB",
        label: "🇵🇦 PAB",
    },
    CurrencySpec {
        code: "PEN",
        label: "🇵🇪 PEN",
    },
    CurrencySpec {
        code: "PGK",
        label: "🇵🇬 PGK",
    },
    CurrencySpec {
        code: "PHP",
        label: "🇵🇭 PHP",
    },
    CurrencySpec {
        code: "PKR",
        label: "🇵🇰 PKR",
    },
    CurrencySpec {
        code: "PLN",
        label: "🇵🇱 PLN",
    },
    CurrencySpec {
        code: "PYG",
        label: "🇵🇾 PYG",
    },
    CurrencySpec {
        code: "QAR",
        label: "🇶🇦 QAR",
    },
    CurrencySpec {
        code: "RON",
        label: "🇷🇴 RON",
    },
    CurrencySpec {
        code: "RSD",
        label: "🇷🇸 RSD",
    },
    CurrencySpec {
        code: "RUB",
        label: "🇷🇺 RUB",
    },
    CurrencySpec {
        code: "RWF",
        label: "🇷🇼 RWF",
    },
    CurrencySpec {
        code: "SAR",
        label: "🇸🇦 SAR",
    },
    CurrencySpec {
        code: "SBD",
        label: "🇸🇧 SBD",
    },
    CurrencySpec {
        code: "SCR",
        label: "🇸🇨 SCR",
    },
    CurrencySpec {
        code: "SDG",
        label: "🇸🇩 SDG",
    },
    CurrencySpec {
        code: "SEK",
        label: "🇸🇪 SEK",
    },
    CurrencySpec {
        code: "SGD",
        label: "🇸🇬 SGD",
    },
    CurrencySpec {
        code: "SHP",
        label: "🇸🇭 SHP",
    },
    CurrencySpec {
        code: "SLE",
        label: "🇸🇱 SLE",
    },
    CurrencySpec {
        code: "SOS",
        label: "🇸🇴 SOS",
    },
    CurrencySpec {
        code: "SRD",
        label: "🇸🇷 SRD",
    },
    CurrencySpec {
        code: "SSP",
        label: "🇸🇸 SSP",
    },
    CurrencySpec {
        code: "STN",
        label: "🇸🇹 STN",
    },
    CurrencySpec {
        code: "SVC",
        label: "🇸🇻 SVC",
    },
    CurrencySpec {
        code: "SYP",
        label: "🇸🇾 SYP",
    },
    CurrencySpec {
        code: "SZL",
        label: "🇸🇿 SZL",
    },
    CurrencySpec {
        code: "THB",
        label: "🇹🇭 THB",
    },
    CurrencySpec {
        code: "TJS",
        label: "🇹🇯 TJS",
    },
    CurrencySpec {
        code: "TMT",
        label: "🇹🇲 TMT",
    },
    CurrencySpec {
        code: "TND",
        label: "🇹🇳 TND",
    },
    CurrencySpec {
        code: "TOP",
        label: "🇹🇴 TOP",
    },
    CurrencySpec {
        code: "TRY",
        label: "🇹🇷 TRY",
    },
    CurrencySpec {
        code: "TTD",
        label: "🇹🇹 TTD",
    },
    CurrencySpec {
        code: "TWD",
        label: "🇹🇼 TWD",
    },
    CurrencySpec {
        code: "TZS",
        label: "🇹🇿 TZS",
    },
    CurrencySpec {
        code: "UAH",
        label: "🇺🇦 UAH",
    },
    CurrencySpec {
        code: "UGX",
        label: "🇺🇬 UGX",
    },
    CurrencySpec {
        code: "USD",
        label: "🇺🇸 USD",
    },
    CurrencySpec {
        code: "UYU",
        label: "🇺🇾 UYU",
    },
    CurrencySpec {
        code: "UZS",
        label: "🇺🇿 UZS",
    },
    CurrencySpec {
        code: "VES",
        label: "🇻🇪 VES",
    },
    CurrencySpec {
        code: "VND",
        label: "🇻🇳 VND",
    },
    CurrencySpec {
        code: "VUV",
        label: "🇻🇺 VUV",
    },
    CurrencySpec {
        code: "WST",
        label: "🇼🇸 WST",
    },
    CurrencySpec {
        code: "XAF",
        label: "🌍 XAF",
    },
    CurrencySpec {
        code: "XCD",
        label: "🌎 XCD",
    },
    CurrencySpec {
        code: "XOF",
        label: "🌍 XOF",
    },
    CurrencySpec {
        code: "XPF",
        label: "🌊 XPF",
    },
    CurrencySpec {
        code: "YER",
        label: "🇾🇪 YER",
    },
    CurrencySpec {
        code: "ZAR",
        label: "🇿🇦 ZAR",
    },
    CurrencySpec {
        code: "ZMW",
        label: "🇿🇲 ZMW",
    },
    CurrencySpec {
        code: "ZWL",
        label: "🇿🇼 ZWL",
    },
];

pub fn cached_exchange_currencies() -> &'static [&'static str] {
    &CACHED_EXCHANGE_CURRENCIES
}

pub fn supported_currency_specs() -> &'static [CurrencySpec] {
    SUPPORTED_CURRENCY_SPECS
}

pub fn currency_display_label(code: &str) -> Option<&'static str> {
    let normalized = code.trim().to_ascii_uppercase();
    SUPPORTED_CURRENCY_SPECS
        .iter()
        .find(|spec| spec.code == normalized)
        .map(|spec| spec.label)
}
