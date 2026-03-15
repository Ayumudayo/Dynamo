use dynamo_core::{
    Context, DiscordCommand, Error, GatewayIntents, Module, ModuleCategory, ModuleManifest,
    SettingsSchema,
};

pub struct InfoModule;

impl Module for InfoModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            "info",
            "Info",
            "Read-only commands that describe the bot and runtime.",
            ModuleCategory::Info,
            true,
            GatewayIntents::GUILDS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        vec![ping(), about()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema::empty()
    }
}

#[poise::command(slash_command, category = "Info")]
async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say("Pong!").await?;
    Ok(())
}

#[poise::command(slash_command, category = "Info")]
async fn about(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();
    let uptime = data.started_at.elapsed().as_secs();
    let module_names = data
        .module_catalog
        .entries
        .iter()
        .map(|entry| entry.module.display_name)
        .collect::<Vec<_>>()
        .join(", ");
    let persistence = data
        .persistence
        .database_name
        .as_deref()
        .unwrap_or("disabled");

    ctx.say(format!(
        "Dynamo Rust workspace foundation is running.\nLoaded modules: {}\nUptime: {}s\nPersistence: {}",
        module_names, uptime, persistence
    ))
    .await?;

    Ok(())
}
