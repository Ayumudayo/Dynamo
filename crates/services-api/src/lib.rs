use std::sync::Arc;

use dynamo_service_exchange::ExchangeRateService;
use dynamo_service_stock::StockQuoteService;

#[derive(Clone, Default)]
pub struct ServiceRegistry {
    pub stock_quotes: Option<Arc<dyn StockQuoteService>>,
    pub exchange_rates: Option<Arc<dyn ExchangeRateService>>,
}

impl ServiceRegistry {
    pub fn new(
        stock_quotes: Option<Arc<dyn StockQuoteService>>,
        exchange_rates: Option<Arc<dyn ExchangeRateService>>,
    ) -> Self {
        Self {
            stock_quotes,
            exchange_rates,
        }
    }
}
