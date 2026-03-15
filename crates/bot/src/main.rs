use dynamo_core::{AppConfig, AppState, Error, aggregate_intents};
use poise::serenity_prelude as serenity;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = dotenvy::dotenv();
    init_tracing();

    let config = AppConfig::from_env()?;
    let registry = dynamo_app::module_registry();
    let persistence = dynamo_app::persistence_from_env().await?;
    let manifests = registry.manifests();
    let commands = registry.commands();
    let intents = aggregate_intents(manifests.iter().copied());
    let setup_catalog = registry.catalog().clone();
    let discord_config = config.discord.clone();
    let setup_persistence = persistence.clone();

    if let Some(database_name) = persistence.database_name.as_deref() {
        info!(database = %database_name, "MongoDB persistence initialized");
    }

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands,
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            let discord_config = discord_config.clone();
            let setup_catalog = setup_catalog.clone();
            let setup_persistence = setup_persistence.clone();

            Box::pin(async move {
                info!(
                    user = %ready.user.name,
                    modules = setup_catalog.entries.len(),
                    "Connected to Discord"
                );

                if discord_config.register_globally {
                    poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                    info!("Registered application commands globally");
                } else if let Some(guild_id) = discord_config.dev_guild_id {
                    poise::builtins::register_in_guild(
                        ctx,
                        &framework.options().commands,
                        serenity::GuildId::new(guild_id),
                    )
                    .await?;
                    info!(
                        guild_id,
                        "Registered application commands in development guild"
                    );
                }

                Ok(AppState::new(setup_catalog, setup_persistence))
            })
        })
        .build();

    let mut client = serenity::ClientBuilder::new(config.discord.token, intents)
        .framework(framework)
        .await?;

    client.start().await?;
    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dynamo_bot=info,dynamo_core=info,poise=info".into()),
        )
        .try_init();
}
