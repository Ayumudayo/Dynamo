use std::{fmt::Write as _, sync::Arc};

use dynamo_enablement::resolve_command_state;
use dynamo_module_kit::{CommandCatalog, DiscordCommand, Module, ModuleCatalog};
use dynamo_ops::DashboardAuditLogRepository;
use dynamo_persistence_api::Persistence;
use dynamo_persistence_mongo::{MongoPersistence, MongoPersistenceConfig};
use dynamo_registry::ModuleRegistry;
use dynamo_repositories::{
    DeploymentSettingsRepository, GiveawaysRepository, GuildSettingsRepository, InviteRepository,
    MemberStatsRepository, ProviderStateRepository, SuggestionsRepository, WarningLogRepository,
};
use dynamo_runtime_api::{AppState, Error};
use dynamo_service_exchange::ExchangeRateService;
use dynamo_service_stock::StockQuoteService;
use dynamo_services_api::ServiceRegistry;
use dynamo_settings::{DeploymentSettings, GuildSettings};
use poise::serenity_prelude::{
    Context, CreateCommand, CreateInteractionResponse, CreateInteractionResponseMessage,
    EditInteractionResponse, FullEvent, Interaction,
};
use sha2::{Digest, Sha256};
use tracing::{error, info, warn};

const APPLICATION_COMMAND_FINGERPRINT_VERSION: &str = "application-command-v1";

pub async fn flush_all_voice_sessions(persistence: &Persistence) -> Result<(), Error> {
    if let Some(repo) = persistence.member_stats.as_deref() {
        dynamo_module_stats::events::flush_all_voice_sessions(repo).await?;
    }
    Ok(())
}

pub fn module_registry() -> ModuleRegistry {
    let modules: Vec<Box<dyn Module<AppState, Error>>> = vec![
        Box::new(dynamo_module_currency::CurrencyModule),
        Box::new(dynamo_module_giveaway::GiveawayModule),
        Box::new(dynamo_module_info::InfoModule),
        Box::new(dynamo_module_gameinfo::GameInfoModule),
        Box::new(dynamo_module_greeting::GreetingModule),
        Box::new(dynamo_module_invite::InviteModule),
        Box::new(dynamo_module_moderation::ModerationModule),
        Box::new(dynamo_module_suggestion::SuggestionModule),
        Box::new(dynamo_module_stats::StatsModule),
        Box::new(dynamo_module_ticket::TicketModule),
        Box::new(dynamo_module_stock::StockModule),
    ];

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
    let dashboard_audit_logs: Arc<dyn DashboardAuditLogRepository> = store.clone();
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
        Some(dashboard_audit_logs),
    ))
}

pub fn services_from_persistence(_persistence: &Persistence) -> anyhow::Result<ServiceRegistry> {
    let Some(toss_client) = toss_client_from_env()? else {
        return Ok(ServiceRegistry::new(None, None));
    };

    let stock_quotes: Arc<dyn StockQuoteService> =
        Arc::new(dynamo_provider_tossinvest::TossInvestStockQuoteService::new(toss_client.clone()));
    let exchange_rates: Arc<dyn ExchangeRateService> =
        Arc::new(dynamo_provider_tossinvest::TossInvestMarketDataService::new(toss_client));
    Ok(ServiceRegistry::new(
        Some(stock_quotes),
        Some(exchange_rates),
    ))
}

fn toss_client_from_env() -> anyhow::Result<Option<dynamo_provider_tossinvest::TossInvestClient>> {
    let config = match dynamo_provider_tossinvest::TossInvestConfig::from_env() {
        Ok(config) => config,
        Err(error) if is_missing_tossinvest_credential_error(&error) => return Ok(None),
        Err(error) => return Err(error.context("failed to load Toss Invest configuration")),
    };

    Ok(Some(dynamo_provider_tossinvest::TossInvestClient::new(
        config,
    )))
}

