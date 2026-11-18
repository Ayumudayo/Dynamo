use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VoiceStatsRecord {
    pub connections: u64,
    pub time_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CommandUsageStats {
    pub slash: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MessageContextUsageStats {
    pub message: u64,
    pub user: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberStatsRecord {
    pub guild_id: u64,
    pub member_id: u64,
    pub messages: u64,
    pub voice: VoiceStatsRecord,
    pub commands: CommandUsageStats,
    pub contexts: MessageContextUsageStats,
    pub xp: u64,
    pub level: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Changes from one activity event; callers never send a stale whole record.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemberStatsDelta {
    pub messages: u64,
    pub voice_connections: u64,
    pub voice_seconds: u64,
    pub slash_commands: u64,
    pub message_contexts: u64,
    pub user_contexts: u64,
    pub xp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsLeaderboardMetric {
    Xp,
    Messages,
    VoiceSeconds,
}

#[derive(Debug, Clone)]
pub struct MemberStatsUpdate {
    pub record: MemberStatsRecord,
    pub leveled_up: bool,
}

impl MemberStatsRecord {
    pub fn xp_needed_for_level(level: u32) -> u64 {
        let level = u64::from(level.max(1));
        level.saturating_mul(level).saturating_mul(100)
    }

    pub fn apply_delta(
        &mut self,
        delta: MemberStatsDelta,
        updated_at: DateTime<Utc>,
    ) -> Result<bool, &'static str> {
        fn add(value: &mut u64, amount: u64) -> Result<(), &'static str> {
            *value = value
                .checked_add(amount)
                .ok_or("member stats counter overflow")?;
            Ok(())
        }
        add(&mut self.messages, delta.messages)?;
        add(&mut self.voice.connections, delta.voice_connections)?;
        add(&mut self.voice.time_seconds, delta.voice_seconds)?;
        add(&mut self.commands.slash, delta.slash_commands)?;
        add(&mut self.contexts.message, delta.message_contexts)?;
        add(&mut self.contexts.user, delta.user_contexts)?;
        add(&mut self.xp, delta.xp)?;
        if self.level == 0 {
            self.level = 1;
        }
        let mut leveled_up = false;
        if delta.xp > 0 {
            loop {
                let needed = Self::xp_needed_for_level(self.level);
                if self.xp < needed {
                    break;
                }
                self.level = self
                    .level
                    .checked_add(1)
                    .ok_or("member stats level overflow")?;
                self.xp -= needed;
                leveled_up = true;
            }
        }
        self.updated_at = updated_at;
        Ok(leveled_up)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{MemberStatsDelta, MemberStatsRecord};

    fn base_record(level: u32, xp: u64) -> MemberStatsRecord {
        let now = Utc::now();
        MemberStatsRecord {
            guild_id: 1,
            member_id: 2,
            messages: 0,
            voice: Default::default(),
            commands: Default::default(),
            contexts: Default::default(),
            xp,
            level,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn levels_up_on_exact_threshold_and_supports_multi_level_jump() {
        let mut exact = base_record(1, 90);
        let leveled = exact
            .apply_delta(
                MemberStatsDelta {
                    xp: 10,
                    ..Default::default()
                },
                Utc::now(),
            )
            .expect("delta succeeds");
        assert!(leveled);
        assert_eq!(exact.level, 2);
        assert_eq!(exact.xp, 0);

        let mut jump = base_record(1, 0);
        let leveled_multi = jump
            .apply_delta(
                MemberStatsDelta {
                    xp: 500,
                    ..Default::default()
                },
                Utc::now(),
            )
            .expect("multi-level delta succeeds");
        assert!(leveled_multi);
        assert_eq!(jump.level, 3);
        assert_eq!(jump.xp, 0);
    }
}
