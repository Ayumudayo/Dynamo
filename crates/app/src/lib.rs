use std::sync::Arc;

use dynamo_core::ModuleRegistry;
use dynamo_persistence_mongo::{MongoPersistence, MongoPersistenceConfig};
use tracing::info;

pub fn module_registry() -> ModuleRegistry {
    ModuleRegistry::new(vec![Box::new(dynamo_module_info::InfoModule)])
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
