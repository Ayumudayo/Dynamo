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
    if !value.is_finite() {
        return "0".to_string();
    }
    let rounded = (value * 100.0).round() / 100.0;
    let sign = if rounded < 0.0 { "-" } else { "" };
    let cents = (rounded.abs() * 100.0).round() as u128;
    let integer = cents / 100;
    let fraction = (cents % 100) as u64;
    let integer = format_grouped_integer(integer);

    if fraction == 0 {
        return format!("{sign}{integer}");
    }

    if fraction.is_multiple_of(10) {
        return format!("{sign}{integer}.{}", fraction / 10);
    }

    format!("{sign}{integer}.{fraction:02}")
}

fn format_grouped_integer(value: u128) -> String {
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
        "KRW" => "South Korean Won (KRW)",
        "USD" => "United States Dollar (USD)",
        _ => "Custom Currency",
    }
}
