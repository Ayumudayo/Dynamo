use dynamo_runtime_api::Error;

pub(crate) fn parse_message_id(value: &str) -> Result<u64, Error> {
    let candidate = value
        .trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim();
    let parsed = candidate
        .parse::<u64>()
        .map_err(|error| anyhow::anyhow!("Invalid message id `{value}`: {error}"))?;
    anyhow::ensure!(
        parsed > 0,
        "Invalid message id `{value}`: id cannot be zero"
    );
    Ok(parsed)
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
                let digits = value
                    .trim_start_matches("<@&")
                    .trim_start_matches("<@")
                    .trim_end_matches('>')
                    .trim();
                let parsed = digits
                    .parse::<u64>()
                    .map_err(|error| anyhow::anyhow!("Invalid role id `{value}`: {error}"))?;
                anyhow::ensure!(parsed > 0, "Invalid role id `{value}`: id cannot be zero");
                Ok(parsed)
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_message_id, parse_role_ids};
    #[test]
    fn parses_role_id_list() {
        assert_eq!(
            parse_role_ids(Some("1, <@&2>,3")).expect("roles"),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn parses_message_id_or_url() {
        assert_eq!(parse_message_id("12345").expect("id"), 12345);
        assert_eq!(
            parse_message_id("https://discord.com/channels/1/2/987654").expect("url"),
            987654
        );
        assert!(parse_message_id("0").is_err());
    }
}
