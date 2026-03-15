use std::sync::Arc;

use dynamo_core::{
    DeploymentSettingsRepository, GuildSettingsRepository, ModuleRegistry, Persistence,
};
use dynamo_persistence_mongo::{MongoPersistence, MongoPersistenceConfig};
use tracing::info;

pub fn module_registry() -> ModuleRegistry {
    ModuleRegistry::new(vec![
        Box::new(dynamo_module_info::InfoModule),
        Box::new(dynamo_module_gameinfo::GameInfoModule),
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
    let deployment_settings: Arc<dyn DeploymentSettingsRepository> = store;

    Ok(Persistence::new(
        database_name,
        Some(guild_settings),
        Some(deployment_settings),
    ))
}