fn is_missing_tossinvest_credential_error(error: &anyhow::Error) -> bool {
    matches!(
        error.to_string().as_str(),
        "Missing required environment variable TOSSINVEST_CLIENT_ID"
            | "Missing required environment variable TOSSINVEST_CLIENT_SECRET"
    )
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

pub fn application_command_fingerprint_for_scope(
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> (String, usize) {
    let commands = create_application_commands_for_scope(deployment, guild);
    let count = commands.len();
    (application_command_fingerprint(&commands), count)
}

pub fn application_command_fingerprint(commands: &[CreateCommand]) -> String {
    let payload =
        serde_json::to_value(commands).expect("Discord application command payloads serialize");
    fingerprint_canonical_json_value(payload)
}

fn fingerprint_canonical_json_value(value: serde_json::Value) -> String {
    let canonical_value = canonicalize_json_value(value);
    let canonical_payload =
        serde_json::to_vec(&canonical_value).expect("canonical command JSON serializes");
    let digest = Sha256::digest(canonical_payload);

    format!(
        "{APPLICATION_COMMAND_FINGERPRINT_VERSION}:{}",
        hex_lower(&digest)
    )
}

fn canonicalize_json_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonicalize_json_value).collect())
        }
        serde_json::Value::Object(map) => {
            let mut entries = map.into_iter().collect::<Vec<_>>();
            entries.sort_by(|(left_key, _), (right_key, _)| left_key.cmp(right_key));

            let mut sorted = serde_json::Map::new();
            for (key, value) in entries {
                sorted.insert(key, canonicalize_json_value(value));
            }
            serde_json::Value::Object(sorted)
        }
        value => value,
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

fn is_handled_event(event: &FullEvent) -> bool {
    match event {
        FullEvent::CacheReady { .. }
        | FullEvent::GuildCreate { .. }
        | FullEvent::GuildDelete { .. }
        | FullEvent::GuildMemberAddition { .. }
        | FullEvent::GuildMemberRemoval { .. }
        | FullEvent::InviteCreate { .. }
        | FullEvent::InviteDelete { .. }
        | FullEvent::VoiceStateUpdate { .. }
        | FullEvent::InteractionCreate { .. } => true,
        FullEvent::Message { new_message } => {
            !new_message.author.bot && new_message.guild_id.is_some()
        }
        _ => false,
    }
}

fn event_guild_id(event: &FullEvent) -> Option<u64> {
    match event {
        FullEvent::GuildCreate { guild, .. } => Some(guild.id.get()),
        FullEvent::GuildDelete { incomplete, .. } => Some(incomplete.id.get()),
        FullEvent::GuildMemberAddition { new_member } => Some(new_member.guild_id.get()),
        FullEvent::GuildMemberRemoval { guild_id, .. } => Some(guild_id.get()),
        FullEvent::InviteCreate { data } => data.guild_id.map(|id| id.get()),
        FullEvent::InviteDelete { data } => data.guild_id.map(|id| id.get()),
        FullEvent::Message { new_message } => new_message.guild_id.map(|id| id.get()),
        FullEvent::VoiceStateUpdate { old, new } => new
            .guild_id
            .or_else(|| old.as_ref().and_then(|state| state.guild_id))
            .map(|id| id.get()),
        FullEvent::InteractionCreate { interaction } => interaction_guild_id(interaction),
        _ => None,
    }
}

fn interaction_guild_id(interaction: &Interaction) -> Option<u64> {
    match interaction {
        Interaction::Command(cmd) | Interaction::Autocomplete(cmd) => {
            cmd.guild_id.map(|id| id.get())
        }
        Interaction::Component(component) => component.guild_id.map(|id| id.get()),
        Interaction::Modal(modal) => modal.guild_id.map(|id| id.get()),
        _ => None,
    }
}

fn interaction_locale(interaction: &Interaction) -> Option<&str> {
    match interaction {
        Interaction::Command(cmd) | Interaction::Autocomplete(cmd) => Some(cmd.locale.as_str()),
        Interaction::Component(component) => Some(component.locale.as_str()),
        Interaction::Modal(modal) => Some(modal.locale.as_str()),
        _ => None,
    }
}

