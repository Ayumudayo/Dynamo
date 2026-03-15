use chrono::Utc;
use dynamo_core::{
    AppState, Context, DiscordCommand, Error, GatewayIntents, GuildModuleSettings, Module,
    ModuleCategory, ModuleManifest, SettingsField, SettingsFieldKind, SettingsSchema,
    SettingsSection, SuggestionRecord, SuggestionStats, SuggestionStatus, SuggestionStatusUpdate,
    module_access_for_context,
};
use poise::serenity_prelude::{
    ActionRowComponent, ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow,
    CreateButton, CreateEmbed, CreateEmbedFooter, CreateInputText, CreateInteractionResponse,
    CreateMessage, CreateModal, InputTextStyle, Interaction, Message, ModalInteraction,
    Permissions, ReactionType,
};
use serde::{Deserialize, Deserializer, Serialize};

const MODULE_ID: &str = "suggestion";
const UPVOTE_EMOJI: &str = "⬆️";
const DOWNVOTE_EMOJI: &str = "⬇️";
const DEFAULT_EMBED_COLOR: u32 = 0x4F545C;
const APPROVED_EMBED_COLOR: u32 = 0x43B581;
const REJECTED_EMBED_COLOR: u32 = 0xF04747;

const APPROVE_BUTTON_ID: &str = "suggestion_approve";
const REJECT_BUTTON_ID: &str = "suggestion_reject";
const DELETE_BUTTON_ID: &str = "suggestion_delete";

const APPROVE_MODAL_ID: &str = "suggestion_approve_modal";
const REJECT_MODAL_ID: &str = "suggestion_reject_modal";
const DELETE_MODAL_ID: &str = "suggestion_delete_modal";
const REASON_INPUT_ID: &str = "reason";

pub struct SuggestionModule;

