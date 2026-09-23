use crate::{
    policy::{ModerationTargetDenial, ModerationTargetFacts},
    settings::ModerationSettings,
    target::authorize_warning_history_clear,
    user_actions::parse_user_id,
};
use poise::serenity_prelude::{Permissions, UserId};
use std::cell::Cell;

#[derive(Default)]
struct WarningRepositorySpy {
    clear_calls: Cell<usize>,
}

impl WarningRepositorySpy {
    fn clear_for_member(&self, _guild_id: u64, _member_id: u64) {
        self.clear_calls.set(self.clear_calls.get() + 1);
    }
}

fn moderation_target_fixture() -> ModerationTargetFacts {
    ModerationTargetFacts {
        actor_id: UserId::new(1),
        actor_permissions: Permissions::KICK_MEMBERS,
        actor_top_role: 20,
        bot_id: UserId::new(2),
        bot_permissions: Permissions::KICK_MEMBERS,
        bot_top_role: 30,
        target_id: UserId::new(3),
        target_is_bot: false,
        target_top_role: 10,
        guild_owner_id: UserId::new(4),
    }
}

fn assert_warning_clear_denied(facts: ModerationTargetFacts, expected: ModerationTargetDenial) {
    let repository = WarningRepositorySpy::default();
    let result = authorize_warning_history_clear(&facts, || {
        repository.clear_for_member(100, facts.target_id.get())
    });
    let error = match result {
        Ok(()) => panic!("warning clear unexpectedly authorized"),
        Err(error) => error,
    };
    assert_eq!(error.downcast_ref(), Some(&expected));
    assert_eq!(repository.clear_calls.get(), 0);
}

#[test]
fn warning_clear_denials_never_invoke_the_repository() {
    let mut actor_permission = moderation_target_fixture();
    actor_permission.actor_permissions = Permissions::empty();
    assert_warning_clear_denied(actor_permission, ModerationTargetDenial::ActorPermission);
    let mut bot_permission = moderation_target_fixture();
    bot_permission.bot_permissions = Permissions::empty();
    assert_warning_clear_denied(bot_permission, ModerationTargetDenial::BotPermission);
    let mut self_target = moderation_target_fixture();
    self_target.target_id = self_target.actor_id;
    assert_warning_clear_denied(self_target, ModerationTargetDenial::SelfTarget);
    let mut bot_target = moderation_target_fixture();
    bot_target.target_is_bot = true;
    assert_warning_clear_denied(bot_target, ModerationTargetDenial::BotTarget);
    let mut guild_owner = moderation_target_fixture();
    guild_owner.target_id = guild_owner.guild_owner_id;
    assert_warning_clear_denied(guild_owner, ModerationTargetDenial::GuildOwnerTarget);
    let mut actor_hierarchy = moderation_target_fixture();
    actor_hierarchy.actor_top_role = actor_hierarchy.target_top_role;
    assert_warning_clear_denied(actor_hierarchy, ModerationTargetDenial::ActorHierarchy);
    let mut bot_hierarchy = moderation_target_fixture();
    bot_hierarchy.bot_top_role = bot_hierarchy.target_top_role;
    assert_warning_clear_denied(bot_hierarchy, ModerationTargetDenial::BotHierarchy);
}

#[test]
fn warning_clear_authorization_invokes_the_repository_once() {
    let facts = moderation_target_fixture();
    let repository = WarningRepositorySpy::default();
    authorize_warning_history_clear(&facts, || {
        repository.clear_for_member(100, facts.target_id.get())
    })
    .expect("authorized warning clear");
    assert_eq!(repository.clear_calls.get(), 1);
}

#[test]
fn moderation_settings_accepts_nested_shape() {
    let settings: ModerationSettings = serde_json::from_value(serde_json::json!({
        "modlog_channel_id": "123", "max_warn": { "limit": 3, "action": "BAN" }
    }))
    .expect("settings");
    assert_eq!(settings.modlog_channel_id, Some(123));
    assert_eq!(settings.max_warn.limit, 3);
}

#[test]
fn parses_user_id_from_mention() {
    assert_eq!(parse_user_id("<@123>").expect("user").get(), 123);
}
