use std::env;

use crate::Error;

pub(crate) const DEPLOYMENT_SETTINGS_ID: &str = "global";
pub const DEFAULT_DATABASE_NAME: &str = "dynamo-rs";

#[derive(Debug, Clone)]
pub struct MongoPersistenceConfig {
    pub connection_string: String,
    pub database_name: String,
}

impl MongoPersistenceConfig {
    pub fn new(connection_string: impl Into<String>, database_name: impl Into<String>) -> Self {
        Self {
            connection_string: connection_string.into(),
            database_name: database_name.into(),
        }
    }

    pub fn from_env() -> Result<Self, Error> {
        let connection_string = env::var("MONGODB_URI")
            .or_else(|_| env::var("MONGO_CONNECTION"))
            .map_err(|_| anyhow::anyhow!("MONGODB_URI or MONGO_CONNECTION must be set"))?;
        let database_name =
            env::var("MONGODB_DATABASE").unwrap_or_else(|_| DEFAULT_DATABASE_NAME.to_string());

        Ok(Self::new(connection_string, database_name))
    }

    pub fn try_from_env() -> Result<Option<Self>, Error> {
        let connection_string =
            match env::var("MONGODB_URI").or_else(|_| env::var("MONGO_CONNECTION")) {
                Ok(value) => value,
                Err(env::VarError::NotPresent) => return Ok(None),
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "MongoDB connection environment could not be read: {error}"
                    ));
                }
            };
        let database_name =
            env::var("MONGODB_DATABASE").unwrap_or_else(|_| DEFAULT_DATABASE_NAME.to_string());

        Ok(Some(Self::new(connection_string, database_name)))
    }
}
