use crate::{channels::parse_ticket_topic, commands::extract_target_id, settings::TicketSettings};

#[test]
fn ticket_settings_accepts_legacy_keys() {
    let settings: TicketSettings = serde_json::from_value(serde_json::json!({
        "log_channel": "123",
        "limit": 25,
        "categories": [
            { "name": "Billing", "staff_roles": ["11", 22] }
        ]
    }))
    .expect("settings should deserialize");

    assert_eq!(settings.log_channel_id, Some(123));
    assert_eq!(settings.limit, 25);
    assert_eq!(settings.categories[0].staff_role_ids, vec![11, 22]);
    assert_eq!(settings.setup_title, "Support Ticket");
}

#[test]
fn parses_ascii_ticket_topic() {
    assert_eq!(
        parse_ticket_topic("ticket|42|Billing"),
        Some((42, "Billing".to_string()))
    );
}

#[test]
fn extracts_mentions_to_numeric_ids() {
    assert_eq!(extract_target_id("<@123>").expect("user mention"), 123);
    assert_eq!(extract_target_id("<@&456>").expect("role mention"), 456);
    assert_eq!(extract_target_id("<@!789>").expect("nickname mention"), 789);
}
