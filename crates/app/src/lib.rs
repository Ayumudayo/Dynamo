use std::sync::Arc;

use dynamo_core::{
    AppState, CommandCatalog, DeploymentSettings, DeploymentSettingsRepository, DiscordCommand,
    Error, GiveawaysRepository, GuildSettings, GuildSettingsRepository, InviteRepository,
    MemberStatsRepository, ModuleCatalog, ModuleRegistry, MusicService, OptionalModulesConfig,
    Persistence, ProviderStateRepository, ServiceRegistry, StockQuoteService,
    SuggestionsRepository, WarningLogRepository, resolve_command_state,
};
use dynamo_persistence_mongo::{MongoPersistence, MongoPersistenceConfig};
use poise::serenity_prelude::{Context, CreateCommand, FullEvent};
use tracing::info;

pub fn module_registry() -> ModuleRegistry {
    let optional_modules = OptionalModulesConfig::from_env().unwrap_or_default();
    module_registry_with_optional(&optional_modules)
}

pub fn module_registry_with_optional(optional_modules: &OptionalModulesConfig) -> ModuleRegistry {
    let mut modules: Vec<Box<dyn dynamo_core::Module>> = vec![
        Box::new(dynamo_module_currency::CurrencyModule),
        Box::new(dynamo_module_info::InfoModule),
        Box::new(dynamo_module_gameinfo::GameInfoModule),
        Box::new(dynamo_module_greeting::GreetingModule),
        Box::new(dynamo_module_invite::InviteModule),
        Box::new(dynamo_module_moderation::ModerationModule),
        Box::new(dynamo_module_music::MusicModule),
        Box::new(dynamo_module_suggestion::SuggestionModule),
        Box::new(dynamo_module_stats::StatsModule),
        Box::new(dynamo_module_ticket::TicketModule),
        Box::new(dynamo_module_stock::StockModule),
    ];

    if optional_modules.giveaway_enabled {
        modules.push(Box::new(dynamo_module_giveaway::GiveawayModule));
    }

    ModuleRegistry::new(modules)
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
    let giveaways: Arc<dyn GiveawaysRepository> = store.clone();
    let invites: Arc<dyn InviteRepository> = store.clone();
    let member_stats: Arc<dyn MemberStatsRepository> = store.clone();
    let warning_logs: Arc<dyn WarningLogRepository> = store.clone();
    let provider_state: Arc<dyn ProviderStateRepository> = store;

    Ok(Persistence::new(
        database_name,
        Some(guild_settings),
        Some(deployment_settings),
        Some(provider_state),
        Some(suggestions),
        Some(giveaways),
        Some(invites),
        Some(member_stats),
        Some(warning_logs),
    ))
}

pub fn services_from_persistence(persistence: &Persistence) -> anyhow::Result<ServiceRegistry> {
    let stock_quotes: Arc<dyn StockQuoteService> = Arc::new(
        dynamo_provider_yahoo::YahooFinanceClient::new(persistence.provider_state.clone())?,
    );
    let music: Arc<dyn MusicService> =
        Arc::new(dynamo_provider_music_songbird::SongbirdMusicService::new());
    Ok(ServiceRegistry::new(Some(stock_quotes), Some(music)))
}

pub fn create_application_commands_for_scope(
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> Vec<CreateCommand> {
    let registry = module_registry();
    let filtered_commands = filter_commands_for_scope(
        registry.commands(),
        registry.catalog(),
        registry.command_catalog(),
        deployment,
        guild,
    );
    poise::builtins::create_application_commands(&filtered_commands)
}

pub async fn handle_framework_event(
    ctx: &Context,
    event: &FullEvent,
    data: &AppState,
) -> Result<(), Error> {
    match event {
        FullEvent::CacheReady { guilds } => {
            for guild_id in guilds {
                dynamo_module_invite::preload_guild_cache(ctx, data, *guild_id).await?;
            }
        }
        FullEvent::GuildMemberAddition { new_member } => {
            let inviter_data =
                dynamo_module_invite::track_joined_member(ctx, data, new_member).await?;
            dynamo_module_greeting::send_welcome(ctx, data, new_member, inviter_data.as_ref())
                .await?;
        }
        FullEvent::GuildMemberRemoval {
            guild_id,
            user,
            member_data_if_available,
        } => {
            let inviter_data =
                dynamo_module_invite::track_left_member(ctx, data, *guild_id, user).await?;
            dynamo_module_greeting::send_farewell(
                ctx,
                data,
                *guild_id,
                user,
                member_data_if_available.as_ref(),
                inviter_data.as_ref(),
            )
            .await?;
        }
        FullEvent::InviteCreate { data: invite } => {
            dynamo_module_invite::handle_invite_create(ctx, data, invite).await?;
        }
        FullEvent::InviteDelete { data: invite } => {
            dynamo_module_invite::handle_invite_delete(ctx, data, invite).await?;
        }
        FullEvent::Message { new_message } => {
            dynamo_module_stats::handle_message(ctx, data, new_message).await?;
        }
        FullEvent::VoiceStateUpdate { old, new } => {
            dynamo_module_stats::handle_voice_state_update(ctx, data, old.as_ref(), new).await?;
        }
        _ => {}
    }

    if let FullEvent::InteractionCreate { interaction } = event {
        if dynamo_module_stock::handle_component_interaction(ctx, interaction).await? {
            return Ok(());
        }
        if dynamo_module_suggestion::handle_interaction(ctx, interaction, data).await? {
            return Ok(());
        }
        if dynamo_module_ticket::handle_interaction(ctx, interaction, data).await? {
            return Ok(());
        }
        if dynamo_module_giveaway::handle_interaction(ctx, interaction, data).await? {
            return Ok(());
        }
        dynamo_module_stats::handle_interaction(ctx, data, interaction).await?;
    }

    Ok(())
}

fn filter_commands_for_scope(
    commands: Vec<DiscordCommand>,
    module_catalog: &ModuleCatalog,
    command_catalog: &CommandCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> Vec<DiscordCommand> {
    commands
        .into_iter()
        .filter_map(|command| {
            filter_command_recursive(
                command,
                &mut Vec::new(),
                module_catalog,
                command_catalog,
                deployment,
                guild,
            )
        })
        .collect()
}

fn filter_command_recursive(
    mut command: DiscordCommand,
    path: &mut Vec<String>,
    module_catalog: &ModuleCatalog,
    command_catalog: &CommandCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> Option<DiscordCommand> {
    path.push(command.name.clone());

    if command.subcommands.is_empty() {
        let command_id = path.join("::");
        let enabled = resolve_command_state(
            module_catalog,
            command_catalog,
            deployment,
            guild,
            &command_id,
        )
        .map(|state| state.effective_enabled)
        .unwrap_or(true);
        path.pop();
        return enabled.then_some(command);
    }

    let mut filtered_subcommands = Vec::new();
    for subcommand in command.subcommands.into_iter() {
        if let Some(filtered) = filter_command_recursive(
            subcommand,
            path,
            module_catalog,
            command_catalog,
            deployment,
            guild,
        ) {
            filtered_subcommands.push(filtered);
        }
    }

    path.pop();
    if filtered_subcommands.is_empty() {
        return None;
    }

    command.subcommands = filtered_subcommands;
    Some(command)
}