impl Module for SuggestionModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Suggestion",
            "Guild suggestion board with approval and rejection workflows.",
            ModuleCategory::Suggestion,
            true,
            GatewayIntents::GUILDS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        vec![suggest()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema {
            sections: vec![SettingsSection {
                id: "suggestions",
                title: "Suggestions",
                description: Some("Configure the suggestion board channels and moderator roles."),
                fields: vec![
                    SettingsField {
                        key: "channel_id",
                        label: "Suggestion channel ID",
                        help_text: Some("Guild text channel where new suggestions are posted."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                    SettingsField {
                        key: "approved_channel_id",
                        label: "Approved channel ID",
                        help_text: Some(
                            "Optional target channel for approved suggestions. Leave empty to edit in place.",
                        ),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                    SettingsField {
                        key: "rejected_channel_id",
                        label: "Rejected channel ID",
                        help_text: Some(
                            "Optional target channel for rejected suggestions. Leave empty to edit in place.",
                        ),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                    SettingsField {
                        key: "staff_role_ids",
                        label: "Staff role IDs",
                        help_text: Some("Array of role IDs that may moderate suggestions."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                ],
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct SuggestionSettings {
    #[serde(deserialize_with = "deserialize_optional_snowflake")]
    channel_id: Option<u64>,
    #[serde(
        alias = "approved_channel",
        deserialize_with = "deserialize_optional_snowflake"
    )]
    approved_channel_id: Option<u64>,
    #[serde(
        alias = "rejected_channel",
        deserialize_with = "deserialize_optional_snowflake"
    )]
    rejected_channel_id: Option<u64>,
    #[serde(alias = "staff_roles", deserialize_with = "deserialize_snowflake_vec")]
    staff_role_ids: Vec<u64>,
}

#[poise::command(slash_command, guild_only, category = "Suggestion")]
async fn suggest(
    ctx: Context<'_>,
    #[description = "Suggestion text"] suggestion: String,
) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID).await?.denial_reason {
        ctx.send(
            poise::CreateReply::default()
                .content(reason)
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let Some(repo) = ctx.data().persistence.suggestions.clone() else {
        ctx.send(
            poise::CreateReply::default()
                .content("The suggestion repository is not available in this deployment.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    let settings = load_settings(ctx.data(), ctx.guild_id().map(|id| id.get())).await?;
    let Some(channel_id) = settings.channel_id else {
        ctx.send(
            poise::CreateReply::default()
                .content("Suggestion channel not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };

    let author = ctx.author();
    let embed = create_pending_embed(&suggestion, author.id.get(), &author.name, author.face());

    let message = ChannelId::new(channel_id)
        .send_message(
            ctx.serenity_context(),
            CreateMessage::new()
                .embed(embed)
                .components(moderation_buttons(SuggestionStatus::Pending)),
        )
        .await?;

    message
        .react(
            ctx.serenity_context(),
            ReactionType::Unicode(UPVOTE_EMOJI.to_string()),
        )
        .await?;
    message
        .react(
            ctx.serenity_context(),
            ReactionType::Unicode(DOWNVOTE_EMOJI.to_string()),
        )
        .await?;

    let now = Utc::now();
    repo.create(SuggestionRecord {
        guild_id: guild_id.get(),
        channel_id: message.channel_id.get(),
        message_id: message.id.get(),
        user_id: author.id.get(),
        suggestion,
        status: SuggestionStatus::Pending,
        stats: SuggestionStats::default(),
        status_updates: Vec::new(),
        created_at: now,
        updated_at: now,
    })
    .await?;

    ctx.send(
        poise::CreateReply::default()
            .content("Your suggestion has been submitted.")
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

async fn load_settings(
    data: &AppState,
    guild_id: Option<u64>,
) -> Result<SuggestionSettings, Error> {
    let Some(guild_id) = guild_id else {
        return Ok(SuggestionSettings::default());
    };

    let guild_settings = data.persistence.guild_settings_or_default(guild_id).await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(parse_suggestion_settings)
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
}

fn parse_suggestion_settings(module: &GuildModuleSettings) -> Result<SuggestionSettings, Error> {
    Ok(serde_json::from_value::<SuggestionSettings>(
        module.configuration.clone(),
    )?)
}

pub async fn handle_interaction(
    ctx: &poise::serenity_prelude::Context,
    interaction: &Interaction,
    data: &AppState,
) -> Result<bool, Error> {
    match interaction {
        Interaction::Component(component) if is_suggestion_button(&component.data.custom_id) => {
            handle_button_interaction(ctx, component).await?;
            Ok(true)
        }
        Interaction::Modal(modal) if is_suggestion_modal(&modal.data.custom_id) => {
            handle_modal_interaction(ctx, modal, data).await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn is_suggestion_button(custom_id: &str) -> bool {
    matches!(
        custom_id,
        APPROVE_BUTTON_ID | REJECT_BUTTON_ID | DELETE_BUTTON_ID
    )
}

fn is_suggestion_modal(custom_id: &str) -> bool {
    matches!(
        custom_id,
        APPROVE_MODAL_ID | REJECT_MODAL_ID | DELETE_MODAL_ID
    )
}

async fn handle_button_interaction(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
) -> Result<(), Error> {
    let (modal_id, title) = match component.data.custom_id.as_str() {
        APPROVE_BUTTON_ID => (APPROVE_MODAL_ID, "Approve Suggestion"),
        REJECT_BUTTON_ID => (REJECT_MODAL_ID, "Reject Suggestion"),
        DELETE_BUTTON_ID => (DELETE_MODAL_ID, "Delete Suggestion"),
        _ => return Ok(()),
    };

    component
        .create_response(
            ctx,
            CreateInteractionResponse::Modal(CreateModal::new(modal_id, title).components(
                vec![CreateActionRow::InputText(
                    CreateInputText::new(InputTextStyle::Paragraph, "Reason", REASON_INPUT_ID)
                        .placeholder("Optional reason")
                        .required(false),
                )],
            )),
        )
        .await?;

    Ok(())
}

async fn handle_modal_interaction(
    ctx: &poise::serenity_prelude::Context,
    modal: &ModalInteraction,
    data: &AppState,
) -> Result<(), Error> {
    modal.defer_ephemeral(ctx).await?;

    let Some(member) = modal.member.as_ref() else {
        modal
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("This action can only be used in a guild."),
            )
            .await?;
        return Ok(());
    };

    let Some(guild_id) = modal.guild_id else {
        modal
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("This action can only be used in a guild."),
            )
            .await?;
        return Ok(());
    };

    let Some(source_message) = modal.message.as_deref() else {
        modal
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("The original suggestion message is no longer available."),
            )
            .await?;
        return Ok(());
    };

    let settings = load_settings(data, Some(guild_id.get())).await?;
    if !has_moderation_permissions(member, &settings) {
        modal
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("You don't have permission to moderate suggestions."),
            )
            .await?;
        return Ok(());
    }

    let Some(repo) = data.persistence.suggestions.clone() else {
        modal
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("The suggestion repository is not available in this deployment."),
            )
            .await?;
        return Ok(());
    };

    let Some(record) = repo
        .get_by_message(guild_id.get(), source_message.id.get())
        .await?
    else {
        modal
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("Suggestion not found."),
            )
            .await?;
        return Ok(());
    };

    let reason = modal_reason(modal).map(|value| value.trim().to_string());
    let reason = reason.filter(|value| !value.is_empty());
    let response = match modal.data.custom_id.as_str() {
        APPROVE_MODAL_ID => {
            transition_suggestion(
                ctx,
                data,
                repo,
                record,
                source_message,
                member,
                &settings,
                SuggestionStatus::Approved,
                reason,
            )
            .await?
        }
        REJECT_MODAL_ID => {
            transition_suggestion(
                ctx,
                data,
                repo,
                record,
                source_message,
                member,
                &settings,
                SuggestionStatus::Rejected,
                reason,
            )
            .await?
        }
        DELETE_MODAL_ID => {
            delete_suggestion(ctx, repo, record, source_message, member, reason).await?
        }
        _ => "Not a valid moderation action.".to_string(),
    };

    modal
        .edit_response(
            ctx,
            poise::serenity_prelude::EditInteractionResponse::new().content(response),
        )
        .await?;
    Ok(())
}

async fn transition_suggestion(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    repo: std::sync::Arc<dyn dynamo_core::SuggestionsRepository>,
    mut record: SuggestionRecord,
    source_message: &Message,
    moderator: &poise::serenity_prelude::Member,
    settings: &SuggestionSettings,
    next_status: SuggestionStatus,
    reason: Option<String>,
) -> Result<String, Error> {
    if record.status == next_status {
        return Ok(match next_status {
            SuggestionStatus::Approved => "Suggestion already approved.".to_string(),
            SuggestionStatus::Rejected => "Suggestion already rejected.".to_string(),
            _ => "Suggestion already updated.".to_string(),
        });
    }

    let stats = vote_stats(source_message);
    record.status = next_status;
    record.stats = stats;
    record.status_updates.push(SuggestionStatusUpdate {
        user_id: moderator.user.id.get(),
        status: next_status,
        reason: reason.clone(),
        timestamp: Utc::now(),
    });
    record.updated_at = Utc::now();

    let target_channel_id = match next_status {
        SuggestionStatus::Approved => settings.approved_channel_id,
        SuggestionStatus::Rejected => settings.rejected_channel_id,
        _ => None,
    };

    let embed = create_reviewed_embed(
        &record,
        &moderator.user.name,
        moderator.user.face(),
        reason.as_deref(),
    );
    let buttons = moderation_buttons(next_status);

    if let Some(target_channel_id) = target_channel_id {
        let sent = ChannelId::new(target_channel_id)
            .send_message(
                ctx,
                CreateMessage::new()
                    .embed(embed)
                    .components(buttons.clone()),
            )
            .await?;
        source_message.delete(ctx).await?;
        record.channel_id = sent.channel_id.get();
        record.message_id = sent.id.get();
    } else {
        source_message
            .channel_id
            .edit_message(
                ctx,
                source_message.id,
                poise::serenity_prelude::EditMessage::new()
                    .embed(embed)
                    .components(buttons),
            )
            .await?;
        source_message.delete_reactions(ctx).await?;
    }

    repo.save(record).await?;

    let deployment = data.persistence.deployment_settings_or_default().await?;
    let response = if deployment
        .modules
        .get(MODULE_ID)
        .is_some_and(|module| !module.enabled)
    {
        "Suggestion updated while the module is currently disabled for the deployment.".to_string()
    } else {
        match next_status {
            SuggestionStatus::Approved => "Suggestion approved.".to_string(),
            SuggestionStatus::Rejected => "Suggestion rejected.".to_string(),
            _ => "Suggestion updated.".to_string(),
        }
    };

    Ok(response)
}

async fn delete_suggestion(
    ctx: &poise::serenity_prelude::Context,
    repo: std::sync::Arc<dyn dynamo_core::SuggestionsRepository>,
    mut record: SuggestionRecord,
    source_message: &Message,
    moderator: &poise::serenity_prelude::Member,
    reason: Option<String>,
) -> Result<String, Error> {
    source_message.delete(ctx).await?;

    record.status = SuggestionStatus::Deleted;
    record.updated_at = Utc::now();
    record.status_updates.push(SuggestionStatusUpdate {
        user_id: moderator.user.id.get(),
        status: SuggestionStatus::Deleted,
        reason,
        timestamp: Utc::now(),
    });
    repo.save(record).await?;

    Ok("Suggestion deleted.".to_string())
}

fn has_moderation_permissions(
    member: &poise::serenity_prelude::Member,
    settings: &SuggestionSettings,
) -> bool {
    if member
        .permissions
        .unwrap_or_else(Permissions::empty)
        .manage_guild()
    {
        return true;
    }

    member
        .roles
        .iter()
        .any(|role_id| settings.staff_role_ids.contains(&role_id.get()))
}

fn moderation_buttons(status: SuggestionStatus) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(APPROVE_BUTTON_ID)
            .label("Approve")
            .style(ButtonStyle::Success)
            .disabled(status == SuggestionStatus::Approved),
        CreateButton::new(REJECT_BUTTON_ID)
            .label("Reject")
            .style(ButtonStyle::Danger)
            .disabled(status == SuggestionStatus::Rejected),
        CreateButton::new(DELETE_BUTTON_ID)
            .label("Delete")
            .style(ButtonStyle::Secondary),
    ])]
}

fn create_pending_embed(
    suggestion: &str,
    user_id: u64,
    username: &str,
    avatar_url: String,
) -> CreateEmbed {
    CreateEmbed::new()
        .title("New Suggestion")
        .description(suggestion)
        .color(DEFAULT_EMBED_COLOR)
        .thumbnail(avatar_url)
        .field("Submitter", format!("{username} [<@{user_id}>]"), false)
        .footer(CreateEmbedFooter::new("Pending review"))
        .timestamp(poise::serenity_prelude::Timestamp::now())
}

fn create_reviewed_embed(
    record: &SuggestionRecord,
    moderator_name: &str,
    moderator_avatar: String,
    reason: Option<&str>,
) -> CreateEmbed {
    let status_name = match record.status {
        SuggestionStatus::Approved => "Suggestion Approved",
        SuggestionStatus::Rejected => "Suggestion Rejected",
        SuggestionStatus::Deleted => "Suggestion Deleted",
        SuggestionStatus::Pending => "Suggestion Pending",
    };

    let mut embed = CreateEmbed::new()
        .title(status_name)
        .description(&record.suggestion)
        .color(status_color(record.status))
        .thumbnail(moderator_avatar)
        .field("Submitter", format!("<@{}>", record.user_id), false)
        .field("Stats", vote_message(&record.stats), false)
        .footer(CreateEmbedFooter::new(format!(
            "{} by {}",
            match record.status {
                SuggestionStatus::Approved => "Approved",
                SuggestionStatus::Rejected => "Rejected",
                SuggestionStatus::Deleted => "Deleted",
                SuggestionStatus::Pending => "Updated",
            },
            moderator_name
        )))
        .timestamp(poise::serenity_prelude::Timestamp::now());

    if let Some(reason) = reason {
        embed = embed.field("Reason", format!("```{reason}```"), false);
    }

    embed
}

fn status_color(status: SuggestionStatus) -> u32 {
    match status {
        SuggestionStatus::Approved => APPROVED_EMBED_COLOR,
        SuggestionStatus::Rejected | SuggestionStatus::Deleted => REJECTED_EMBED_COLOR,
        SuggestionStatus::Pending => DEFAULT_EMBED_COLOR,
    }
}

fn vote_stats(message: &Message) -> SuggestionStats {
    let upvotes = reaction_count(message, UPVOTE_EMOJI);
    let downvotes = reaction_count(message, DOWNVOTE_EMOJI);
    SuggestionStats { upvotes, downvotes }
}

fn reaction_count(message: &Message, emoji: &str) -> u64 {
    message
        .reactions
        .iter()
        .find(|reaction| matches!(&reaction.reaction_type, ReactionType::Unicode(value) if value == emoji))
        .map(|reaction| reaction.count.saturating_sub(1))
        .unwrap_or(0)
}

fn vote_message(stats: &SuggestionStats) -> String {
    let total = stats.upvotes + stats.downvotes;
    if total == 0 {
        return "_Upvotes: NA_\n_Downvotes: NA_".to_string();
    }

    let upvote_percent = ((stats.upvotes as f64 / total as f64) * 100.0).round() as u64;
    let downvote_percent = ((stats.downvotes as f64 / total as f64) * 100.0).round() as u64;
    format!(
        "_Upvotes: {} [{}%]_\n_Downvotes: {} [{}%]_",
        stats.upvotes, upvote_percent, stats.downvotes, downvote_percent
    )
}

fn modal_reason(modal: &ModalInteraction) -> Option<String> {
    modal
        .data
        .components
        .iter()
        .flat_map(|row| row.components.iter())
        .find_map(|component| match component {
            ActionRowComponent::InputText(input) if input.custom_id == REASON_INPUT_ID => {
                input.value.clone()
            }
            _ => None,
        })
}

fn deserialize_optional_snowflake<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    parse_optional_snowflake_value(value).map_err(serde::de::Error::custom)
}

fn deserialize_snowflake_vec<'de, D>(deserializer: D) -> Result<Vec<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    parse_snowflake_vec_value(value).map_err(serde::de::Error::custom)
}

fn parse_optional_snowflake_value(value: Option<serde_json::Value>) -> Result<Option<u64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };

    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(value) if value.trim().is_empty() => Ok(None),
        serde_json::Value::String(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|error| format!("invalid snowflake `{value}`: {error}")),
        serde_json::Value::Number(value) => value
            .as_u64()
            .ok_or_else(|| "snowflake number must be an unsigned integer".to_string())
            .map(Some),
        other => Err(format!("snowflake must be a string or number, got {other}")),
    }
}

fn parse_snowflake_vec_value(value: Option<serde_json::Value>) -> Result<Vec<u64>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(|value| parse_optional_snowflake_value(Some(value)))
            .collect::<Result<Vec<_>, _>>()
            .map(|values| values.into_iter().flatten().collect()),
        serde_json::Value::String(values) if values.trim().is_empty() => Ok(Vec::new()),
        serde_json::Value::String(values) => values
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|error| format!("invalid snowflake `{value}`: {error}"))
            })
            .collect(),
        other => Err(format!(
            "snowflake array must be a string or array, got {other}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{SuggestionSettings, vote_message};
    use dynamo_core::SuggestionStats;

    #[test]
    fn suggestion_settings_accepts_string_ids() {
        let settings: SuggestionSettings = serde_json::from_value(serde_json::json!({
            "channel_id": "123",
            "approved_channel": "456",
            "rejected_channel": 789,
            "staff_roles": ["11", 22]
        }))
        .expect("settings should deserialize");

        assert_eq!(settings.channel_id, Some(123));
        assert_eq!(settings.approved_channel_id, Some(456));
        assert_eq!(settings.rejected_channel_id, Some(789));
        assert_eq!(settings.staff_role_ids, vec![11, 22]);
    }

    #[test]
    fn vote_message_handles_zero_votes() {
        assert_eq!(
            vote_message(&SuggestionStats::default()),
            "_Upvotes: NA_\n_Downvotes: NA_"
        );
    }
}
