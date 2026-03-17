use serde::Serialize;

pub type DiscordCommand<Data = (), Err = anyhow::Error> = poise::Command<Data, Err>;
pub type GatewayIntents = poise::serenity_prelude::GatewayIntents;

#[derive(Debug, Clone, Copy, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ModuleCategory {
    Core,
    Info,
    Currency,
    Utility,
    Moderation,
    Ticket,
    Suggestion,
    Giveaway,
    GameInfo,
    Stocks,
    Dashboard,
    Operations,
}

#[derive(Debug, Clone, Copy)]
pub struct ModuleManifest {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub category: ModuleCategory,
    pub enabled_by_default: bool,
    pub required_intents: GatewayIntents,
}

impl ModuleManifest {
    pub const fn new(
        id: &'static str,
        display_name: &'static str,
        description: &'static str,
        category: ModuleCategory,
        enabled_by_default: bool,
        required_intents: GatewayIntents,
    ) -> Self {
        Self {
            id,
            display_name,
            description,
            category,
            enabled_by_default,
            required_intents,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub category: ModuleCategory,
    pub enabled_by_default: bool,
    pub required_intents_bits: u64,
}

impl From<ModuleManifest> for ModuleDescriptor {
    fn from(value: ModuleManifest) -> Self {
        Self {
            id: value.id,
            display_name: value.display_name,
            description: value.description,
            category: value.category,
            enabled_by_default: value.enabled_by_default,
            required_intents_bits: value.required_intents.bits(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ModuleCatalog {
    pub entries: Vec<ModuleCatalogEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleCatalogEntry {
    pub module: ModuleDescriptor,
    pub settings: SettingsSchema,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CommandCatalog {
    pub entries: Vec<CommandCatalogEntry>,
}

impl CommandCatalog {
    pub fn find_by_id(&self, command_id: &str) -> Option<&CommandCatalogEntry> {
        self.entries
            .iter()
            .find(|entry| entry.command.id == command_id)
    }

    pub fn find_by_qualified_name(&self, qualified_name: &str) -> Option<&CommandCatalogEntry> {
        self.entries
            .iter()
            .find(|entry| entry.command.qualified_name == qualified_name)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandCatalogEntry {
    pub command: CommandDescriptor,
    pub settings: SettingsSchema,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandDescriptor {
    pub id: String,
    pub module_id: &'static str,
    pub module_display_name: &'static str,
    pub top_level_name: String,
    pub display_name: String,
    pub qualified_name: String,
    pub category: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct SettingsSchema {
    pub sections: Vec<SettingsSection>,
}

impl SettingsSchema {
    pub fn empty() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsSection {
    pub id: &'static str,
    pub title: &'static str,
    pub description: Option<&'static str>,
    pub fields: Vec<SettingsField>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsField {
    pub key: &'static str,
    pub label: &'static str,
    pub help_text: Option<&'static str>,
    pub required: bool,
    pub kind: SettingsFieldKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingsFieldKind {
    Toggle,
    Text,
    Integer,
    Select { options: Vec<SettingOption> },
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingOption {
    pub label: &'static str,
    pub value: &'static str,
}

pub trait Module<Data = (), Err = anyhow::Error>: Send + Sync {
    fn manifest(&self) -> ModuleManifest;
    fn commands(&self) -> Vec<DiscordCommand<Data, Err>>;

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema::empty()
    }

    fn command_settings_schema(&self, _command_id: &str) -> SettingsSchema {
        SettingsSchema::empty()
    }
}
