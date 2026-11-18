//! A presentation model for module and command enablement.
//!
//! The enablement crate resolves raw booleans; this module turns them into
//! what the dashboard shows: one overall state (active / off / blocked), the
//! switch value the user controls in the current scope, the activation chain,
//! and the first blocker when a switched-on item is still inactive.

use dynamo_enablement::{ResolvedCommandState, ResolvedModuleState};
use serde::Serialize;

use crate::i18n::{Locale, Texts};

/// Where settings are being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Deployment,
    Guild(u64),
}

impl Scope {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Scope::Deployment => "deployment",
            Scope::Guild(_) => "guild",
        }
    }

    pub(crate) fn guild_id(self) -> Option<u64> {
        match self {
            Scope::Deployment => None,
            Scope::Guild(guild_id) => Some(guild_id),
        }
    }

    /// The page path that owns this scope.
    pub(crate) fn base_path(self) -> String {
        match self {
            Scope::Deployment => "/deployment".to_string(),
            Scope::Guild(guild_id) => format!("/guild/{guild_id}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntityKind {
    Module,
    Command,
}

impl EntityKind {
    pub(crate) fn key(self) -> &'static str {
        match self {
            EntityKind::Module => "module",
            EntityKind::Command => "command",
        }
    }

    pub(crate) fn from_key(value: &str) -> Option<Self> {
        match value {
            "module" => Some(EntityKind::Module),
            "command" => Some(EntityKind::Command),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntityState {
    Active,
    Off,
    Blocked,
}

impl EntityState {
    pub(crate) fn key(self) -> &'static str {
        match self {
            EntityState::Active => "active",
            EntityState::Off => "off",
            EntityState::Blocked => "blocked",
        }
    }

    pub(crate) fn tone(self) -> &'static str {
        match self {
            EntityState::Active => "success",
            EntityState::Off => "neutral",
            EntityState::Blocked => "warn",
        }
    }

    pub(crate) fn label(self, t: &Texts) -> &'static str {
        match self {
            EntityState::Active => t.state_active,
            EntityState::Off => t.state_off,
            EntityState::Blocked => t.state_blocked,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Blocker {
    ParentModule,
    NotInstalled,
    Deployment,
    Server,
}

impl Blocker {
    pub(crate) fn label(self, t: &Texts) -> &'static str {
        match self {
            Blocker::ParentModule => t.blocker_parent_module,
            Blocker::NotInstalled => t.blocker_not_installed,
            Blocker::Deployment => t.blocker_deployment,
            Blocker::Server => t.blocker_server,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChainLink {
    Module,
    Installed,
    Deployment,
    Server,
}

impl ChainLink {
    pub(crate) fn label(self, t: &Texts) -> &'static str {
        match self {
            ChainLink::Module => t.chain_module,
            ChainLink::Installed => t.chain_installed,
            ChainLink::Deployment => t.chain_deployment,
            ChainLink::Server => t.chain_server,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntityStatus {
    pub(crate) state: EntityState,
    /// The value of the switch the user controls in this scope.
    pub(crate) own_enabled: bool,
    pub(crate) blocker: Option<Blocker>,
    pub(crate) chain: Vec<(ChainLink, bool)>,
}

impl EntityStatus {
    pub(crate) fn for_module(scope: Scope, resolved: &ResolvedModuleState) -> Self {
        let mut chain = vec![
            (ChainLink::Installed, resolved.installed),
            (ChainLink::Deployment, resolved.deployment_enabled),
        ];
        let own_enabled = match scope {
            Scope::Deployment => resolved.deployment_enabled,
            Scope::Guild(_) => {
                chain.push((ChainLink::Server, resolved.guild_enabled));
                resolved.guild_enabled
            }
        };
        let effective = match scope {
            Scope::Deployment => resolved.installed && resolved.deployment_enabled,
            Scope::Guild(_) => resolved.effective_enabled,
        };
        let blocker = if !resolved.installed {
            Some(Blocker::NotInstalled)
        } else if !resolved.deployment_enabled {
            Some(Blocker::Deployment)
        } else if matches!(scope, Scope::Guild(_)) && !resolved.guild_enabled {
            Some(Blocker::Server)
        } else {
            None
        };
        Self::from_parts(effective, own_enabled, blocker, chain)
    }

    pub(crate) fn for_command(scope: Scope, resolved: &ResolvedCommandState) -> Self {
        let mut chain = vec![
            (ChainLink::Module, resolved.module_effective_enabled),
            (ChainLink::Installed, resolved.installed),
            (ChainLink::Deployment, resolved.deployment_enabled),
        ];
        let own_enabled = match scope {
            Scope::Deployment => resolved.deployment_enabled,
            Scope::Guild(_) => {
                chain.push((ChainLink::Server, resolved.guild_enabled));
                resolved.guild_enabled
            }
        };
        let effective = match scope {
            Scope::Deployment => {
                resolved.module_effective_enabled
                    && resolved.installed
                    && resolved.deployment_enabled
            }
            Scope::Guild(_) => resolved.effective_enabled,
        };
        let blocker = if !resolved.module_effective_enabled {
            Some(Blocker::ParentModule)
        } else if !resolved.installed {
            Some(Blocker::NotInstalled)
        } else if !resolved.deployment_enabled {
            Some(Blocker::Deployment)
        } else if matches!(scope, Scope::Guild(_)) && !resolved.guild_enabled {
            Some(Blocker::Server)
        } else {
            None
        };
        Self::from_parts(effective, own_enabled, blocker, chain)
    }

    fn from_parts(
        effective: bool,
        own_enabled: bool,
        blocker: Option<Blocker>,
        chain: Vec<(ChainLink, bool)>,
    ) -> Self {
        let state = if effective {
            EntityState::Active
        } else if !own_enabled {
            EntityState::Off
        } else {
            EntityState::Blocked
        };
        Self {
            state,
            own_enabled,
            // Only a switched-on item has something "blocking" it; an item the
            // user turned off is simply off.
            blocker: (state == EntityState::Blocked).then_some(blocker).flatten(),
            chain,
        }
    }

    pub(crate) fn blocker_text(&self, t: &Texts) -> Option<String> {
        self.blocker
            .map(|blocker| crate::i18n::fill(t.blocked_reason, &[("reason", &blocker.label(t))]))
    }

    pub(crate) fn to_view(&self, kind: EntityKind, id: &str, locale: Locale) -> EntityStatusView {
        let t = locale.t();
        EntityStatusView {
            kind: kind.key(),
            id: id.to_string(),
            state: self.state.key(),
            state_label: self.state.label(t),
            tone: self.state.tone(),
            own_enabled: self.own_enabled,
            blocker: self.blocker_text(t),
            chain: self
                .chain
                .iter()
                .map(|(link, on)| ChainStepView {
                    label: link.label(t),
                    on: *on,
                    value_label: if *on { t.chain_on } else { t.chain_off },
                })
                .collect(),
        }
    }
}

/// JSON shape sent to the browser so it can refresh status displays in place.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct EntityStatusView {
    pub(crate) kind: &'static str,
    pub(crate) id: String,
    pub(crate) state: &'static str,
    pub(crate) state_label: &'static str,
    pub(crate) tone: &'static str,
    pub(crate) own_enabled: bool,
    pub(crate) blocker: Option<String>,
    pub(crate) chain: Vec<ChainStepView>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChainStepView {
    pub(crate) label: &'static str,
    pub(crate) on: bool,
    pub(crate) value_label: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use dynamo_module_kit::{ModuleCategory, ModuleDescriptor};

    fn module_state(installed: bool, deployment: bool, guild: bool) -> ResolvedModuleState {
        ResolvedModuleState {
            module: ModuleDescriptor {
                id: "stock",
                display_name: "Stock",
                description: "",
                category: ModuleCategory::Stocks,
                enabled_by_default: true,
                required_intents_bits: 0,
            },
            installed,
            deployment_enabled: deployment,
            guild_enabled: guild,
            effective_enabled: installed && deployment && guild,
        }
    }

    #[test]
    fn guild_switch_on_but_deployment_off_is_blocked_by_deployment() {
        let status = EntityStatus::for_module(Scope::Guild(1), &module_state(true, false, true));
        assert_eq!(status.state, EntityState::Blocked);
        assert!(status.own_enabled);
        assert_eq!(status.blocker, Some(Blocker::Deployment));
        assert_eq!(status.chain.len(), 3);
    }

    #[test]
    fn guild_switch_off_is_off_without_blocker() {
        let status = EntityStatus::for_module(Scope::Guild(1), &module_state(true, false, false));
        assert_eq!(status.state, EntityState::Off);
        assert_eq!(status.blocker, None);
    }

    #[test]
    fn deployment_scope_ignores_guild_switch() {
        let status = EntityStatus::for_module(Scope::Deployment, &module_state(true, true, false));
        assert_eq!(status.state, EntityState::Active);
        assert_eq!(status.chain.len(), 2);
        let status = EntityStatus::for_module(Scope::Deployment, &module_state(false, true, true));
        assert_eq!(status.state, EntityState::Blocked);
        assert_eq!(status.blocker, Some(Blocker::NotInstalled));
    }

    #[test]
    fn view_is_localized() {
        let status = EntityStatus::for_module(Scope::Guild(1), &module_state(true, false, true));
        let view = status.to_view(EntityKind::Module, "stock", Locale::Ko);
        assert_eq!(view.state_label, "차단됨");
        assert_eq!(
            view.blocker.as_deref(),
            Some("차단 사유: 봇 전체에서 비활성")
        );
        assert_eq!(view.tone, "warn");
    }
}