async fn send_blocked_interaction_notice(ctx: &Context, interaction: &Interaction) {
    let text = if dynamo_runtime_api::is_korean_locale(interaction_locale(interaction)) {
        "이 서버에서는 현재 Dynamo 사용이 제한되어 있습니다."
    } else {
        "Dynamo is disabled in this server."
    };
    let response = CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .content(text)
            .ephemeral(true),
    );
    match interaction {
        Interaction::Component(component) => {
            let _ = component.create_response(ctx, response).await;
        }
        Interaction::Modal(modal) => {
            let _ = modal.create_response(ctx, response).await;
        }
        _ => {}
    }
}

async fn send_interaction_error_fallback(ctx: &Context, interaction: &Interaction) {
    let text = if dynamo_runtime_api::is_korean_locale(interaction_locale(interaction)) {
        "상호작용을 처리하는 중 내부 오류가 발생했습니다."
    } else {
        "An internal error occurred while processing this interaction."
    };
    if let Interaction::Component(component) = interaction {
        if component
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content(text)
                        .ephemeral(true),
                ),
            )
            .await
            .is_err()
        {
            let _ = component
                .edit_response(ctx, EditInteractionResponse::new().content(text))
                .await;
        }
    } else if let Interaction::Modal(modal) = interaction
        && modal
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content(text)
                        .ephemeral(true),
                ),
            )
            .await
            .is_err()
    {
        let _ = modal
            .edit_response(ctx, EditInteractionResponse::new().content(text))
            .await;
    }
}

async fn dispatch_interaction_handlers(
    ctx: &Context,
    interaction: &Interaction,
    data: &AppState,
) -> Result<(), Error> {
    if dynamo_module_stock::interactions::handle(ctx, interaction, data).await? {
        return Ok(());
    }
    if dynamo_module_suggestion::interactions::handle(ctx, interaction, data).await? {
        return Ok(());
    }
    if dynamo_module_ticket::interactions::handle(ctx, interaction, data).await? {
        return Ok(());
    }
    if dynamo_module_giveaway::interactions::handle(ctx, interaction, data).await? {
        return Ok(());
    }
    dynamo_module_stats::events::handle_interaction(ctx, data, interaction).await?;
    Ok(())
}

