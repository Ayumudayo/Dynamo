use dynamo_runtime_api::Error;
use serde::Deserialize;

pub(crate) fn parse_message_id(value: &str) -> Result<u64, Error> {
    value
        .trim()
        .parse::<u64>()
        .map_err(|error| anyhow::anyhow!("Invalid message id `{value}`: {error}"))
}

pub(crate) fn parse_role_ids(value: Option<&str>) -> Result<Vec<u64>, Error> {
    match value {
        None => Ok(Vec::new()),
        Some(value) if value.trim().is_empty() => Ok(Vec::new()),
        Some(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|error| anyhow::anyhow!("Invalid role id `{value}`: {error}"))
            })
            .collect(),
    }
}

pub(crate) fn deserialize_optional_snowflake<'de, D>(
    deserializer: D,
) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(value) if value.trim().is_empty() => Ok(None),
        serde_json::Value::String(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(serde::de::Error::custom),
        serde_json::Value::Number(value) => value
            .as_u64()
            .ok_or_else(|| serde::de::Error::custom("snowflake number must be an unsigned integer"))
            .map(Some),
        other => Err(serde::de::Error::custom(format!(
            "snowflake must be a string or number, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_role_ids;
    #[test]
    fn parses_role_id_list() {
        assert_eq!(
            parse_role_ids(Some("1, 2,3")).expect("roles"),
            vec![1, 2, 3]
        );
    }
}
