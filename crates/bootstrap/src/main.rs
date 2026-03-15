use anyhow::Result;
use dynamo_persistence_mongo::{DEFAULT_DATABASE_NAME, MongoPersistence, MongoPersistenceConfig};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    init_tracing();

    let config = MongoPersistenceConfig::from_env()?;
    let database_name = config.database_name.clone();
    let store = MongoPersistence::connect(config).await?;
    store.ensure_initialized().await?;

    let collections = store.database().list_collection_names().await?;
    info!(
        database = %database_name,
        collections = ?collections,
        default_database = DEFAULT_DATABASE_NAME,
        "MongoDB bootstrap completed"
    );

    println!(
        "Bootstrapped MongoDB database '{}' with collections: {}",
        database_name,
        collections.join(", ")
    );

    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dynamo_bootstrap=info,dynamo_persistence_mongo=info".into()),
        )
        .try_init();
}
