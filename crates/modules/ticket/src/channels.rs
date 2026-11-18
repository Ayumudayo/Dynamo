use dynamo_runtime_api::Error;
use poise::serenity_prelude::{Channel, ChannelId, ChannelType, GuildChannel, GuildId};

pub(crate) fn is_ticket_channel(channel: &GuildChannel) -> bool {
    channel.kind == ChannelType::Text
        && (channel.name.starts_with("ticket-") || channel.name.starts_with("tіcket-"))
        && channel
            .topic
            .as_deref()
            .is_some_and(|topic| topic.starts_with("ticket|") || topic.starts_with("tіcket|"))
}

pub(crate) fn parse_ticket_details(channel: &GuildChannel) -> Option<(u64, String)> {
    parse_ticket_topic(channel.topic.as_deref()?)
}

pub(crate) fn parse_ticket_topic(topic: &str) -> Option<(u64, String)> {
    let normalized = topic.replace("tіcket|", "ticket|");
    let mut split = normalized.splitn(3, '|');
    let prefix = split.next()?;
    if prefix != "ticket" {
        return None;
    }
    let user_id = split.next()?.parse().ok()?;
    let category_name = split.next().unwrap_or("Default").to_string();
    Some((user_id, category_name))
}

pub(crate) fn next_ticket_number(channels: &[GuildChannel]) -> u64 {
    channels
        .iter()
        .filter_map(|channel| {
            channel
                .name
                .strip_prefix("ticket-")
                .or_else(|| channel.name.strip_prefix("tіcket-"))
                .and_then(|suffix| suffix.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

pub(crate) async fn ticket_channels(
    ctx: &poise::serenity_prelude::Context,
    guild_id: GuildId,
) -> Result<Vec<GuildChannel>, Error> {
    Ok(guild_id
        .channels(ctx)
        .await?
        .into_values()
        .filter(is_ticket_channel)
        .collect())
}

pub(crate) fn existing_ticket_channel(
    channels: &[GuildChannel],
    user_id: u64,
) -> Option<GuildChannel> {
    channels
        .iter()
        .find(|channel| {
            parse_ticket_details(channel).is_some_and(|(opened_by, _)| opened_by == user_id)
        })
        .cloned()
}

pub(crate) async fn current_guild_channel(
    ctx: &poise::serenity_prelude::Context,
    channel_id: ChannelId,
) -> Result<Option<GuildChannel>, Error> {
    let channel = channel_id.to_channel(ctx).await?;
    Ok(match channel {
        Channel::Guild(channel) => Some(channel),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_ticket_topic;

    #[test]
    fn parse_ticket_topic_preserves_pipes_in_category_and_normalizes_cyrillic_prefix() {
        assert_eq!(
            parse_ticket_topic("ticket|42|Billing | VIP"),
            Some((42, "Billing | VIP".to_string()))
        );
        assert_eq!(
            parse_ticket_topic("tіcket|42|Support"),
            Some((42, "Support".to_string()))
        );
    }
}
