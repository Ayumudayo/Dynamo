use dynamo_runtime_api::Error;

use super::{render::normalize_currency, settings::ResolvedExchangeDefaults};

pub(super) const DEFAULT_EXCHANGE_FROM: &str = "USD";
pub(super) const DEFAULT_EXCHANGE_TO: &str = "KRW";
pub(super) const DEFAULT_EXCHANGE_AMOUNT: f64 = 1.0;
pub(super) const DEFAULT_RATE_TARGETS: [&str; 6] = ["KRW", "", "", "", "", ""];
pub(super) const TOSS_EXCHANGE_SUPPORT_ERROR: &str =
    "Only KRW and USD are supported by the current Toss Invest exchange-rate provider.";

pub(super) fn is_toss_maintenance_error(error: &Error) -> bool {
    error.to_string().contains("code: maintenance")
}

pub(super) fn default_rate_targets() -> Vec<String> {
    normalize_rate_targets(
        DEFAULT_RATE_TARGETS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    )
}

pub(super) fn supported_toss_currency(input: &str) -> Option<String> {
    let normalized = normalize_currency(input);
    match normalized.as_str() {
        "KRW" | "USD" => Some(normalized),
        _ => None,
    }
}

pub(super) fn sanitize_exchange_pair(from: String, to: String) -> (String, String) {
    let from =
        sanitize_persisted_currency(&from).unwrap_or_else(|| DEFAULT_EXCHANGE_FROM.to_string());
    let to = sanitize_persisted_currency(&to).unwrap_or_else(|| opposite_currency(&from));

    if from == to {
        return (from.clone(), opposite_currency(&from));
    }

    (from, to)
}

pub(super) fn resolve_explicit_exchange_pair(
    from: Option<&str>,
    to: Option<&str>,
    defaults: &ResolvedExchangeDefaults,
) -> Result<(String, String), &'static str> {
    match (from, to) {
        (Some(explicit_from), Some(explicit_to)) => validate_distinct_pair(
            validate_supported_explicit_currency(explicit_from)?,
            validate_supported_explicit_currency(explicit_to)?,
        ),
        (Some(explicit_from), None) => {
            let from = validate_supported_explicit_currency(explicit_from)?;
            let mut to = sanitize_persisted_currency(&defaults.default_to)
                .unwrap_or_else(|| opposite_currency(&from));
            if to == from {
                to = opposite_currency(&from);
            }
            Ok((from, to))
        }
        (None, Some(explicit_to)) => {
            let to = validate_supported_explicit_currency(explicit_to)?;
            let mut from = sanitize_persisted_currency(&defaults.default_from)
                .unwrap_or_else(|| DEFAULT_EXCHANGE_FROM.to_string());
            if from == to {
                from = opposite_currency(&to);
            }
            Ok((from, to))
        }
        (None, None) => Ok(sanitize_exchange_pair(
            defaults.default_from.clone(),
            defaults.default_to.clone(),
        )),
    }
}

pub(super) fn resolve_rate_base_currency(
    explicit_from: Option<&str>,
    defaults: &ResolvedExchangeDefaults,
) -> Result<String, &'static str> {
    match explicit_from {
        Some(explicit_from) => validate_supported_explicit_currency(explicit_from),
        None => Ok(sanitize_persisted_currency(&defaults.default_from)
            .unwrap_or_else(|| DEFAULT_EXCHANGE_FROM.to_string())),
    }
}

pub(super) fn sanitize_rate_targets(from: &str, targets: Vec<String>) -> Vec<String> {
    let normalized_from =
        sanitize_persisted_currency(from).unwrap_or_else(|| DEFAULT_EXCHANGE_FROM.to_string());
    let opposite = opposite_currency(&normalized_from);
    let mut values = targets
        .into_iter()
        .filter_map(|value| sanitize_persisted_currency(&value))
        .filter(|value| value != &normalized_from)
        .collect::<Vec<_>>();
    values.dedup();

    if values.is_empty() {
        values.push(opposite);
    }

    values
}

pub(super) fn normalize_rate_targets(targets: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    targets
        .into_iter()
        .map(|value| normalize_currency(&value))
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn sanitize_persisted_currency(input: &str) -> Option<String> {
    supported_toss_currency(input)
}

fn validate_supported_explicit_currency(input: &str) -> Result<String, &'static str> {
    supported_toss_currency(input).ok_or(TOSS_EXCHANGE_SUPPORT_ERROR)
}

fn validate_distinct_pair(from: String, to: String) -> Result<(String, String), &'static str> {
    if from == to {
        return Err(TOSS_EXCHANGE_SUPPORT_ERROR);
    }

    Ok((from, to))
}

fn opposite_currency(from: &str) -> String {
    if from == "KRW" {
        "USD".to_string()
    } else {
        "KRW".to_string()
    }
}
