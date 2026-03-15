use dynamo_core::{Context, DiscordCommand, Error, GatewayIntents, Module, ModuleManifest};

pub struct InfoModule;

impl Module for InfoModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new("info", "Info", true, GatewayIntents::GUILDS)
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        vec![ping(), about()]
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
        .modules
        .iter()
        .map(|module| module.display_name)
        .collect::<Vec<_>>()
        .join(", ");

    ctx.say(format!(
        "Dynamo Rust workspace foundation is running.\nLoaded modules: {}\nUptime: {}s",
        module_names, uptime
    ))
    .await?;

    Ok(())
}
