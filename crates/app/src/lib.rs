use std::sync::Arc;

use dynamo_core::{
    AppState, DeploymentSettingsRepository, Error, GuildSettingsRepository, ModuleRegistry,
    Persistence, ProviderStateRepository, ServiceRegistry, StockQuoteService,
    SuggestionsRepository,
};
use dynamo_persistence_mongo::{MongoPersistence, MongoPersistenceConfig};
use poise::serenity_prelude::{Context, FullEvent};
use tracing::info;

pub fn module_registry() -> ModuleRegistry {
    ModuleRegistry::new(vec![
        Box::new(dynamo_module_info::InfoModule),
        Box::new(dynamo_module_gameinfo::GameInfoModule),
        Box::new(dynamo_module_suggestion::SuggestionModule),
        Box::new(dynamo_module_stock::StockModule),
    ])
}

pub async fn optional_mongo_from_env() -> anyhow::Result<Option<Arc<MongoPersistence>>> {
    let Some(config) = MongoPersistenceConfig::try_from_env()? else {
        info!("MongoDB configuration not found; continuing without persistence bootstrap");
        return Ok(None);
    };

    let store = MongoPersistence::connect(config).await?;
    store.ensure_initialized().await?;
    Ok(Some(Arc::new(store)))
}

pub async fn persistence_from_env() -> anyhow::Result<Persistence> {
    let Some(store) = optional_mongo_from_env().await? else {
        return Ok(Persistence::default());
    };

    let database_name = Some(store.database().name().to_string());
    let guild_settings: Arc<dyn GuildSettingsRepository> = store.clone();
    let deployment_settings: Arc<dyn DeploymentSettingsRepository> = store.clone();
    let suggestions: Arc<dyn SuggestionsRepository> = store.clone();
    let provider_state: Arc<dyn ProviderStateRepository> = store;

    Ok(Persistence::new(
        database_name,
        Some(guild_settings),
        Some(deployment_settings),
        Some(provider_state),
        Some(suggestions),
    ))
}

pub fn services_from_persistence(persistence: &Persistence) -> anyhow::Result<ServiceRegistry> {
    let stock_quotes: Arc<dyn StockQuoteService> = Arc::new(
        dynamo_provider_yahoo::YahooFinanceClient::new(persistence.provider_state.clone())?,
    );
    Ok(ServiceRegistry::new(Some(stock_quotes)))
}

pub async fn handle_framework_event(
    ctx: &Context,
    event: &FullEvent,
    data: &AppState,
) -> Result<(), Error> {
    if let FullEvent::InteractionCreate { interaction } = event {
        if dynamo_module_stock::handle_component_interaction(ctx, interaction).await? {
            return Ok(());
        }
        if dynamo_module_suggestion::handle_interaction(ctx, interaction, data).await? {
            return Ok(());
        }
    }

    Ok(())
}
