use async_trait::async_trait;
use dynamo_domain_stock::StockQuote;

pub type Error = anyhow::Error;

#[async_trait]
pub trait StockQuoteService: Send + Sync {
    async fn fetch_quote(&self, symbol: &str) -> Result<Option<StockQuote>, Error>;
    async fn fetch_quotes(
        &self,
        symbols: &[String],
    ) -> Result<Vec<Result<StockQuote, String>>, Error>;
}
