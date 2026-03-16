use anyhow::Result;
use dynamo_core::{StartupPhase, StartupReport, StartupStatus};
use dynamo_persistence_mongo::{DEFAULT_DATABASE_NAME, MongoPersistence, MongoPersistenceConfig};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    init_tracing();

    let config = MongoPersistenceConfig::from_env()?;
    let database_name = config.database_name.clone();
    let connection_target = redact_connection_target(&config.connection_string);
    let store = MongoPersistence::connect(config).await?;
    let initialization = store.ensure_initialized_report().await?;

    let mut report = StartupReport::new("bootstrap");
    report.add_phase(
        StartupPhase::new(
            "connection",
            StartupStatus::Ok,
            format!("db={}", initialization.database_name),
        )
        .detail("connection_target", connection_target)
        .detail("database", initialization.database_name.clone())
        .detail("default_database", DEFAULT_DATABASE_NAME),
    );
    report.add_phase(
        StartupPhase::new(
            "initialization",
            StartupStatus::Ok,
            format!(
                "collections={} created={} existing={}",
                initialization.final_collections.len(),
                initialization.created_collections.len(),
                initialization.existing_collections.len()
            ),
        )
        .detail(
            "created",
            if initialization.created_collections.is_empty() {
                "none".to_string()
            } else {
                initialization.created_collections.join(", ")
            },
        )
        .detail(
            "already_existed",
            if initialization.existing_collections.is_empty() {
                "none".to_string()
            } else {
                initialization.existing_collections.join(", ")
            },
        )
        .detail(
            "final_collections",
            initialization.final_collections.join(", "),
        )
        .detail(
            "deployment_settings",
            if initialization.deployment_settings_seeded {
                "upserted deployment doc"
            } else {
                "deployment doc already present"
            },
        ),
    );
    report.log();

    println!(
        "Bootstrap summary | database={} | collections={} | created={} | existing={} | deployment_settings={}",
        database_name,
        initialization.final_collections.join(", "),
        if initialization.created_collections.is_empty() {
            "none".to_string()
        } else {
            initialization.created_collections.join(", ")
        },
        if initialization.existing_collections.is_empty() {
            "none".to_string()
        } else {
            initialization.existing_collections.join(", ")
        },
        if initialization.deployment_settings_seeded {
            "upserted"
        } else {
            "already-present"
        }
    );

    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stdout)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dynamo_bootstrap=info,dynamo_persistence_mongo=info".into()),
        )
        .try_init();
}

fn redact_connection_target(connection_string: &str) -> String {
    if let Some((scheme, rest)) = connection_string.split_once("://") {
        if let Some((_, host_and_path)) = rest.split_once('@') {
            return format!("{scheme}://***@{host_and_path}");
        }
        return format!("{scheme}://{rest}");
    }

    connection_string.to_string()
}
