use chrono::{Duration as ChronoDuration, Utc};
use dynamo_core::{
    AppState, Context, DiscordCommand, Error, GatewayIntents, GiveawayRecord, GiveawayStatus,
    Module, ModuleCategory, ModuleManifest, SettingsField, SettingsFieldKind, SettingsSchema,
    SettingsSection, module_access_for_app,
};
use poise::serenity_prelude::{
    ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow, CreateButton, CreateEmbed,
    CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage,
    EditInteractionResponse, EditMessage, Interaction, Timestamp, User,
};
use rand::{seq::SliceRandom, thread_rng};
use serde::{Deserialize, Serialize};

const MODULE_ID: &str = "giveaway";
const GIVEAWAY_ENTER_BUTTON_ID: &str = "giveaway_enter";
const DEFAULT_BUTTON_LABEL: &str = "Enter Giveaway";
const ACTIVE_COLOR: u32 = 0xF1C40F;
const PAUSED_COLOR: u32 = 0xE67E22;
const ENDED_COLOR: u32 = 0x2ECC71;

pub struct GiveawayModule;

impl Module for GiveawayModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Giveaway",
            "Optional first-party giveaway workflow with persisted entry tracking.",
            ModuleCategory::Giveaway,
            false,
            GatewayIntents::GUILDS | GatewayIntents::GUILD_MEMBERS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        vec![giveaway()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema {
            sections: vec![SettingsSection {
                id: "giveaway",
                title: "Giveaway",
                description: Some("Optional giveaway module configuration."),
                fields: vec![
                    SettingsField {
                        key: "default_channel",
                        label: "Default channel ID",
                        help_text: Some("Primary channel for giveaway announcements."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                    SettingsField {
                        key: "button_label",
                        label: "Entry button label",
                        help_text: Some("Button text shown on giveaway messages."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                ],
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct GiveawaySettings {
    #[serde(alias = "default_channel_id", deserialize_with = "deserialize_optional_snowflake")]
    default_channel: Option<u64>,
    button_label: String,
}

impl Default for GiveawaySettings {
    fn default() -> Self {
        Self {
            default_channel: None,
            button_label: DEFAULT_BUTTON_LABEL.to_string(),
        }
    }
}

#[poise::command(
    slash_command,
    guild_only,
    category = "Giveaway",
    subcommands(
        "giveaway_start",
        "giveaway_list",
        "giveaway_end",
        "giveaway_pause",
        "giveaway_resume",
        "giveaway_reroll",
        "giveaway_edit"
    ),
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    rename = "start",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_start(
    ctx: Context<'_>,
    #[description = "Duration like 30m, 1h, or 2d"] duration: String,
    #[description = "Prize shown in the giveaway message"] prize: String,
    #[description = "Number of winners"] winners: i32,
    #[description = "Optional giveaway channel; omit to use module settings"] channel: Option<ChannelId>,
    #[description = "Optional host user override"] host: Option<User>,
    #[description = "Optional comma-separated role IDs allowed to enter"] allowed_roles: Option<String>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.say("Giveaway repository is not configured.").await?;
        return Ok(());
    };

    let settings = load_settings(ctx.data(), guild_id.get()).await?;
    let target_channel = channel
        .map(|value| value.get())
        .or(settings.default_channel)
        .ok_or_else(|| anyhow::anyhow!("Giveaway channel not configured."))?;
    let winner_count = winners.max(1) as u64;
    let duration = humantime::parse_duration(&duration)
        .map_err(|error| anyhow::anyhow!("Invalid duration: {error}"))?;
    let allowed_role_ids = parse_role_ids(allowed_roles.as_deref())?;

    let start_at = Utc::now();
    let ends_at = start_at + ChronoDuration::from_std(duration)?;
    let host_user_id = host.unwrap_or_else(|| ctx.author().clone()).id.get();
    let button_label = if settings.button_label.trim().is_empty() {
        DEFAULT_BUTTON_LABEL.to_string()
    } else {
        settings.button_label.clone()
    };

    let draft = GiveawayRecord {
        guild_id: guild_id.get(),
        channel_id: target_channel,
        message_id: 0,
        prize: prize.clone(),
        winner_count,
        host_user_id,
        allowed_role_ids,
        entries: Vec::new(),
        winner_ids: Vec::new(),
        status: GiveawayStatus::Active,
        started_at: start_at,
        ends_at,
        paused_at: None,
        button_label: button_label.clone(),
        created_at: start_at,
        updated_at: start_at,
    };

    let message = ChannelId::new(target_channel)
        .send_message(
            ctx.serenity_context(),
            CreateMessage::new()
                .embed(build_embed(&draft))
                .components(vec![entry_button(&draft)]),
        )
        .await?;

    let mut record = draft;
    record.message_id = message.id.get();
    repo.create(record).await?;

    ctx.say(format!(
        "Giveaway started in <#{}> with message `{}`.",
        target_channel,
        message.id.get()
    ))
    .await?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    rename = "list",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_list(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.say("Giveaway repository is not configured.").await?;
        return Ok(());
    };

    let mut giveaways = repo.list_by_guild(guild_id.get()).await?;
    giveaways.sort_by_key(|record| std::cmp::Reverse(record.created_at.timestamp()));

    if giveaways.is_empty() {
        ctx.say("There are no giveaways tracked in this guild.")
            .await?;
        return Ok(());
    }

    let description = giveaways
        .into_iter()
        .take(10)
        .map(|record| {
            format!(
                "`{}` {} in <#{}> | entries: {} | status: {:?}",
                record.message_id,
                record.prize,
                record.channel_id,
                record.entries.len(),
                record.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let embed = CreateEmbed::new()
        .title("Tracked Giveaways")
        .description(description);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    rename = "end",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_end(
    ctx: Context<'_>,
    #[description = "Giveaway message ID"] message_id: String,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.say("Unable to find that giveaway.").await?;
        return Ok(());
    };

    if record.status == GiveawayStatus::Ended {
        ctx.say("That giveaway has already ended.").await?;
        return Ok(());
    }

    finalize_giveaway(ctx.serenity_context(), ctx.data(), &mut record).await?;
    ctx.say("The giveaway has been ended.").await?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    rename = "pause",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_pause(
    ctx: Context<'_>,
    #[description = "Giveaway message ID"] message_id: String,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.say("Giveaway repository is not configured.").await?;
        return Ok(());
    };
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.say("Unable to find that giveaway.").await?;
        return Ok(());
    };

    if record.status != GiveawayStatus::Active {
        ctx.say("Only active giveaways can be paused.").await?;
        return Ok(());
    }

    record.status = GiveawayStatus::Paused;
    record.paused_at = Some(Utc::now());
    record.updated_at = Utc::now();
    let record = repo.save(record).await?;
    sync_message(ctx.serenity_context().http.as_ref(), &record).await?;
    ctx.say("The giveaway has been paused.").await?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    rename = "resume",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_resume(
    ctx: Context<'_>,
    #[description = "Giveaway message ID"] message_id: String,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.say("Giveaway repository is not configured.").await?;
        return Ok(());
    };
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.say("Unable to find that giveaway.").await?;
        return Ok(());
    };

    if record.status != GiveawayStatus::Paused {
        ctx.say("Only paused giveaways can be resumed.").await?;
        return Ok(());
    }

    let paused_at = record.paused_at.unwrap_or(record.updated_at);
    let pause_duration = Utc::now() - paused_at;
    record.ends_at += pause_duration;
    record.status = GiveawayStatus::Active;
    record.paused_at = None;
    record.updated_at = Utc::now();
    let record = repo.save(record).await?;
    sync_message(ctx.serenity_context().http.as_ref(), &record).await?;
    ctx.say("The giveaway has been resumed.").await?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    rename = "reroll",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_reroll(
    ctx: Context<'_>,
    #[description = "Giveaway message ID"] message_id: String,
    #[description = "Optional override winner count"] winners: Option<i32>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.say("Giveaway repository is not configured.").await?;
        return Ok(());
    };
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.say("Unable to find that giveaway.").await?;
        return Ok(());
    };

    if record.status != GiveawayStatus::Ended {
        ctx.say("Only ended giveaways can be rerolled.").await?;
        return Ok(());
    }

    if let Some(winner_count) = winners {
        record.winner_count = winner_count.max(1) as u64;
    }
    record.winner_ids = choose_winners(&record.entries, record.winner_count);
    record.updated_at = Utc::now();
    let record = repo.save(record).await?;
    sync_message(ctx.serenity_context().http.as_ref(), &record).await?;

    let winners_text = format_winners(&record.winner_ids);
    ctx.say(format!("Giveaway rerolled. Winners: {winners_text}"))
        .await?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    rename = "edit",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_edit(
    ctx: Context<'_>,
    #[description = "Giveaway message ID"] message_id: String,
    #[description = "Optional minutes to add"] add_minutes: Option<i64>,
    #[description = "Optional new prize"] new_prize: Option<String>,
    #[description = "Optional new winner count"] new_winners: Option<i32>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.say("Giveaway repository is not configured.").await?;
        return Ok(());
    };
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.say("Unable to find that giveaway.").await?;
        return Ok(());
    };

    if record.status == GiveawayStatus::Ended {
        ctx.say("Ended giveaways cannot be edited.").await?;
        return Ok(());
    }

    if let Some(minutes) = add_minutes {
        record.ends_at += ChronoDuration::minutes(minutes);
    }
    if let Some(prize) = new_prize {
        record.prize = prize;
    }
    if let Some(winners) = new_winners {
        record.winner_count = winners.max(1) as u64;
    }
    record.updated_at = Utc::now();
    let record = repo.save(record).await?;
    sync_message(ctx.serenity_context().http.as_ref(), &record).await?;
    ctx.say("Giveaway updated.").await?;
    Ok(())
}

pub async fn handle_interaction(
    ctx: &poise::serenity_prelude::Context,
    interaction: &Interaction,
    data: &AppState,
) -> Result<bool, Error> {
    match interaction {
        Interaction::Component(component) if component.data.custom_id == GIVEAWAY_ENTER_BUTTON_ID => {
            handle_entry_interaction(ctx, component, data).await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub async fn poll_due_giveaways(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
) -> Result<(), Error> {
    let Some(repo) = data.persistence.giveaways.clone() else {
        return Ok(());
    };
    let due = repo.list_due_before(Utc::now()).await?;
    for mut record in due {
        finalize_giveaway(ctx, data, &mut record).await?;
    }
    Ok(())
}

async fn handle_entry_interaction(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
    data: &AppState,
) -> Result<(), Error> {
    let Some(guild_id) = component.guild_id else {
        return Ok(());
    };
    if module_access_for_app(data, MODULE_ID, Some(guild_id.get()))
        .await?
        .denial_reason
        .is_some()
    {
        component
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("The giveaway module is currently disabled.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    let Some(repo) = data.persistence.giveaways.clone() else {
        return Ok(());
    };
    let Some(mut record) = repo
        .get_by_message(guild_id.get(), component.message.id.get())
        .await?
    else {
        return Ok(());
    };

    if record.status != GiveawayStatus::Active {
        component
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("This giveaway is no longer accepting entries.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    if !record.allowed_role_ids.is_empty() {
        let member_roles = component
            .member
            .as_ref()
            .map(|member| member.roles.clone())
            .unwrap_or_default();
        let allowed = record
            .allowed_role_ids
            .iter()
            .any(|role_id| member_roles.contains(&poise::serenity_prelude::RoleId::new(*role_id)));
        if !allowed {
            component
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("You do not meet the role requirements for this giveaway.")
                            .ephemeral(true),
                    ),
                )
                .await?;
            return Ok(());
        }
    }

    component.defer_ephemeral(ctx).await?;
    let user_id = component.user.id.get();
    let response = if let Some(position) = record.entries.iter().position(|entry| *entry == user_id) {
        record.entries.remove(position);
        "You left the giveaway."
    } else {
        record.entries.push(user_id);
        "You entered the giveaway."
    };
    record.updated_at = Utc::now();
    let record = repo.save(record).await?;
    sync_message(ctx.http.as_ref(), &record).await?;
    component
        .edit_response(ctx, EditInteractionResponse::new().content(response))
        .await?;
    Ok(())
}

async fn load_settings(data: &AppState, guild_id: u64) -> Result<GiveawaySettings, Error> {
    let guild_settings = data.persistence.guild_settings_or_default(guild_id).await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(|module| serde_json::from_value::<GiveawaySettings>(module.configuration.clone()))
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
}

async fn load_giveaway(
    data: &AppState,
    guild_id: u64,
    message_id: &str,
) -> Result<Option<GiveawayRecord>, Error> {
    let Some(repo) = data.persistence.giveaways.clone() else {
        return Ok(None);
    };
    let message_id = parse_message_id(message_id)?;
    repo.get_by_message(guild_id, message_id).await
}

async fn finalize_giveaway(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    record: &mut GiveawayRecord,
) -> Result<(), Error> {
    if record.status == GiveawayStatus::Ended {
        return Ok(());
    }

    let Some(repo) = data.persistence.giveaways.clone() else {
        return Ok(());
    };

    record.status = GiveawayStatus::Ended;
    record.paused_at = None;
    record.winner_ids = choose_winners(&record.entries, record.winner_count);
    record.updated_at = Utc::now();
    let record = repo.save(record.clone()).await?;
    sync_message(ctx.http.as_ref(), &record).await?;

    let message = if record.winner_ids.is_empty() {
        format!("Giveaway `{}` ended with no eligible winners.", record.prize)
    } else {
        format!(
            "Giveaway `{}` ended. Winners: {}",
            record.prize,
            format_winners(&record.winner_ids)
        )
    };
    let _ = ChannelId::new(record.channel_id)
        .send_message(ctx, CreateMessage::new().content(message))
        .await;
    Ok(())
}

async fn sync_message(
    http: &poise::serenity_prelude::Http,
    record: &GiveawayRecord,
) -> Result<(), Error> {
    ChannelId::new(record.channel_id)
        .edit_message(
            http,
            record.message_id,
            EditMessage::new()
                .embed(build_embed(record))
                .components(vec![entry_button(record)]),
        )
        .await?;
    Ok(())
}

fn build_embed(record: &GiveawayRecord) -> CreateEmbed {
    let mut embed = CreateEmbed::new()
        .title(match record.status {
            GiveawayStatus::Active => "Giveaway",
            GiveawayStatus::Paused => "Giveaway Paused",
            GiveawayStatus::Ended => "Giveaway Ended",
        })
        .description(record.prize.clone())
        .field("Winners", record.winner_count.to_string(), true)
        .field("Entries", record.entries.len().to_string(), true)
        .field("Hosted by", format!("<@{}>", record.host_user_id), true)
        .color(match record.status {
            GiveawayStatus::Active => ACTIVE_COLOR,
            GiveawayStatus::Paused => PAUSED_COLOR,
            GiveawayStatus::Ended => ENDED_COLOR,
        })
        .footer(CreateEmbedFooter::new(match record.status {
            GiveawayStatus::Active => "Press the button below to enter or leave.",
            GiveawayStatus::Paused => "Entries are paused.",
            GiveawayStatus::Ended => "This giveaway has concluded.",
        }));

    if let Ok(timestamp) = Timestamp::from_unix_timestamp(record.ends_at.timestamp()) {
        embed = embed.timestamp(timestamp);
    }

    if !record.allowed_role_ids.is_empty() {
        let roles = record
            .allowed_role_ids
            .iter()
            .map(|role_id| format!("<@&{}>", role_id))
            .collect::<Vec<_>>()
            .join(", ");
        embed = embed.field("Allowed Roles", roles, false);
    }

    if record.status == GiveawayStatus::Ended {
        embed = embed.field("Winners", format_winners(&record.winner_ids), false);
    }

    embed
}

fn entry_button(record: &GiveawayRecord) -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(GIVEAWAY_ENTER_BUTTON_ID)
            .label(if record.button_label.trim().is_empty() {
                DEFAULT_BUTTON_LABEL.to_string()
            } else {
                record.button_label.clone()
            })
            .style(ButtonStyle::Success)
            .disabled(record.status != GiveawayStatus::Active),
    ])
}

fn choose_winners(entries: &[u64], winner_count: u64) -> Vec<u64> {
    let mut pool = entries.to_vec();
    pool.sort_unstable();
    pool.dedup();
    pool.shuffle(&mut thread_rng());
    pool.truncate(winner_count as usize);
    pool
}

fn format_winners(winner_ids: &[u64]) -> String {
    if winner_ids.is_empty() {
        "No winners".to_string()
    } else {
        winner_ids
            .iter()
            .map(|winner_id| format!("<@{}>", winner_id))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn parse_message_id(value: &str) -> Result<u64, Error> {
    value
        .trim()
        .parse::<u64>()
        .map_err(|error| anyhow::anyhow!("Invalid message id `{value}`: {error}"))
}

fn parse_role_ids(value: Option<&str>) -> Result<Vec<u64>, Error> {
    match value {
        None => Ok(Vec::new()),
        Some(value) if value.trim().is_empty() => Ok(Vec::new()),
        Some(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|error| anyhow::anyhow!("Invalid role id `{value}`: {error}"))
            })
            .collect(),
    }
}

fn deserialize_optional_snowflake<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };

    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(value) if value.trim().is_empty() => Ok(None),
        serde_json::Value::String(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(serde::de::Error::custom),
        serde_json::Value::Number(value) => value
            .as_u64()
            .ok_or_else(|| serde::de::Error::custom("snowflake number must be an unsigned integer"))
            .map(Some),
        other => Err(serde::de::Error::custom(format!(
            "snowflake must be a string or number, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{GiveawaySettings, choose_winners, parse_role_ids};

    #[test]
    fn giveaway_settings_accept_string_channel() {
        let settings: GiveawaySettings = serde_json::from_value(serde_json::json!({
            "default_channel": "123",
            "button_label": "Join"
        }))
        .expect("settings");
        assert_eq!(settings.default_channel, Some(123));
        assert_eq!(settings.button_label, "Join");
    }

    #[test]
    fn parses_role_id_list() {
        let roles = parse_role_ids(Some("1, 2,3")).expect("roles");
        assert_eq!(roles, vec![1, 2, 3]);
    }

    #[test]
    fn choose_winners_never_exceeds_deduped_pool() {
        let winners = choose_winners(&[1, 1, 2], 3);
        assert!(winners.len() <= 2);
    }
}
