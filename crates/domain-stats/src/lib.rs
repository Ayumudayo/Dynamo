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
    pub prefix: u64,
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

#[derive(Debug, Clone)]
pub struct MemberStatsUpdate {
    pub record: MemberStatsRecord,
    pub leveled_up: bool,
}

impl MemberStatsRecord {
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
        let needed = u64::from(self.level)
            .saturating_mul(u64::from(self.level))
            .saturating_mul(100);
        let leveled_up = delta.xp > 0 && self.xp > needed;
        if leveled_up {
            self.level = self
                .level
                .checked_add(1)
                .ok_or("member stats level overflow")?;
            self.xp -= needed;
        }
        self.updated_at = updated_at;
        Ok(leveled_up)
    }
}