pub async fn handle_framework_event(
    ctx: &Context,
    event: &FullEvent,
    data: &AppState,
) -> Result<(), Error> {
    if !is_handled_event(event) {
        return Ok(());
    }

    let deployment = data.deployment_settings_cached().await?;
    if let Some(guild_id) = event_guild_id(event)
        && deployment.blocked_guild_ids.contains(&guild_id)
    {
        if let FullEvent::InteractionCreate { interaction } = event {
            send_blocked_interaction_notice(ctx, interaction).await;
        }
        return Ok(());
    }

    match event {
        FullEvent::CacheReady { guilds } => {
            for guild_id in guilds {
                if deployment.blocked_guild_ids.contains(&guild_id.get()) {
                    continue;
                }
                if let Err(error) =
                    dynamo_module_invite::events::preload_guild_cache(ctx, data, *guild_id).await
                {
                    warn!(
                        guild_id = guild_id.get(),
                        ?error,
                        "failed to preload invite cache on cache ready"
                    );
                }
                if let Err(error) = dynamo_module_stats::events::seed_voice_sessions_from_cache(
                    ctx, data, *guild_id,
                )
                .await
                {
                    warn!(
                        guild_id = guild_id.get(),
                        ?error,
                        "failed to seed voice sessions on cache ready"
                    );
                }
            }
        }
        FullEvent::GuildCreate { guild, is_new } => {
            if is_new.unwrap_or(true)
                && let Err(error) =
                    dynamo_module_invite::events::preload_guild_cache(ctx, data, guild.id).await
            {
                warn!(
                    guild_id = guild.id.get(),
                    ?error,
                    "failed to preload invite cache on guild create"
                );
            }
            if let Err(error) =
                dynamo_module_stats::events::seed_voice_sessions_from_cache(ctx, data, guild.id)
                    .await
            {
                warn!(
                    guild_id = guild.id.get(),
                    ?error,
                    "failed to seed voice sessions on guild create"
                );
            }
        }
        FullEvent::GuildDelete { incomplete, .. } => {
            if !incomplete.unavailable {
                dynamo_module_invite::events::evict_guild_cache(incomplete.id).await;
                data.invalidate_guild_settings_cache(incomplete.id.get())
                    .await;
            }
        }
        FullEvent::GuildMemberAddition { new_member } => {
            let inviter_data = match dynamo_module_invite::events::track_joined_member(
                ctx, data, new_member,
            )
            .await
            {
                Ok(record) => record,
                Err(error) => {
                    warn!(
                        guild_id = new_member.guild_id.get(),
                        user_id = new_member.user.id.get(),
                        ?error,
                        "failed to track invite on member join"
                    );
                    None
                }
            };
            dynamo_module_greeting::events::send_welcome(
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
                match dynamo_module_invite::events::track_left_member(ctx, data, *guild_id, user)
                    .await
                {
                    Ok(record) => record,
                    Err(error) => {
                        warn!(
                            guild_id = guild_id.get(),
                            user_id = user.id.get(),
                            ?error,
                            "failed to track invite on member removal"
                        );
                        None
                    }
                };
            dynamo_module_greeting::events::send_farewell(
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
            dynamo_module_invite::events::handle_invite_create(ctx, data, invite).await?;
        }
        FullEvent::InviteDelete { data: invite } => {
            dynamo_module_invite::events::handle_invite_delete(ctx, data, invite).await?;
        }
        FullEvent::Message { new_message } => {
            dynamo_module_stats::events::handle_message(ctx, data, new_message).await?;
        }
        FullEvent::VoiceStateUpdate { old, new } => {
            dynamo_module_stats::events::handle_voice_state_update(ctx, data, old.as_ref(), new)
                .await?;
        }
        FullEvent::InteractionCreate { interaction } => {
            if let Err(error) = dispatch_interaction_handlers(ctx, interaction, data).await {
                error!(?error, "interaction handler failed");
                send_interaction_error_fallback(ctx, interaction).await;
            }
        }
        _ => {}
    }

    Ok(())
}

fn korean_command_description(description: &str) -> Option<&'static str> {
    Some(match description {
        "Convert one currency amount into another currency." => "금액을 다른 통화로 환산합니다.",
        "Show the configured exchange-rate board for one base currency." => {
            "기준 통화 하나에 대해 설정된 환율표를 보여줍니다."
        }
        "Manage giveaway lifecycle actions for this guild." => {
            "이 서버의 경품 추첨 진행 상태를 관리합니다."
        }
        "Start a new giveaway message in the configured channel." => {
            "설정된 채널에서 새 경품 추첨을 시작합니다."
        }
        "List the giveaways currently tracked in this guild." => {
            "이 서버에서 진행 중인 경품 추첨 목록을 보여줍니다."
        }
        "End an active giveaway immediately." => "진행 중인 경품 추첨을 즉시 종료합니다.",
        "Pause an active giveaway so no new entries can be accepted." => {
            "진행 중인 경품 추첨을 일시 정지해 새 참가를 막습니다."
        }
        "Resume a paused giveaway and extend its end time by the paused duration." => {
            "일시 정지된 경품 추첨을 재개하고, 정지했던 시간만큼 종료 시각을 늦춥니다."
        }
        "Pick a fresh set of winners for an already ended giveaway." => {
            "이미 종료된 경품 추첨의 당첨자를 다시 뽑습니다."
        }
        "Edit the schedule, prize, or winner count of an active giveaway." => {
            "진행 중인 경품 추첨의 일정, 상품, 당첨자 수를 수정합니다."
        }
        "Check gateway and REST response latency." => {
            "게이트웨이 및 REST 응답 지연 시간을 확인합니다."
        }
        "Show runtime details such as version, loaded modules, uptime, and persistence status." => {
            "버전, 불러온 모듈, 가동 시간, 저장소 상태 등 실행 정보를 보여줍니다."
        }
        "List commands currently enabled for this server." => {
            "현재 이 서버에서 활성화된 명령어 목록을 보여줍니다."
        }
        "Show the War Thunder and World of Tanks referral panel for this guild." => {
            "이 서버의 워썬더·월드 오브 탱크 추천 패널을 보여줍니다."
        }
        "Show the latest known FFXIV global maintenance window." => {
            "가장 최근에 확인된 FFXIV 글로벌 서버 점검 일정을 보여줍니다."
        }
        "Show the latest known Producer Letter Live schedule." => {
            "가장 최근에 확인된 프로듀서 레터 라이브 일정을 보여줍니다."
        }
        "Manage greeting previews for the configured welcome and farewell templates." => {
            "설정된 입장·퇴장 인사 템플릿의 미리보기를 관리합니다."
        }
        "Preview a welcome or farewell message using the current guild settings." => {
            "현재 서버 설정으로 입장·퇴장 메시지를 미리 봅니다."
        }
        "View tracked invite statistics for yourself or another member." => {
            "본인 또는 다른 멤버의 초대 통계를 확인합니다."
        }
        "Show the top inviters in this server." => "이 서버의 상위 초대자 순위를 보여줍니다.",
        "Manage stored invite counts for server members." => {
            "서버 멤버의 초대 횟수 기록을 관리합니다."
        }
        "Add bonus invites to a member." => "멤버에게 보너스 초대 횟수를 추가합니다.",
        "Deduct invites from a member." => "멤버의 초대 횟수를 차감합니다.",
        "Reset all tracked invite counters for a member." => {
            "멤버의 모든 초대 집계 기록을 초기화합니다."
        }
        "Issue a warning to a guild member." => "멤버에게 경고를 부여합니다.",
        "Manage the stored warning history for a guild member." => {
            "서버 멤버의 경고 기록을 관리합니다."
        }
        "List the warnings currently stored for a guild member." => {
            "멤버에게 저장된 경고 목록을 보여줍니다."
        }
        "Clear all stored warnings for a guild member." => "멤버의 경고를 모두 삭제합니다.",
        "Timeout a guild member for a specific duration." => {
            "멤버를 지정한 시간 동안 타임아웃합니다."
        }
        "Remove an active timeout from a guild member." => "멤버의 타임아웃을 해제합니다.",
        "Kick a guild member from the server." => "멤버를 서버에서 추방합니다.",
        "Ban a user from the server." => "사용자를 서버에서 차단합니다.",
        "Unban a user by their Discord user ID." => "Discord 사용자 ID로 차단을 해제합니다.",
        "Ban and immediately unban a user to remove recent messages." => {
            "사용자를 차단했다가 바로 해제해 최근 메시지를 삭제합니다."
        }
        "Change or clear a guild member's nickname." => "멤버의 별명을 바꾸거나 지웁니다.",
        "Submit a new suggestion to the guild suggestion board." => {
            "서버 건의 게시판에 새 건의를 올립니다."
        }
        "View level, XP, message count, and voice activity for a server member." => {
            "서버 멤버의 레벨, 경험치, 메시지 수, 음성 활동 시간을 확인합니다."
        }
        "View the top members in this server ranked by XP, messages, or voice time." => {
            "경험치, 메시지 수, 음성 활동 시간 기준 상위 멤버 순위를 보여줍니다."
        }
        "Manage the guild's ticket panel, open tickets, and ticket access controls." => {
            "서버의 티켓 패널, 열린 티켓 및 접근 권한을 관리합니다."
        }
        "Post or refresh the ticket creation panel in a target channel." => {
            "지정한 채널에 티켓 생성 패널을 올리거나 새로 고칩니다."
        }
        "Set or clear the channel that receives ticket transcripts and close logs." => {
            "티켓 대화 기록과 종료 로그를 받을 채널을 지정하거나 해제합니다."
        }
        "Set the maximum number of open ticket channels for this guild." => {
            "이 서버에서 동시에 열 수 있는 티켓 채널 수를 정합니다."
        }
        "Close the current ticket channel and archive its transcript." => {
            "현재 티켓 채널을 닫고 대화 기록을 보관합니다."
        }
        "Close every open ticket channel in the current guild." => {
            "이 서버의 열린 티켓 채널을 모두 닫습니다."
        }
        "Grant a user or role access to the current ticket channel." => {
            "현재 티켓 채널에 사용자나 역할의 접근을 허용합니다."
        }
        "Remove a user or role from the current ticket channel." => {
            "현재 티켓 채널에서 사용자나 역할을 제거합니다."
        }
        "Show a quote for one stock symbol and keep it refreshable for a short time." => {
            "종목 하나의 시세를 보여주고 잠시 동안 자동으로 갱신합니다."
        }
        "Show the configured ETF watchlist for this guild." => {
            "이 서버에 설정된 ETF 관심 종목을 보여줍니다."
        }
        _ => return None,
    })
}

fn localize_command_recursive(command: &mut DiscordCommand<AppState, Error>) {
    if let Some(ko) = command
        .description
        .as_deref()
        .map(str::trim)
        .and_then(korean_command_description)
    {
        command
            .description_localizations
            .entry("ko".to_string())
            .or_insert_with(|| ko.to_string());
    }
    for sub in &mut command.subcommands {
        localize_command_recursive(sub);
    }
}

fn filter_commands_for_scope(
    commands: Vec<DiscordCommand<AppState, Error>>,
    module_catalog: &ModuleCatalog,
    command_catalog: &CommandCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> Vec<DiscordCommand<AppState, Error>> {
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
            .map(|mut command| {
                localize_command_recursive(&mut command);
                command
            })
        })
        .collect()
}

