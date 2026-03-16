use std::{collections::BTreeMap, fmt};

use crate::{
    CommandCatalog, DeploymentSettings, GatewayIntents, GuildSettings, ModuleCatalog,
    resolve_command_states, resolve_module_states,
};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum StartupStatus {
    Ok,
    Warn,
    Error,
}

impl StartupStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for StartupStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct StartupPhase {
    pub name: &'static str,
    pub status: StartupStatus,
    pub summary: String,
    pub details: Vec<(String, String)>,
}

impl StartupPhase {
    pub fn new(name: &'static str, status: StartupStatus, summary: impl Into<String>) -> Self {
        Self {
            name,
            status,
            summary: summary.into(),
            details: Vec::new(),
        }
    }

    pub fn detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.push((key.into(), value.into()));
        self
    }
}

#[derive(Debug, Clone)]
pub struct StartupReport {
    pub process: &'static str,
    pub phases: Vec<StartupPhase>,
}

impl StartupReport {
    pub fn new(process: &'static str) -> Self {
        Self {
            process,
            phases: Vec::new(),
        }
    }

    pub fn add_phase(&mut self, phase: StartupPhase) {
        self.phases.push(phase);
    }

    pub fn overall_status(&self) -> StartupStatus {
        self.phases
            .iter()
            .map(|phase| phase.status)
            .max()
            .unwrap_or(StartupStatus::Ok)
    }

    pub fn log(&self) {
        let rendered = self.render();
        match self.overall_status() {
            StartupStatus::Ok => tracing::info!("{rendered}"),
            StartupStatus::Warn => tracing::warn!("{rendered}"),
            StartupStatus::Error => tracing::error!("{rendered}"),
        }
    }

    pub fn render(&self) -> String {
        let mut lines = vec![
            format!(
                "[startup:{}] overall={} phases={}",
                self.process,
                self.overall_status(),
                self.phases.len()
            ),
            render_table(
                &["phase", "stat", "summary"],
                &self
                    .phases
                    .iter()
                    .map(|phase| {
                        vec![
                            phase.name.to_string(),
                            phase.status.as_str().to_string(),
                            phase.summary.clone(),
                        ]
                    })
                    .collect::<Vec<_>>(),
                &[14, 7, 84],
            ),
        ];

        for phase in &self.phases {
            if phase.details.is_empty() {
                continue;
            }

            lines.push(String::new());
            lines.push(format!("[{} details]", phase.name));
            lines.push(render_detail_block(&phase.details));
        }

        lines.join("\n")
    }
}

#[derive(Debug, Clone, Default)]
pub struct CatalogStartupSummary {
    pub module_count: usize,
    pub module_ids: Vec<String>,
    pub discovered_leaf_command_count: usize,
    pub per_module_command_counts: Vec<(String, usize)>,
    pub per_category_command_counts: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Default)]
pub struct ScopeStartupSummary {
    pub discovered_leaf_command_count: usize,
    pub active_command_count: usize,
    pub filtered_command_count: usize,
    pub active_module_count: usize,
    pub active_module_ids: Vec<String>,
    pub disabled_module_count: usize,
    pub disabled_command_count: usize,
}

pub fn catalog_startup_summary(
    module_catalog: &ModuleCatalog,
    command_catalog: &CommandCatalog,
) -> CatalogStartupSummary {
    let mut per_module_command_counts = BTreeMap::new();
    let mut per_category_command_counts = BTreeMap::new();

    for entry in &command_catalog.entries {
        *per_module_command_counts
            .entry(entry.command.module_id.to_string())
            .or_insert(0usize) += 1;
        *per_category_command_counts
            .entry(
                entry
                    .command
                    .category
                    .clone()
                    .unwrap_or_else(|| entry.command.module_display_name.to_string()),
            )
            .or_insert(0usize) += 1;
    }

    CatalogStartupSummary {
        module_count: module_catalog.entries.len(),
        module_ids: module_catalog
            .entries
            .iter()
            .map(|entry| entry.module.id.to_string())
            .collect(),
        discovered_leaf_command_count: command_catalog.entries.len(),
        per_module_command_counts: per_module_command_counts.into_iter().collect(),
        per_category_command_counts: per_category_command_counts.into_iter().collect(),
    }
}

