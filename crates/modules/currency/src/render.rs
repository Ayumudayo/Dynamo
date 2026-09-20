use dynamo_domain_currency::ExchangeRateQuote;

pub(super) fn normalize_currency(input: &str) -> String {
    input.trim().to_ascii_uppercase()
}

pub(super) fn currency_display_label(currency: &str) -> String {
    let normalized = normalize_currency(currency);
    dynamo_domain_currency::currency_display_label(&normalized)
        .map(str::to_string)
        .unwrap_or_else(|| format!("🌐 {normalized}"))
}

pub(super) fn format_rate_board_value(quote: &ExchangeRateQuote, amount: f64) -> String {
    format_decimal(quote.rate * amount)
}

pub(super) fn format_decimal(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    let sign = if rounded < 0.0 { "-" } else { "" };
    let absolute = rounded.abs();
    let integer = absolute.trunc() as i64;
    let fraction = ((absolute - integer as f64) * 100.0).round() as i64;
    let integer = format_grouped_integer(integer);

    if fraction == 0 {
        return format!("{sign}{integer}");
    }

    if fraction % 10 == 0 {
        return format!("{sign}{integer}.{}", fraction / 10);
    }

    format!("{sign}{integer}.{fraction:02}")
}

fn format_grouped_integer(value: i64) -> String {
    let digits = value.to_string();
    let mut output = String::new();
    let mut count = 0usize;

    for ch in digits.chars().rev() {
        if count == 3 {
            output.push(',');
            count = 0;
        }
        output.push(ch);
        count += 1;
    }

    output.chars().rev().collect()
}

