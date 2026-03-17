use async_trait::async_trait;
use dynamo_domain_currency::{
    ExchangeRateCacheStatus, ExchangeRateQuote, ExchangeRateRefreshResult,
};

pub type Error = anyhow::Error;

#[async_trait]
pub trait ExchangeRateService: Send + Sync {
    async fn fetch_pair(&self, from: &str, to: &str) -> Result<ExchangeRateQuote, Error>;
    async fn refresh_cache(&self) -> Result<ExchangeRateRefreshResult, Error>;
    async fn cache_status(&self) -> Result<ExchangeRateCacheStatus, Error>;
    fn cache_target_count(&self) -> usize;
    fn uses_persisted_cache(&self) -> bool;
}
