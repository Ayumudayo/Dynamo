use std::{collections::BTreeMap, fmt};

use anyhow::{Result, anyhow};

const CLIENT_ID_ENV: &str = "TOSSINVEST_CLIENT_ID";
const CLIENT_SECRET_ENV: &str = "TOSSINVEST_CLIENT_SECRET";
const BASE_URL_ENV: &str = "TOSSINVEST_BASE_URL";
const DEFAULT_BASE_URL: &str = "https://openapi.tossinvest.com";

#[derive(Clone, PartialEq, Eq)]
pub struct TossInvestConfig {
    client_id: String,
    client_secret: String,
    pub base_url: String,
}

impl TossInvestConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            client_id: required_env_value(CLIENT_ID_ENV)?,
            client_secret: required_env_value(CLIENT_SECRET_ENV)?,
            base_url: optional_env_value(BASE_URL_ENV)?.unwrap_or_else(default_base_url),
        })
    }

    pub fn from_map(env: &BTreeMap<String, String>) -> Result<Self> {
        Ok(Self {
            client_id: required_value(env, CLIENT_ID_ENV)?,
            client_secret: required_value(env, CLIENT_SECRET_ENV)?,
            base_url: optional_value(env, BASE_URL_ENV).unwrap_or_else(default_base_url),
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

fn optional_value(env: &BTreeMap<String, String>, key: &str) -> Option<String> {
    env.get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn required_env_value(key: &str) -> Result<String> {
    match std::env::var(key) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Err(anyhow!("Missing required environment variable {key}"))
            } else {
                Ok(trimmed.to_string())
            }
        }
        Err(std::env::VarError::NotPresent) => {
            Err(anyhow!("Missing required environment variable {key}"))
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(anyhow!("Environment variable {key} is not valid Unicode"))
        }
    }
}

fn optional_env_value(key: &str) -> Result<Option<String>> {
    match std::env::var(key) {
        Ok(value) => Ok({
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(anyhow!("Environment variable {key} is not valid Unicode"))
        }
    }
}

fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}

#[cfg(test)]
mod tests {
    use super::TossInvestConfig;
    use std::{ffi::OsString, sync::Mutex};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct ScopedEnvVar {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: this test module serializes environment mutation with ENV_LOCK,
            // and scoped guards restore the variables before the lock is released.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: this test module serializes environment mutation with ENV_LOCK,
            // and scoped guards restore the variables before the lock is released.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            // SAFETY: ScopedEnvVar values are dropped while ENV_LOCK is still held
            // by the test that created them, so restore mutation is serialized too.
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn config_rejects_missing_credentials() {
        let env = std::collections::BTreeMap::new();
        let error = TossInvestConfig::from_map(&env).unwrap_err().to_string();
        assert!(error.contains("TOSSINVEST_CLIENT_ID"));
    }

    #[test]
    fn config_rejects_missing_client_secret() {
        let env = std::collections::BTreeMap::from([(
            "TOSSINVEST_CLIENT_ID".to_string(),
            "configured-client-id".to_string(),
        )]);
        let error = TossInvestConfig::from_map(&env).unwrap_err().to_string();
        assert!(error.contains("TOSSINVEST_CLIENT_SECRET"));
    }

    #[test]
    fn config_rejects_whitespace_required_values() {
        let env = std::collections::BTreeMap::from([
            ("TOSSINVEST_CLIENT_ID".to_string(), "   ".to_string()),
            (
                "TOSSINVEST_CLIENT_SECRET".to_string(),
                "configured-client-secret".to_string(),
            ),
        ]);
        let error = TossInvestConfig::from_map(&env).unwrap_err().to_string();
        assert!(error.contains("TOSSINVEST_CLIENT_ID"));

        let env = std::collections::BTreeMap::from([
            (
                "TOSSINVEST_CLIENT_ID".to_string(),
                "configured-client-id".to_string(),
            ),
            ("TOSSINVEST_CLIENT_SECRET".to_string(), "\t".to_string()),
        ]);
        let error = TossInvestConfig::from_map(&env).unwrap_err().to_string();
        assert!(error.contains("TOSSINVEST_CLIENT_SECRET"));
    }

    #[test]
    fn config_uses_default_base_url_without_override() {
        let env = std::collections::BTreeMap::from([
            (
                "TOSSINVEST_CLIENT_ID".to_string(),
                "configured-client-id".to_string(),
            ),
            (
                "TOSSINVEST_CLIENT_SECRET".to_string(),
                "configured-client-secret".to_string(),
            ),
        ]);
        let config = TossInvestConfig::from_map(&env).unwrap();
        assert_eq!(config.base_url.as_str(), "https://openapi.tossinvest.com");
    }

    #[test]
    fn config_accepts_base_url_override() {
        let env = std::collections::BTreeMap::from([
            (
                "TOSSINVEST_CLIENT_ID".to_string(),
                "configured-client-id".to_string(),
            ),
            (
                "TOSSINVEST_CLIENT_SECRET".to_string(),
                "configured-client-secret".to_string(),
            ),
            (
                "TOSSINVEST_BASE_URL".to_string(),
                "https://sandbox.openapi.tossinvest.example".to_string(),
            ),
        ]);
        let config = TossInvestConfig::from_map(&env).unwrap();
        assert_eq!(
            config.base_url.as_str(),
            "https://sandbox.openapi.tossinvest.example"
        );
    }

    #[test]
    fn config_reads_credentials_without_logging_values() {
        let env = std::collections::BTreeMap::from([
            (
                "TOSSINVEST_CLIENT_ID".to_string(),
                "configured-client-id".to_string(),
            ),
            (
                "TOSSINVEST_CLIENT_SECRET".to_string(),
                "configured-client-secret".to_string(),
            ),
        ]);
        let config = TossInvestConfig::from_map(&env).unwrap();
        assert_eq!(config.base_url.as_str(), "https://openapi.tossinvest.com");

        let debug = format!("{config:?}");
        assert!(!debug.contains("configured-client-id"));
        assert!(!debug.contains("configured-client-secret"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn config_reads_credentials_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
        let _client_id = ScopedEnvVar::set("TOSSINVEST_CLIENT_ID", "env-client-id");
        let _client_secret = ScopedEnvVar::set("TOSSINVEST_CLIENT_SECRET", "env-client-secret");
        let _base_url = ScopedEnvVar::remove("TOSSINVEST_BASE_URL");

        let config = TossInvestConfig::from_env().unwrap();

        assert_eq!(config.client_id(), "env-client-id");
        assert_eq!(config.client_secret(), "env-client-secret");
        assert_eq!(config.base_url.as_str(), "https://openapi.tossinvest.com");
    }

    #[test]
    fn config_reads_base_url_override_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
        let _client_id = ScopedEnvVar::set("TOSSINVEST_CLIENT_ID", "env-client-id");
        let _client_secret = ScopedEnvVar::set("TOSSINVEST_CLIENT_SECRET", "env-client-secret");
        let _base_url = ScopedEnvVar::set(
            "TOSSINVEST_BASE_URL",
            "https://sandbox.openapi.tossinvest.example",
        );

        let config = TossInvestConfig::from_env().unwrap();

        assert_eq!(
            config.base_url.as_str(),
            "https://sandbox.openapi.tossinvest.example"
        );
    }
}
