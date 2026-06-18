use std::{collections::BTreeMap, fmt};

use anyhow::{Result, anyhow};

const CLIENT_ID_ENV: &str = "TOSSINVEST_CLIENT_ID";
const CLIENT_SECRET_ENV: &str = "TOSSINVEST_CLIENT_SECRET";
const DEFAULT_BASE_URL: &str = "https://openapi.tossinvest.com";

#[derive(Clone, PartialEq, Eq)]
pub struct TossInvestConfig {
    client_id: String,
    client_secret: String,
    pub base_url: String,
}

impl TossInvestConfig {
    pub fn from_env() -> Result<Self> {
        let env = std::env::vars().collect::<BTreeMap<_, _>>();
        Self::from_map(&env)
    }

    pub fn from_map(env: &BTreeMap<String, String>) -> Result<Self> {
        Ok(Self {
            client_id: required_value(env, CLIENT_ID_ENV)?,
            client_secret: required_value(env, CLIENT_SECRET_ENV)?,
            base_url: DEFAULT_BASE_URL.to_string(),
        })
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn client_secret(&self) -> &str {
        &self.client_secret
    }
}

impl fmt::Debug for TossInvestConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TossInvestConfig")
            .field("client_id", &"<redacted>")
            .field("client_secret", &"<redacted>")
            .field("base_url", &self.base_url)
            .finish()
    }
}

fn required_value(env: &BTreeMap<String, String>, key: &str) -> Result<String> {
    env.get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("Missing required environment variable {key}"))
}

#[cfg(test)]
mod tests {
    use super::TossInvestConfig;

    #[test]
    fn config_rejects_missing_credentials() {
        let env = std::collections::BTreeMap::new();
        let error = TossInvestConfig::from_map(&env).unwrap_err().to_string();
        assert!(error.contains("TOSSINVEST_CLIENT_ID"));
    }

    #[test]
    fn config_reads_credentials_without_logging_values() {
        let env = std::collections::BTreeMap::from([
            ("TOSSINVEST_CLIENT_ID".to_string(), "id".to_string()),
            ("TOSSINVEST_CLIENT_SECRET".to_string(), "secret".to_string()),
        ]);
        let config = TossInvestConfig::from_map(&env).unwrap();
        assert_eq!(config.base_url.as_str(), "https://openapi.tossinvest.com");
    }
}
