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
}

#[derive(Debug, Clone)]
pub struct MongoPersistence {
    config: MongoPersistenceConfig,
}

impl MongoPersistence {
    pub fn new(config: MongoPersistenceConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &MongoPersistenceConfig {
        &self.config
    }
}
