mod dashboard_audit;
mod giveaway;
mod invite;
mod moderation;
mod provider_state;
mod settings;
mod stats;
mod suggestion;

pub(crate) use dashboard_audit::DashboardAuditLogDocument;
pub(crate) use giveaway::GiveawayDocument;
pub(crate) use invite::InviteMemberDocument;
pub(crate) use moderation::WarningLogDocument;
pub(crate) use provider_state::ProviderStateDocument;
pub(crate) use settings::{DeploymentSettingsDocument, GuildSettingsDocument};
pub(crate) use stats::MemberStatsDocument;
pub(crate) use suggestion::SuggestionDocument;

use crate::Error;

pub(crate) fn parse_snowflake(value: &str, field_name: &str) -> Result<u64, Error> {
    value
        .parse()
        .map_err(|error| anyhow::anyhow!("Stored {field_name} is not a valid u64: {error}"))
}
