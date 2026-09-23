use dynamo_domain_suggestion::SuggestionStats;

use crate::{render::vote_message, settings::SuggestionSettings};

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
