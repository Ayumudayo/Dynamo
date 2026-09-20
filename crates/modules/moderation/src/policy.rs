use poise::serenity_prelude::{Permissions, UserId};
use std::fmt;

/// Discord facts required to authorize a moderation target without side effects.
#[derive(Debug, Clone, Copy)]
pub struct ModerationTargetFacts {
    pub actor_id: UserId,
    pub actor_permissions: Permissions,
    pub actor_top_role: i64,
    pub bot_id: UserId,
    pub bot_permissions: Permissions,
    pub bot_top_role: i64,
    pub target_id: UserId,
    pub target_is_bot: bool,
    pub target_top_role: i64,
    pub guild_owner_id: UserId,
}

/// The first fail-closed reason that prevents a moderation action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModerationTargetDenial {
    ActorPermission,
    BotPermission,
    SelfTarget,
    BotTarget,
    GuildOwnerTarget,
    ActorHierarchy,
    BotHierarchy,
}

impl fmt::Display for ModerationTargetDenial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ActorPermission => "You do not have the required Discord permission.",
            Self::BotPermission => "The bot does not have the required Discord permission.",
            Self::SelfTarget => "You cannot moderate yourself.",
            Self::BotTarget => "Bots are protected moderation targets.",
            Self::GuildOwnerTarget => "The server owner is a protected moderation target.",
            Self::ActorHierarchy => "You do not have permission to moderate this member.",
            Self::BotHierarchy => "The bot cannot moderate this member due to role hierarchy.",
        })
    }
}

impl std::error::Error for ModerationTargetDenial {}

/// Authorizes a target using a stable, fail-closed check order.
pub fn authorize_moderation_target(
    facts: &ModerationTargetFacts,
    required_permission: Permissions,
) -> Result<(), ModerationTargetDenial> {
    if !facts.actor_permissions.contains(required_permission) {
        return Err(ModerationTargetDenial::ActorPermission);
    }
    if !facts.bot_permissions.contains(required_permission) {
        return Err(ModerationTargetDenial::BotPermission);
    }
    if facts.target_id == facts.actor_id {
        return Err(ModerationTargetDenial::SelfTarget);
    }
    if facts.target_is_bot {
        return Err(ModerationTargetDenial::BotTarget);
    }
    if facts.target_id == facts.guild_owner_id {
        return Err(ModerationTargetDenial::GuildOwnerTarget);
    }
    if facts.actor_id != facts.guild_owner_id && facts.actor_top_role <= facts.target_top_role {
        return Err(ModerationTargetDenial::ActorHierarchy);
    }
    if facts.bot_id != facts.guild_owner_id && facts.bot_top_role <= facts.target_top_role {
        return Err(ModerationTargetDenial::BotHierarchy);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ModerationTargetDenial, ModerationTargetFacts, authorize_moderation_target};
    use poise::serenity_prelude::{Permissions, UserId};

    fn fixture() -> ModerationTargetFacts {
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

    fn assert_denied(facts: ModerationTargetFacts, expected: ModerationTargetDenial) {
        assert_eq!(
            authorize_moderation_target(&facts, Permissions::KICK_MEMBERS),
            Err(expected)
        );
    }

    #[test]
    fn every_denial_is_fail_closed() {
        let mut actor_permission = fixture();
        actor_permission.actor_permissions = Permissions::empty();
        assert_denied(actor_permission, ModerationTargetDenial::ActorPermission);

        let mut bot_permission = fixture();
        bot_permission.bot_permissions = Permissions::empty();
        assert_denied(bot_permission, ModerationTargetDenial::BotPermission);

        let mut self_target = fixture();
        self_target.target_id = self_target.actor_id;
        assert_denied(self_target, ModerationTargetDenial::SelfTarget);

        let mut bot_target = fixture();
        bot_target.target_is_bot = true;
        assert_denied(bot_target, ModerationTargetDenial::BotTarget);

        let mut guild_owner = fixture();
        guild_owner.target_id = guild_owner.guild_owner_id;
        assert_denied(guild_owner, ModerationTargetDenial::GuildOwnerTarget);

        let mut equal_actor_role = fixture();
        equal_actor_role.actor_top_role = equal_actor_role.target_top_role;
        assert_denied(equal_actor_role, ModerationTargetDenial::ActorHierarchy);

        let mut higher_actor_role = fixture();
        higher_actor_role.actor_top_role = higher_actor_role.target_top_role - 1;
        assert_denied(higher_actor_role, ModerationTargetDenial::ActorHierarchy);

        let mut equal_bot_role = fixture();
        equal_bot_role.bot_top_role = equal_bot_role.target_top_role;
        assert_denied(equal_bot_role, ModerationTargetDenial::BotHierarchy);

        let mut higher_bot_role = fixture();
        higher_bot_role.bot_top_role = higher_bot_role.target_top_role - 1;
        assert_denied(higher_bot_role, ModerationTargetDenial::BotHierarchy);
    }

    #[test]
    fn denial_precedence_matches_the_documented_order() {
        let mut actor_before_bot = fixture();
        actor_before_bot.actor_permissions = Permissions::empty();
        actor_before_bot.bot_permissions = Permissions::empty();
        assert_denied(actor_before_bot, ModerationTargetDenial::ActorPermission);

        let mut bot_before_self = fixture();
        bot_before_self.bot_permissions = Permissions::empty();
        bot_before_self.target_id = bot_before_self.actor_id;
        assert_denied(bot_before_self, ModerationTargetDenial::BotPermission);

        let mut self_before_bot = fixture();
        self_before_bot.target_id = self_before_bot.actor_id;
        self_before_bot.target_is_bot = true;
        assert_denied(self_before_bot, ModerationTargetDenial::SelfTarget);

        let mut bot_before_owner = fixture();
        bot_before_owner.target_id = bot_before_owner.guild_owner_id;
        bot_before_owner.target_is_bot = true;
        assert_denied(bot_before_owner, ModerationTargetDenial::BotTarget);

        let mut owner_before_actor_hierarchy = fixture();
        owner_before_actor_hierarchy.target_id = owner_before_actor_hierarchy.guild_owner_id;
        owner_before_actor_hierarchy.actor_top_role = 0;
        assert_denied(
            owner_before_actor_hierarchy,
            ModerationTargetDenial::GuildOwnerTarget,
        );

        let mut actor_before_bot_hierarchy = fixture();
        actor_before_bot_hierarchy.actor_top_role = actor_before_bot_hierarchy.target_top_role;
        actor_before_bot_hierarchy.bot_top_role = actor_before_bot_hierarchy.target_top_role;
        assert_denied(
            actor_before_bot_hierarchy,
            ModerationTargetDenial::ActorHierarchy,
        );
    }

    #[test]
    fn authorized_lower_role_is_allowed() {
        assert_eq!(
            authorize_moderation_target(&fixture(), Permissions::KICK_MEMBERS),
            Ok(())
        );
    }

    #[test]
    fn guild_owner_actor_bypasses_actor_role_hierarchy() {
        let mut facts = fixture();
        facts.actor_id = facts.guild_owner_id;
        facts.actor_top_role = 0;
        assert_eq!(
            authorize_moderation_target(&facts, Permissions::KICK_MEMBERS),
            Ok(())
        );
    }
}
