use std::sync::Arc;

use dynamo_core::{
    AppState, DeploymentSettingsRepository, Error, GuildSettingsRepository, ModuleRegistry,
    InviteRepository, MemberStatsRepository, Persistence, ProviderStateRepository, ServiceRegistry,
    StockQuoteService, SuggestionsRepository, WarningLogRepository,
};
use dynamo_persistence_mongo::{MongoPersistence, MongoPersistenceConfig};
use poise::serenity_prelude::{Context, FullEvent};
use tracing::info;

pub fn module_registry() -> ModuleRegistry {
    ModuleRegistry::new(vec![
        Box::new(dynamo_module_info::InfoModule),
        Box::new(dynamo_module_gameinfo::GameInfoModule),
        Box::new(dynamo_module_greeting::GreetingModule),
        Box::new(dynamo_module_invite::InviteModule),
        Box::new(dynamo_module_suggestion::SuggestionModule),
        Box::new(dynamo_module_stats::StatsModule),
        Box::new(dynamo_module_ticket::TicketModule),
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
        Some(invites),
        Some(member_stats),
        Some(warning_logs),
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
    match event {
        FullEvent::CacheReady { guilds } => {
            for guild_id in guilds {
                dynamo_module_invite::preload_guild_cache(ctx, data, *guild_id).await?;
            }
        }
        FullEvent::GuildMemberAddition { new_member } => {
            let inviter_data = dynamo_module_invite::track_joined_member(ctx, data, new_member)
                .await?;
            dynamo_module_greeting::send_welcome(
                ctx,
                data,
                new_member,
                inviter_data.as_ref(),
            )
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
        dynamo_module_stats::handle_interaction(ctx, data, interaction).await?;
    }

    Ok(())
}