fn filter_command_recursive(
    mut command: DiscordCommand<AppState, Error>,
    path: &mut Vec<String>,
    module_catalog: &ModuleCatalog,
    command_catalog: &CommandCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> Option<DiscordCommand<AppState, Error>> {
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

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, sync::Mutex};

    use dynamo_persistence_api::Persistence;
    use dynamo_service_exchange::ExchangeRateService;

    use super::{
        application_command_fingerprint_for_scope, create_application_commands_for_scope,
        fingerprint_canonical_json_value, module_registry, services_from_persistence,
        toss_client_from_env,
    };

    static TOSS_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct ScopedEnvVar {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: tests that mutate Toss environment variables hold TOSS_ENV_LOCK,
            // and ScopedEnvVar restores the original value before releasing the lock.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: tests that mutate Toss environment variables hold TOSS_ENV_LOCK,
            // and ScopedEnvVar restores the original value before releasing the lock.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            // SAFETY: ScopedEnvVar values are dropped while TOSS_ENV_LOCK is still held,
            // so environment mutation stays serialized for the full test scope.
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn default_registry_commands_have_explicit_descriptions() {
        let registry = module_registry();
        for entry in &registry.command_catalog().entries {
            let description = entry
                .command
                .description
                .as_deref()
                .expect("command description");
            assert!(
                !description.starts_with("Command /"),
                "command `{}` still uses fallback description: {description}",
                entry.command.id
            );
        }
    }

    #[test]
    fn module_hook_namespaces_are_exposed() {
        let _ = dynamo_module_stock::interactions::handle;
        let _ = dynamo_module_ticket::interactions::handle;
        let _ = dynamo_module_suggestion::interactions::handle;
        let _ = dynamo_module_giveaway::interactions::handle;

        let _ = dynamo_module_invite::events::preload_guild_cache;
        let _ = dynamo_module_invite::events::handle_invite_create;
        let _ = dynamo_module_invite::events::handle_invite_delete;
        let _ = dynamo_module_invite::events::track_joined_member;
        let _ = dynamo_module_invite::events::track_left_member;

        let _ = dynamo_module_greeting::events::send_welcome;
        let _ = dynamo_module_greeting::events::send_farewell;
        let _ = dynamo_module_stats::events::handle_message;
        let _ = dynamo_module_stats::events::handle_interaction;
        let _ = dynamo_module_stats::events::handle_voice_state_update;
    }

    #[test]
    fn services_keep_market_data_optional_without_toss_credentials() {
        let _lock = TOSS_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _client_id = ScopedEnvVar::remove("TOSSINVEST_CLIENT_ID");
        let _client_secret = ScopedEnvVar::remove("TOSSINVEST_CLIENT_SECRET");
        let _base_url = ScopedEnvVar::remove("TOSSINVEST_BASE_URL");

        let services = services_from_persistence(&Persistence::default()).unwrap();

        assert!(services.stock_quotes.is_none());
        assert!(services.exchange_rates.is_none());
    }

    #[test]
    fn services_build_toss_market_data_when_credentials_are_present() {
        let _lock = TOSS_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _client_id = ScopedEnvVar::set("TOSSINVEST_CLIENT_ID", "test-client-id");
        let _client_secret = ScopedEnvVar::set("TOSSINVEST_CLIENT_SECRET", "test-client-secret");
        let _base_url = ScopedEnvVar::set("TOSSINVEST_BASE_URL", "https://openapi.tossinvest.com");

        let client = toss_client_from_env().unwrap().expect("Toss client");
        let service = dynamo_provider_tossinvest::TossInvestMarketDataService::new(client);

        assert_eq!(service.cache_target_count(), 2);
        assert!(!service.uses_persisted_cache());

        let services = services_from_persistence(&Persistence::default()).unwrap();
        assert!(services.stock_quotes.is_some());
        assert!(services.exchange_rates.is_some());
    }

    #[test]
    fn services_reject_invalid_toss_configuration_when_credentials_are_present() {
        let _lock = TOSS_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _client_id = ScopedEnvVar::set("TOSSINVEST_CLIENT_ID", "test-client-id");
        let _client_secret = ScopedEnvVar::set("TOSSINVEST_CLIENT_SECRET", "test-client-secret");
        let _base_url = ScopedEnvVar::set("TOSSINVEST_BASE_URL", "http://not-allowed.example");

        let error = match services_from_persistence(&Persistence::default()) {
            Ok(_) => panic!("invalid Toss configuration should fail"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("failed to load Toss Invest configuration")
        );
    }

    #[test]
    fn application_command_fingerprint_has_version_prefix() {
        let deployment = Default::default();

        let (fingerprint, command_count) =
            application_command_fingerprint_for_scope(&deployment, None);

        assert!(command_count > 0);
        assert!(
            fingerprint.starts_with("application-command-v1:"),
            "unexpected fingerprint prefix: {fingerprint}"
        );
    }

    #[test]
    fn application_command_fingerprint_is_stable_across_json_object_key_order() {
        let first = serde_json::json!([
            {
                "name": "quote",
                "description": "Lookup a quote",
                "name_localizations": {
                    "fr": "Cours",
                    "en-US": "Quote"
                }
            }
        ]);
        let second = serde_json::json!([
            {
                "name_localizations": {
                    "en-US": "Quote",
                    "fr": "Cours"
                },
                "description": "Lookup a quote",
                "name": "quote"
            }
        ]);

        assert_eq!(
            fingerprint_canonical_json_value(first),
            fingerprint_canonical_json_value(second)
        );
    }

    #[test]
    fn every_application_command_and_subcommand_has_korean_description_localization() {
        fn assert_has_ko_description(node: &serde_json::Value, path: &str) {
            let ko = node
                .get("description_localizations")
                .and_then(|map| map.get("ko"))
                .and_then(serde_json::Value::as_str);
            assert!(
                ko.is_some_and(|text| !text.trim().is_empty()),
                "missing Korean description_localizations for command `{path}`: {node}"
            );
            if let Some(options) = node.get("options").and_then(serde_json::Value::as_array) {
                for opt in options {
                    if matches!(
                        opt.get("type").and_then(serde_json::Value::as_u64),
                        Some(1 | 2)
                    ) {
                        let sub_name = opt
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("<unnamed>");
                        let sub_path = format!("{path} {sub_name}");
                        assert_has_ko_description(opt, &sub_path);
                    }
                }
            }
        }

        let commands = create_application_commands_for_scope(
            &dynamo_settings::DeploymentSettings::default(),
            None,
        );
        assert!(!commands.is_empty());
        let payload = serde_json::to_value(&commands).expect("commands serialize");
        let list = payload.as_array().expect("command array");
        for command in list {
            let name = command
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<unnamed>");
            assert_has_ko_description(command, name);
        }
    }
}