pub(super) fn currency_option_label(code: &str) -> &'static str {
    match code {
        "AED" => "United Arab Emirates Dirham (AED)",
        "AFN" => "Afghan Afghani (AFN)",
        "ALL" => "Albanian Lek (ALL)",
        "AMD" => "Armenian Dram (AMD)",
        "ANG" => "Netherlands Antillean Guilder (ANG)",
        "AOA" => "Angolan Kwanza (AOA)",
        "ARS" => "Argentine Peso (ARS)",
        "AUD" => "Australian Dollar (AUD)",
        "AWG" => "Aruban Florin (AWG)",
        "AZN" => "Azerbaijani Manat (AZN)",
        "BAM" => "Bosnia and Herzegovina Convertible Mark (BAM)",
        "BBD" => "Barbadian Dollar (BBD)",
        "BDT" => "Bangladeshi Taka (BDT)",
        "BGN" => "Bulgarian Lev (BGN)",
        "BHD" => "Bahraini Dinar (BHD)",
        "BIF" => "Burundian Franc (BIF)",
        "BMD" => "Bermudian Dollar (BMD)",
        "BND" => "Brunei Dollar (BND)",
        "BOB" => "Bolivian Boliviano (BOB)",
        "BRL" => "Brazilian Real (BRL)",
        "BSD" => "Bahamian Dollar (BSD)",
        "BTN" => "Bhutanese Ngultrum (BTN)",
        "BWP" => "Botswana Pula (BWP)",
        "BYN" => "Belarusian Ruble (BYN)",
        "BZD" => "Belize Dollar (BZD)",
        "CAD" => "Canadian Dollar (CAD)",
        "CDF" => "Congolese Franc (CDF)",
        "CHF" => "Swiss Franc (CHF)",
        "CLP" => "Chilean Peso (CLP)",
        "CNY" => "Chinese Yuan (CNY)",
        "COP" => "Colombian Peso (COP)",
        "CRC" => "Costa Rican Colon (CRC)",
        "CUP" => "Cuban Peso (CUP)",
        "CVE" => "Cape Verdean Escudo (CVE)",
        "CZK" => "Czech Koruna (CZK)",
        "DJF" => "Djiboutian Franc (DJF)",
        "DKK" => "Danish Krone (DKK)",
        "DOP" => "Dominican Peso (DOP)",
        "DZD" => "Algerian Dinar (DZD)",
        "EGP" => "Egyptian Pound (EGP)",
        "ERN" => "Eritrean Nakfa (ERN)",
        "ETB" => "Ethiopian Birr (ETB)",
        "EUR" => "Euro (EUR)",
        "FJD" => "Fijian Dollar (FJD)",
        "FKP" => "Falkland Islands Pound (FKP)",
        "GBP" => "British Pound Sterling (GBP)",
        "GEL" => "Georgian Lari (GEL)",
        "GHS" => "Ghanaian Cedi (GHS)",
        "GIP" => "Gibraltar Pound (GIP)",
        "GMD" => "Gambian Dalasi (GMD)",
        "GNF" => "Guinean Franc (GNF)",
        "GTQ" => "Guatemalan Quetzal (GTQ)",
        "GYD" => "Guyanese Dollar (GYD)",
        "HKD" => "Hong Kong Dollar (HKD)",
        "HNL" => "Honduran Lempira (HNL)",
        "HTG" => "Haitian Gourde (HTG)",
        "HUF" => "Hungarian Forint (HUF)",
        "IDR" => "Indonesian Rupiah (IDR)",
        "ILS" => "Israeli New Shekel (ILS)",
        "INR" => "Indian Rupee (INR)",
        "IQD" => "Iraqi Dinar (IQD)",
        "IRR" => "Iranian Rial (IRR)",
        "ISK" => "Icelandic Krona (ISK)",
        "JMD" => "Jamaican Dollar (JMD)",
        "JOD" => "Jordanian Dinar (JOD)",
        "JPY" => "Japanese Yen (JPY)",
        "KES" => "Kenyan Shilling (KES)",
        "KGS" => "Kyrgyzstani Som (KGS)",
        "KHR" => "Cambodian Riel (KHR)",
        "KMF" => "Comorian Franc (KMF)",
        "KRW" => "South Korean Won (KRW)",
        "KWD" => "Kuwaiti Dinar (KWD)",
        "KYD" => "Cayman Islands Dollar (KYD)",
        "KZT" => "Kazakhstani Tenge (KZT)",
        "LAK" => "Lao Kip (LAK)",
        "LBP" => "Lebanese Pound (LBP)",
        "LKR" => "Sri Lankan Rupee (LKR)",
        "LRD" => "Liberian Dollar (LRD)",
        "LSL" => "Lesotho Loti (LSL)",
        "LYD" => "Libyan Dinar (LYD)",
        "MAD" => "Moroccan Dirham (MAD)",
        "MDL" => "Moldovan Leu (MDL)",
        "MGA" => "Malagasy Ariary (MGA)",
        "MKD" => "Macedonian Denar (MKD)",
        "MMK" => "Myanmar Kyat (MMK)",
        "MNT" => "Mongolian Tugrik (MNT)",
        "MOP" => "Macanese Pataca (MOP)",
        "MRU" => "Mauritanian Ouguiya (MRU)",
        "MUR" => "Mauritian Rupee (MUR)",
        "MVR" => "Maldivian Rufiyaa (MVR)",
        "MWK" => "Malawian Kwacha (MWK)",
        "MXN" => "Mexican Peso (MXN)",
        "MYR" => "Malaysian Ringgit (MYR)",
        "MZN" => "Mozambican Metical (MZN)",
        "NAD" => "Namibian Dollar (NAD)",
        "NGN" => "Nigerian Naira (NGN)",
        "NIO" => "Nicaraguan Cordoba (NIO)",
        "NOK" => "Norwegian Krone (NOK)",
        "NPR" => "Nepalese Rupee (NPR)",
        "NZD" => "New Zealand Dollar (NZD)",
        "OMR" => "Omani Rial (OMR)",
        "PAB" => "Panamanian Balboa (PAB)",
        "PEN" => "Peruvian Sol (PEN)",
        "PGK" => "Papua New Guinean Kina (PGK)",
        "PHP" => "Philippine Peso (PHP)",
        "PKR" => "Pakistani Rupee (PKR)",
        "PLN" => "Polish Zloty (PLN)",
        "PYG" => "Paraguayan Guarani (PYG)",
        "QAR" => "Qatari Riyal (QAR)",
        "RON" => "Romanian Leu (RON)",
        "RSD" => "Serbian Dinar (RSD)",
        "RUB" => "Russian Ruble (RUB)",
        "RWF" => "Rwandan Franc (RWF)",
        "SAR" => "Saudi Riyal (SAR)",
        "SBD" => "Solomon Islands Dollar (SBD)",
        "SCR" => "Seychellois Rupee (SCR)",
        "SDG" => "Sudanese Pound (SDG)",
        "SEK" => "Swedish Krona (SEK)",
        "SGD" => "Singapore Dollar (SGD)",
        "SHP" => "Saint Helena Pound (SHP)",
        "SLE" => "Sierra Leonean Leone (SLE)",
        "SOS" => "Somali Shilling (SOS)",
        "SRD" => "Surinamese Dollar (SRD)",
        "SSP" => "South Sudanese Pound (SSP)",
        "STN" => "Sao Tome and Principe Dobra (STN)",
        "SVC" => "Salvadoran Colon (SVC)",
        "SYP" => "Syrian Pound (SYP)",
        "SZL" => "Swazi Lilangeni (SZL)",
        "THB" => "Thai Baht (THB)",
        "TJS" => "Tajikistani Somoni (TJS)",
        "TMT" => "Turkmenistani Manat (TMT)",
        "TND" => "Tunisian Dinar (TND)",
        "TOP" => "Tongan Paʻanga (TOP)",
        "TRY" => "Turkish Lira (TRY)",
        "TTD" => "Trinidad and Tobago Dollar (TTD)",
        "TWD" => "New Taiwan Dollar (TWD)",
        "TZS" => "Tanzanian Shilling (TZS)",
        "UAH" => "Ukrainian Hryvnia (UAH)",
        "UGX" => "Ugandan Shilling (UGX)",
        "USD" => "United States Dollar (USD)",
        "UYU" => "Uruguayan Peso (UYU)",
        "UZS" => "Uzbekistani Som (UZS)",
        "VES" => "Venezuelan Bolivar (VES)",
        "VND" => "Vietnamese Dong (VND)",
        "VUV" => "Vanuatu Vatu (VUV)",
        "WST" => "Samoan Tala (WST)",
        "XAF" => "Central African CFA Franc (XAF)",
        "XCD" => "East Caribbean Dollar (XCD)",
        "XOF" => "West African CFA Franc (XOF)",
        "XPF" => "CFP Franc (XPF)",
        "YER" => "Yemeni Rial (YER)",
        "ZAR" => "South African Rand (ZAR)",
        "ZMW" => "Zambian Kwacha (ZMW)",
        "ZWL" => "Zimbabwean Dollar (ZWL)",
        _ => "Custom Currency",
    }
}
