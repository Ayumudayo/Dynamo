use crate::constants::{
    ACTIVE_COLOR, DEFAULT_BUTTON_LABEL, ENDED_COLOR, GIVEAWAY_ENTER_BUTTON_ID, PAUSED_COLOR,
};
use crate::lifecycle::format_winners;
use dynamo_domain_giveaway::{GiveawayRecord, GiveawayStatus};
use poise::serenity_prelude::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter, Timestamp,
};

pub(crate) fn build_embed(record: &GiveawayRecord) -> CreateEmbed {
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

pub(crate) fn entry_button(record: &GiveawayRecord) -> CreateActionRow {
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
