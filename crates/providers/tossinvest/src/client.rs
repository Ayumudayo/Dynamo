use crate::TossInvestConfig;

#[derive(Clone, Debug)]
pub struct TossInvestClient {
    config: TossInvestConfig,
}

impl TossInvestClient {
    pub fn new(config: TossInvestConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &TossInvestConfig {
        &self.config
    }
}