pub fn scope_startup_summary(
    module_catalog: &ModuleCatalog,
    command_catalog: &CommandCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> ScopeStartupSummary {
    let resolved_modules = resolve_module_states(module_catalog, deployment, guild);
    let resolved_commands =
        resolve_command_states(module_catalog, command_catalog, deployment, guild);

    ScopeStartupSummary {
        discovered_leaf_command_count: resolved_commands.len(),
        active_command_count: resolved_commands
            .iter()
            .filter(|state| state.effective_enabled)
            .count(),
        filtered_command_count: resolved_commands
            .iter()
            .filter(|state| !state.effective_enabled)
            .count(),
        active_module_count: resolved_modules
            .iter()
            .filter(|state| state.effective_enabled)
            .count(),
        active_module_ids: resolved_modules
            .iter()
            .filter(|state| state.effective_enabled)
            .map(|state| state.module.id.to_string())
            .collect(),
        disabled_module_count: resolved_modules
            .iter()
            .filter(|state| !state.effective_enabled)
            .count(),
        disabled_command_count: resolved_commands
            .iter()
            .filter(|state| !state.effective_enabled)
            .count(),
    }
}

pub fn format_kv_list(entries: &[(String, usize)]) -> String {
    if entries.is_empty() {
        return "none".to_string();
    }

    entries
        .iter()
        .map(|(key, value)| format!("{key}:{value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_table(headers: &[&str], rows: &[Vec<String>], max_widths: &[usize]) -> String {
    let column_count = headers.len();
    let mut widths = headers
        .iter()
        .map(|header| header.len())
        .collect::<Vec<_>>();

    for row in rows {
        for (index, cell) in row.iter().enumerate().take(column_count) {
            widths[index] = widths[index].max(visible_width(cell));
        }
    }

    for (index, width) in widths.iter_mut().enumerate() {
        if let Some(max_width) = max_widths.get(index) {
            *width = (*width).min(*max_width);
        }
    }

    let border = format!(
        "+{}+",
        widths
            .iter()
            .map(|width| "-".repeat(*width + 2))
            .collect::<Vec<_>>()
            .join("+")
    );

    let mut lines = vec![border.clone()];
    lines.push(render_table_row(
        &headers
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>(),
        &widths,
    ));
    lines.push(border.clone());
    for row in rows {
        lines.extend(render_wrapped_rows(row, &widths));
    }
    lines.push(border);
    lines.join("\n")
}

fn render_detail_block(details: &[(String, String)]) -> String {
    let key_width = details
        .iter()
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(12, 28);
    let value_width = 92usize.saturating_sub(key_width);
    let mut lines = Vec::new();

    for (key, value) in details {
        let wrapped = wrap_cell(value, value_width.max(24));
        for (index, line) in wrapped.into_iter().enumerate() {
            if index == 0 {
                lines.push(format!("  {:width$}  {}", key, line, width = key_width));
            } else {
                lines.push(format!("  {:width$}  {}", "", line, width = key_width));
            }
        }
    }

    lines.join("\n")
}

fn render_wrapped_rows(row: &[String], widths: &[usize]) -> Vec<String> {
    let wrapped_cells = row
        .iter()
        .enumerate()
        .map(|(index, cell)| wrap_cell(cell, widths[index]))
        .collect::<Vec<_>>();
    let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
    let mut lines = Vec::with_capacity(row_height);

    for line_index in 0..row_height {
        let columns = wrapped_cells
            .iter()
            .zip(widths.iter())
            .map(|(cell_lines, width)| {
                cell_lines
                    .get(line_index)
                    .cloned()
                    .unwrap_or_else(|| "".to_string())
                    .chars()
                    .collect::<String>()
                    .chars()
                    .take(*width)
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        lines.push(render_table_row(&columns, widths));
    }

    lines
}

fn render_table_row(columns: &[String], widths: &[usize]) -> String {
    let cells = columns
        .iter()
        .zip(widths.iter())
        .map(|(column, width)| format!(" {} ", pad_visible(column, *width)))
        .collect::<Vec<_>>()
        .join("|");
    format!("|{}|", cells)
}

fn wrap_cell(value: &str, width: usize) -> Vec<String> {
    if value.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for word in value.split(' ') {
        if current.is_empty() {
            if word.chars().count() <= width {
                current.push_str(word);
            } else {
                lines.extend(hard_wrap(word, width));
            }
            continue;
        }

        let next_len = current.chars().count() + 1 + word.chars().count();
        if next_len <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            if word.chars().count() <= width {
                current = word.to_string();
            } else {
                lines.extend(hard_wrap(word, width));
                current = String::new();
            }
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn hard_wrap(value: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    for ch in value.chars() {
        current.push(ch);
        if current.chars().count() >= width {
            lines.push(current);
            current = String::new();
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn visible_width(value: &str) -> usize {
    strip_ansi(value).chars().count()
}

fn pad_visible(value: &str, width: usize) -> String {
    let visible = visible_width(value);
    if visible >= width {
        return value.to_string();
    }

    format!("{value}{}", " ".repeat(width - visible))
}

fn strip_ansi(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for inner in chars.by_ref() {
                if inner == 'm' {
                    break;
                }
            }
            continue;
        }

        result.push(ch);
    }

    result
}

pub fn format_gateway_intents(intents: GatewayIntents) -> String {
    let mappings = [
        ("GUILDS", GatewayIntents::GUILDS),
        ("GUILD_MEMBERS", GatewayIntents::GUILD_MEMBERS),
        ("GUILD_MODERATION", GatewayIntents::GUILD_MODERATION),
        (
            "GUILD_EMOJIS_AND_STICKERS",
            GatewayIntents::GUILD_EMOJIS_AND_STICKERS,
        ),
        ("GUILD_INTEGRATIONS", GatewayIntents::GUILD_INTEGRATIONS),
        ("GUILD_WEBHOOKS", GatewayIntents::GUILD_WEBHOOKS),
        ("GUILD_INVITES", GatewayIntents::GUILD_INVITES),
        ("GUILD_VOICE_STATES", GatewayIntents::GUILD_VOICE_STATES),
        ("GUILD_PRESENCES", GatewayIntents::GUILD_PRESENCES),
        ("GUILD_MESSAGES", GatewayIntents::GUILD_MESSAGES),
        (
            "GUILD_MESSAGE_REACTIONS",
            GatewayIntents::GUILD_MESSAGE_REACTIONS,
        ),
        ("GUILD_MESSAGE_TYPING", GatewayIntents::GUILD_MESSAGE_TYPING),
        ("DIRECT_MESSAGES", GatewayIntents::DIRECT_MESSAGES),
        (
            "DIRECT_MESSAGE_REACTIONS",
            GatewayIntents::DIRECT_MESSAGE_REACTIONS,
        ),
        (
            "DIRECT_MESSAGE_TYPING",
            GatewayIntents::DIRECT_MESSAGE_TYPING,
        ),
        ("MESSAGE_CONTENT", GatewayIntents::MESSAGE_CONTENT),
        (
            "GUILD_SCHEDULED_EVENTS",
            GatewayIntents::GUILD_SCHEDULED_EVENTS,
        ),
        (
            "AUTO_MODERATION_CONFIGURATION",
            GatewayIntents::AUTO_MODERATION_CONFIGURATION,
        ),
        (
            "AUTO_MODERATION_EXECUTION",
            GatewayIntents::AUTO_MODERATION_EXECUTION,
        ),
        ("GUILD_MESSAGE_POLLS", GatewayIntents::GUILD_MESSAGE_POLLS),
        ("DIRECT_MESSAGE_POLLS", GatewayIntents::DIRECT_MESSAGE_POLLS),
    ];

    let names = mappings
        .into_iter()
        .filter_map(|(name, flag)| intents.contains(flag).then_some(name))
        .collect::<Vec<_>>();

    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}
