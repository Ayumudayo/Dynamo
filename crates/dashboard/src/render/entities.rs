//! Module and command lists, the overview, command sync, and the audit log for
//! the deployment and guild settings pages.

use chrono::{DateTime, Utc};
use dynamo_enablement::{ResolvedCommandState, ResolvedModuleState};
use dynamo_module_kit::{CommandCatalog, CommandCatalogEntry, ModuleCatalog, ModuleCategory};
use dynamo_ops::{
    DashboardAuditAction, DashboardAuditEntityType, DashboardAuditLogEntry, DashboardAuditLogPage,
    DashboardAuditScope,
};
use maud::{Markup, html};

use crate::i18n::{Locale, fill};
use crate::status::{EntityKind, EntityState, EntityStatus, Scope};
use crate::{
    CommandSyncDisplayState, CommandSyncMeta, CommandSyncPanel, page_query_for_logs,
    page_query_for_owner_logs,
};

/// One module or command as shown in a list, an overview, or a dialog.
#[derive(Debug, Clone)]
pub(crate) struct EntityRow {
    pub(crate) kind: EntityKind,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) category: Option<(String, String)>,
    pub(crate) has_settings: bool,
    pub(crate) status: EntityStatus,
    pub(crate) search: String,
}

fn module_category_key(category: ModuleCategory) -> &'static str {
    match category {
        ModuleCategory::Core => "core",
        ModuleCategory::Info => "info",
        ModuleCategory::Currency => "currency",
        ModuleCategory::Utility => "utility",
        ModuleCategory::Moderation => "moderation",
        ModuleCategory::Ticket => "ticket",
        ModuleCategory::Suggestion => "suggestion",
        ModuleCategory::Giveaway => "giveaway",
        ModuleCategory::GameInfo => "game_info",
        ModuleCategory::Stocks => "stocks",
        ModuleCategory::Dashboard => "dashboard",
        ModuleCategory::Operations => "operations",
    }
}

fn module_category_label(category: ModuleCategory, locale: Locale) -> &'static str {
    let t = locale.t();
    match category {
        ModuleCategory::Core => t.mod_cat_core,
        ModuleCategory::Info => t.mod_cat_info,
        ModuleCategory::Currency => t.mod_cat_currency,
        ModuleCategory::Utility => t.mod_cat_utility,
        ModuleCategory::Moderation => t.mod_cat_moderation,
        ModuleCategory::Ticket => t.mod_cat_ticket,
        ModuleCategory::Suggestion => t.mod_cat_suggestion,
        ModuleCategory::Giveaway => t.mod_cat_giveaway,
        ModuleCategory::GameInfo => t.mod_cat_game_info,
        ModuleCategory::Stocks => t.mod_cat_stocks,
        ModuleCategory::Dashboard => t.mod_cat_dashboard,
        ModuleCategory::Operations => t.mod_cat_operations,
    }
}

pub(crate) fn module_rows(
    catalog: &ModuleCatalog,
    resolved: &[ResolvedModuleState],
    scope: Scope,
    locale: Locale,
) -> Vec<EntityRow> {
    catalog
        .entries
        .iter()
        .filter_map(|entry| {
            let state = resolved
                .iter()
                .find(|state| state.module.id == entry.module.id)?;
            let name = locale.catalog(entry.module.display_name).to_string();
            let description = locale.catalog(entry.module.description).to_string();
            let category_key = module_category_key(entry.module.category).to_string();
            let category_label = module_category_label(entry.module.category, locale).to_string();
            Some(EntityRow {
                kind: EntityKind::Module,
                id: entry.module.id.to_string(),
                search: search_text(&[
                    entry.module.id,
                    entry.module.display_name,
                    &name,
                    entry.module.description,
                    &description,
                    &category_label,
                ]),
                name,
                description,
                category: Some((category_key, category_label)),
                has_settings: !entry.settings.sections.is_empty(),
                status: EntityStatus::for_module(scope, state),
            })
        })
        .collect()
}

pub(crate) fn command_rows(
    catalog: &CommandCatalog,
    resolved: &[ResolvedCommandState],
    scope: Scope,
    locale: Locale,
) -> Vec<EntityRow> {
    let t = locale.t();
    catalog
        .entries
        .iter()
        .filter_map(|entry| {
            let state = resolved
                .iter()
                .find(|state| state.command.id == entry.command.id)?;
            let source_description = entry.command.description.as_deref().unwrap_or_default();
            let description = if source_description.is_empty() {
                t.no_description.to_string()
            } else {
                locale.catalog(source_description).to_string()
            };
            let category_label = locale.catalog(&command_category_source(entry)).to_string();
            Some(EntityRow {
                kind: EntityKind::Command,
                id: entry.command.id.clone(),
                search: search_text(&[
                    &entry.command.id,
                    &entry.command.display_name,
                    &entry.command.qualified_name,
                    source_description,
                    &description,
                    &category_label,
                ]),
                name: entry.command.display_name.clone(),
                description,
                category: Some((command_category_key(entry), category_label)),
                has_settings: !entry.settings.sections.is_empty(),
                status: EntityStatus::for_command(scope, state),
            })
        })
        .collect()
}

fn search_text(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .map(|part| part.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn command_category_source(entry: &CommandCatalogEntry) -> String {
    entry
        .command
        .category
        .clone()
        .unwrap_or_else(|| entry.command.module_display_name.to_string())
}

pub(crate) fn command_category_key(entry: &CommandCatalogEntry) -> String {
    command_category_source(entry)
        .to_ascii_lowercase()
        .replace(' ', "-")
}

pub(crate) fn status_key(value: &str) -> String {
    value.replace(':', "-")
}

/// Percent-encodes one URL path segment (RFC 3986 unreserved characters pass).
pub(crate) fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub(crate) fn modal_id(scope: Scope, kind: EntityKind, id: &str) -> String {
    format!("modal-{}-{}-{}", scope.key(), kind.key(), status_key(id))
}

pub(crate) fn patch_url(scope: Scope, kind: EntityKind, id: &str) -> String {
    let id = encode_path_segment(id);
    match (scope, kind) {
        (Scope::Deployment, EntityKind::Module) => format!("/api/deployment-settings/{id}"),
        (Scope::Deployment, EntityKind::Command) => {
            format!("/api/deployment-command-settings/{id}")
        }
        (Scope::Guild(guild_id), EntityKind::Module) => {
            format!("/api/guild-settings/{guild_id}/{id}")
        }
        (Scope::Guild(guild_id), EntityKind::Command) => {
            format!("/api/guild-command-settings/{guild_id}/{id}")
        }
    }
}

pub(crate) fn fragment_url(scope: Scope, kind: EntityKind, id: &str) -> String {
    format!(
        "{}/settings/{}/{}",
        scope.base_path(),
        kind.key(),
        encode_path_segment(id)
    )
}

pub(crate) fn state_url(scope: Scope) -> String {
    match scope {
        Scope::Deployment => "/api/deployment-state".to_string(),
        Scope::Guild(guild_id) => format!("/api/guild-state/{guild_id}"),
    }
}

/// The badge, activation chain, and blocker shared by cards and dialogs. The
/// `data-*` hooks let the browser refresh them from `/api/*-state`.
pub(crate) fn render_status_parts(row_status: &EntityStatus, locale: Locale) -> Markup {
    let t = locale.t();
    let blocker = row_status.blocker_text(t);
    html! {
        ul.chain aria-label=(t.chain_label) data-chain {
            @for (link, on) in &row_status.chain {
                li.chain-step data-on=(on) {
                    (link.label(t))
                    span.visually-hidden { ": " (if *on { t.chain_on } else { t.chain_off }) }
                }
            }
        }
        p.blocker data-blocker hidden[blocker.is_none()] { (blocker.unwrap_or_default()) }
    }
}

pub(crate) fn render_status_badge(status: &EntityStatus, locale: Locale) -> Markup {
    html! {
        span.badge data-tone=(status.state.tone()) data-status-badge { (status.state.label(locale.t())) }
    }
}

fn render_entity_card(row: &EntityRow, scope: Scope, locale: Locale) -> Markup {
    let t = locale.t();
    let key = status_key(&row.id);
    let kind = row.kind.key();
    let toggle_label = match scope {
        Scope::Deployment => fill(t.toggle_label_deployment, &[("name", &row.name)]),
        Scope::Guild(_) => fill(t.toggle_label_guild, &[("name", &row.name)]),
    };
    html! {
        article.entity-card
            data-testid={ (kind) "-card-" (key) }
            data-filter-item
            data-entity-status
            data-entity-kind=(kind)
            data-entity-id=(row.id)
            data-entity-name=(row.name)
            data-state=(row.status.state.key())
            data-search=(row.search)
            data-category=[row.category.as_ref().map(|(key, _)| key.as_str())]
            data-patch-url=(patch_url(scope, row.kind, &row.id)) {
            div.entity-card-head {
                div.entity-card-title {
                    h3 { (row.name) }
                    (render_status_badge(&row.status, locale))
                }
                label.switch {
                    input type="checkbox"
                        data-testid={ (kind) "-toggle-" (key) }
                        data-action="toggle-entity"
                        aria-label=(toggle_label)
                        checked[row.status.own_enabled];
                    span.switch-track aria-hidden="true" {}
                }
            }
            p.entity-card-desc { (row.description) }
            @if let Some((_, label)) = &row.category {
                p.entity-card-category { (label) }
            }
            (render_status_parts(&row.status, locale))
            @if scope == Scope::Deployment || row.has_settings {
                @let settings_label = if scope == Scope::Deployment {
                    t.settings_button_deployment
                } else {
                    t.settings_button
                };
                div.entity-card-foot {
                    button.button.button-secondary.button-compact type="button"
                        data-testid={ (kind) "-settings-button-" (key) }
                        data-action="open-settings"
                        data-modal-id=(modal_id(scope, row.kind, &row.id))
                        data-fragment-url=(fragment_url(scope, row.kind, &row.id))
                        aria-label=(fill(t.settings_button_label, &[("name", &row.name)])) {
                        (settings_label)
                    }
                }
            }
        }
    }
}

fn state_counts(rows: &[EntityRow]) -> [(Option<EntityState>, usize); 4] {
    let count = |state| rows.iter().filter(|row| row.status.state == state).count();
    [
        (None, rows.len()),
        (Some(EntityState::Active), count(EntityState::Active)),
        (Some(EntityState::Off), count(EntityState::Off)),
        (Some(EntityState::Blocked), count(EntityState::Blocked)),
    ]
}

/// Renders the module or command list with search, status filters and, for
/// commands, the sync panel and category filters.
pub(crate) fn render_entity_section(
    kind: EntityKind,
    scope: Scope,
    rows: &[EntityRow],
    sync_panel: Option<&CommandSyncPanel>,
    locale: Locale,
) -> Markup {
    let t = locale.t();
    let kind_key = kind.key();
    let (heading, search_label, empty) = match kind {
        EntityKind::Module => (t.tab_modules, t.search_modules, t.no_modules_match),
        EntityKind::Command => (t.tab_commands, t.search_commands, t.no_commands_match),
    };
    let filter_id = format!("{kind_key}-filter");
    let status_id = format!("{kind_key}-filter-status");
    let mut categories: Vec<(String, String)> = Vec::new();
    for row in rows {
        if let Some(category) = &row.category
            && !categories.iter().any(|(key, _)| key == &category.0)
        {
            categories.push(category.clone());
        }
    }
    html! {
        section.panel
            id={ (kind_key) "s" }
            data-testid={ (scope.key()) "-" (kind_key) "s-section" }
            data-filter-scope=(kind_key)
            aria-labelledby={ (kind_key) "s-heading" } {
            div.panel-head {
                h2 id={ (kind_key) "s-heading" } { (heading) " " span.count-badge { (rows.len()) } }
            }
            @if let Some(panel) = sync_panel {
                div.category-row { (render_command_sync_panel(panel, locale)) }
            }
            div.list-toolbar {
                div.search-field {
                    label.visually-hidden for=(filter_id) { (search_label) }
                    input.search-input id=(filter_id) data-testid=(filter_id) type="search"
                        data-filter-input placeholder=(search_label) autocomplete="off"
                        aria-describedby=(status_id);
                }
                div.chip-group role="group" aria-label=(t.filter_status_label) {
                    @for (state, count) in state_counts(rows) {
                        @let key = state.map(EntityState::key).unwrap_or("all");
                        @let label = state.map(|state| state.label(t)).unwrap_or(t.filter_all);
                        button.chip type="button" data-state-filter=(key)
                            aria-pressed=(if state.is_none() { "true" } else { "false" }) {
                            (label) " " span.count-badge data-state-count=(key) { (count) }
                        }
                    }
                }
            }
            @if !categories.is_empty() {
                div.chip-group.category-row.command-category-row role="group" aria-label=(t.filter_category_label) {
                    button.chip.command-tab type="button" data-testid={ (kind_key) "-tab-all" }
                        data-category-filter="all" aria-pressed="true" { (t.filter_all) }
                    @for (key, label) in &categories {
                        button.chip.command-tab type="button" data-testid={ (kind_key) "-tab-" (key) }
                            data-category-filter=(key) aria-pressed="false" { (label) }
                    }
                }
            }
            p.filter-feedback id=(status_id) data-filter-status role="status" aria-live="polite" aria-atomic="true" {}
            p.empty-state.filter-empty data-filter-empty hidden { (empty) }
            div.entity-grid data-testid={ (kind_key) "-card-grid" } {
                @for row in rows { (render_entity_card(row, scope, locale)) }
            }
        }
    }
}

pub(crate) fn sync_tone(state: CommandSyncDisplayState) -> &'static str {
    match state {
        CommandSyncDisplayState::InSync => "success",
        CommandSyncDisplayState::Required => "warn",
        CommandSyncDisplayState::Pending => "info",
        CommandSyncDisplayState::Failed => "danger",
        CommandSyncDisplayState::Unsupported => "neutral",
    }
}

pub(crate) fn render_command_sync_panel(panel: &CommandSyncPanel, locale: Locale) -> Markup {
    html! {
        div.callout data-tone=(sync_tone(panel.state)) data-testid="command-sync-panel" data-sync-panel
            role="region" aria-labelledby="command-sync-title" {
            div.callout-body {
                h3 #command-sync-title data-sync-title { (panel.title) }
                p data-sync-body { (panel.message) }
                ul.callout-meta data-sync-meta hidden[panel.meta.is_empty()] {
                    @for meta in &panel.meta {
                        li {
                            @match meta {
                                CommandSyncMeta::Time(label, at) => { (label) " " (render_time(*at)) }
                                CommandSyncMeta::Text(text) => { (text) }
                            }
                        }
                    }
                }
                @if let Some(raw_error) = &panel.raw_error {
                    details.sync-error-details {
                        summary { (locale.t().sync_error_details_label) }
                        code.mono { (raw_error) }
                    }
                }
            }
            @if let Some(action) = &panel.action {
                div.callout-actions {
                    button.button.button-primary.button-compact type="button"
                        data-action="request-sync" data-sync-url=(action.url) {
                        (action.label)
                    }
                }
            }
        }
    }
}

/// A timestamp the browser rewrites into the viewer's locale and time zone.
pub(crate) fn render_time(at: DateTime<Utc>) -> Markup {
    html! {
        time datetime=(at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)) data-local-time {
            (at.format("%Y-%m-%d %H:%M UTC"))
        }
    }
}

/// Header shown on every tab of the deployment and guild pages.
pub(crate) fn render_scope_header(
    title: &str,
    subtitle: &str,
    icon: Option<Markup>,
    badge: Option<(&str, &str)>,
) -> Markup {
    html! {
        div.page-header {
            @if let Some(icon) = icon { (icon) }
            div.page-header-text {
                h1 { (title) }
                p.lede { (subtitle) }
            }
            @if let Some((tone, label)) = badge {
                div.page-header-meta { span.badge data-tone=(tone) { (label) } }
            }
        }
    }
}

/// Wraps a deployment or guild page with the data the browser needs to save
/// changes and refresh state.
pub(crate) fn render_scope_page(
    scope: Scope,
    header: Markup,
    tabs: Markup,
    active: Markup,
) -> Markup {
    html! {
        div.scope-page
            data-scope=(scope.key())
            data-guild-id=[scope.guild_id()]
            data-state-url=(state_url(scope)) {
            (header)
            (tabs)
            (active)
        }
    }
}

pub(crate) struct OverviewData<'a> {
    pub(crate) scope: Scope,
    pub(crate) modules: &'a [EntityRow],
    pub(crate) commands: &'a [EntityRow],
    pub(crate) sync_panel: Option<&'a CommandSyncPanel>,
    pub(crate) recent: &'a [DashboardAuditLogEntry],
    /// (settings state, test id suffix, localized text)
    pub(crate) settings_notice: Option<(&'a str, &'a str, &'a str)>,
    pub(crate) runtime_notices: Option<Markup>,
    pub(crate) extra_panel: Option<Markup>,
    pub(crate) entity_names: &'a dyn Fn(DashboardAuditEntityType, &str) -> String,
}

pub(crate) fn render_overview(data: &OverviewData<'_>, locale: Locale) -> Markup {
    let t = locale.t();
    let base = data.scope.base_path();
    let active = |rows: &[EntityRow]| {
        rows.iter()
            .filter(|row| row.status.state == EntityState::Active)
            .count()
    };
    let blocked = data
        .modules
        .iter()
        .chain(data.commands.iter())
        .filter(|row| row.status.state == EntityState::Blocked)
        .collect::<Vec<_>>();
    let blocked_modules = blocked.iter().any(|row| row.kind == EntityKind::Module);
    let blocked_commands = blocked.iter().any(|row| row.kind == EntityKind::Command);
    let (testid, settings_state) = match data.scope {
        Scope::Guild(_) => (
            "guild-runtime-summary",
            data.settings_notice.map(|(state, _, _)| state),
        ),
        Scope::Deployment => ("deployment-overview-section", None),
    };
    html! {
        section.stack data-testid=(testid) data-settings-state=[settings_state] aria-labelledby="overview-heading" {
            h2.visually-hidden #overview-heading { (t.overview_heading) }
            @if let Some((_, testid_suffix, text)) = data.settings_notice {
                p.notice data-testid={ "guild-settings-" (testid_suffix) } { (text) }
            }
            div.overview-grid {
                @if let Some(panel) = data.sync_panel {
                    (render_command_sync_panel(panel, locale))
                }
                article.panel {
                    div.panel-head { h3 { (t.overview_heading) } }
                    (render_metric(t.overview_active_modules, active(data.modules), data.modules.len(), locale))
                    (render_metric(t.overview_active_commands, active(data.commands), data.commands.len(), locale))
                }
                article.panel {
                    div.panel-head {
                        h3 { (t.overview_blocked_title) }
                        @if !blocked.is_empty() {
                            span.badge data-tone="warn" { (fill(t.overview_blocked_count, &[("count", &blocked.len())])) }
                        }
                    }
                    @if blocked.is_empty() {
                        p.muted { (t.overview_blocked_empty) }
                    } @else {
                        ul.plain-list {
                            @for row in blocked.iter().take(6) {
                                li {
                                    span { (row.name) }
                                    span.muted { (row.status.blocker.map(|blocker| blocker.label(t)).unwrap_or_default()) }
                                }
                            }
                        }
                        div.panel-foot.panel-foot-actions {
                            @if blocked_modules {
                                a.button.button-secondary.button-compact href=(format!("{base}?tab=modules&state=blocked")) { (t.overview_view_modules) }
                            }
                            @if blocked_commands {
                                a.button.button-secondary.button-compact href=(format!("{base}?tab=commands&state=blocked")) { (t.overview_view_commands) }
                            }
                        }
                    }
                }
                article.panel {
                    div.panel-head { h3 { (t.overview_recent_title) } }
                    @if data.recent.is_empty() {
                        p.muted { (t.overview_recent_empty) }
                    } @else {
                        ul.plain-list {
                            @for entry in data.recent {
                                li {
                                    span {
                                        strong { ((data.entity_names)(entry.entity_type, &entry.entity_id)) }
                                        " · " (localized_audit_summary(entry, locale))
                                    }
                                    span.muted data-relative-time=(entry.timestamp.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)) {
                                        (entry.timestamp.format("%Y-%m-%d"))
                                    }
                                }
                            }
                        }
                    }
                    div.panel-foot {
                        a.button.button-secondary.button-compact href=(format!("{base}?tab=logs")) { (t.overview_view_all) }
                    }
                }
                @if let Some(extra) = &data.extra_panel {
                    (extra)
                }
                @if let Some(notices) = &data.runtime_notices {
                    (notices)
                }
            }
        }
    }
}

fn render_metric(label: &str, active: usize, total: usize, locale: Locale) -> Markup {
    let t = locale.t();
    html! {
        div.metric {
            div.metric-row {
                span { (label) }
                strong { (fill(t.overview_ratio, &[("active", &active), ("total", &total)])) }
            }
            progress.meter value=(active) max=(total.max(1)) aria-label=(label) {}
        }
    }
}

pub(crate) fn audit_entity_label(
    entity_type: DashboardAuditEntityType,
    locale: Locale,
) -> &'static str {
    let t = locale.t();
    match entity_type {
        DashboardAuditEntityType::Module => t.audit_entity_module,
        DashboardAuditEntityType::Command => t.audit_entity_command,
        DashboardAuditEntityType::Guild => t.audit_entity_guild,
        DashboardAuditEntityType::Operations => t.audit_entity_operations,
    }
}

pub(crate) fn audit_action_label(action: DashboardAuditAction, locale: Locale) -> &'static str {
    let t = locale.t();
    match action {
        DashboardAuditAction::Toggle => t.audit_action_toggle,
        DashboardAuditAction::SaveSettings => t.audit_action_save,
        DashboardAuditAction::Sync => t.audit_action_sync,
        DashboardAuditAction::Reset => t.audit_action_reset,
        DashboardAuditAction::LeaveGuild => t.audit_action_leave_guild,
        DashboardAuditAction::BlockGuild => t.audit_action_block_guild,
        DashboardAuditAction::UnblockGuild => t.audit_action_unblock_guild,
        DashboardAuditAction::SendAnnouncement => t.audit_action_send_announcement,
    }
}

pub(crate) fn audit_scope_label(entry: &DashboardAuditLogEntry, locale: Locale) -> String {
    let t = locale.t();
    match entry.scope {
        DashboardAuditScope::Deployment => t.audit_scope_global.to_string(),
        DashboardAuditScope::Guild => {
            let id = entry
                .guild_id
                .map(|gid| gid.to_string())
                .unwrap_or_else(|| entry.entity_id.clone());
            fill(t.audit_scope_guild, &[("id", &id)])
        }
    }
}

/// Builds a localized description from the structured audit entry. Toggle
/// summaries written by this dashboard contain `installed=`/`enabled=` tokens;
/// other actions fall back to the action label or stored summary.
pub(crate) fn localized_audit_summary(entry: &DashboardAuditLogEntry, locale: Locale) -> String {
    let t = locale.t();
    match entry.action {
        DashboardAuditAction::SaveSettings => t.audit_settings_saved.to_string(),
        DashboardAuditAction::Toggle => {
            let token = |name: &str| {
                entry
                    .summary
                    .split(|c: char| c.is_whitespace() || c == '.' || c == ',')
                    .find_map(|part| part.strip_prefix(name))
                    .and_then(|value| value.parse::<bool>().ok())
            };
            let mut parts = Vec::new();
            if let Some(installed) = token("installed=") {
                parts.push(if installed {
                    t.audit_installed_on
                } else {
                    t.audit_installed_off
                });
            }
            if let Some(enabled) = token("enabled=") {
                parts.push(if enabled {
                    t.audit_enabled_on
                } else {
                    t.audit_enabled_off
                });
            }
            if parts.is_empty() {
                entry.summary.clone()
            } else {
                parts.join(" · ")
            }
        }
        DashboardAuditAction::Sync
        | DashboardAuditAction::Reset
        | DashboardAuditAction::LeaveGuild
        | DashboardAuditAction::BlockGuild
        | DashboardAuditAction::UnblockGuild
        | DashboardAuditAction::SendAnnouncement => {
            if entry.summary.trim().is_empty() {
                audit_action_label(entry.action, locale).to_string()
            } else {
                entry.summary.clone()
            }
        }
    }
}

pub(crate) fn render_audit_logs_section(
    base_path: &str,
    page: &DashboardAuditLogPage,
    entity_type: Option<DashboardAuditEntityType>,
    action: Option<DashboardAuditAction>,
    locale: Locale,
    entity_names: &dyn Fn(DashboardAuditEntityType, &str) -> String,
) -> Markup {
    render_audit_logs_inner(
        base_path,
        page,
        None,
        entity_type,
        action,
        locale,
        entity_names,
        &|next_page| {
            format!(
                "{base_path}{}",
                page_query_for_logs(entity_type, action, next_page)
            )
        },
    )
}

pub(crate) fn render_owner_audit_logs_section(
    base_path: &str,
    page: &DashboardAuditLogPage,
    log_scope: Option<DashboardAuditScope>,
    entity_type: Option<DashboardAuditEntityType>,
    action: Option<DashboardAuditAction>,
    locale: Locale,
    entity_names: &dyn Fn(DashboardAuditEntityType, &str) -> String,
) -> Markup {
    render_audit_logs_inner(
        base_path,
        page,
        Some(log_scope),
        entity_type,
        action,
        locale,
        entity_names,
        &|next_page| {
            format!(
                "{base_path}{}",
                page_query_for_owner_logs(log_scope, entity_type, action, next_page)
            )
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn render_audit_logs_inner(
    base_path: &str,
    page: &DashboardAuditLogPage,
    owner_scope_filter: Option<Option<DashboardAuditScope>>,
    entity_type: Option<DashboardAuditEntityType>,
    action: Option<DashboardAuditAction>,
    locale: Locale,
    entity_names: &dyn Fn(DashboardAuditEntityType, &str) -> String,
    page_href: &dyn Fn(u64) -> String,
) -> Markup {
    let t = locale.t();
    let is_owner = owner_scope_filter.is_some();
    let col_count = if is_owner { "6" } else { "5" };
    let page_total = page.total.max(1).div_ceil(page.page_size.max(1));
    html! {
        section.panel data-testid="logs-section" aria-labelledby="logs-heading" {
            div.panel-head { h2 #logs-heading { (t.logs_heading) } }
            form.logs-filters method="get" action=(base_path) {
                input type="hidden" name="tab" value="logs";
                @if let Some(log_scope) = owner_scope_filter {
                    div.field {
                        label for="logs-scope-filter" { (t.logs_filter_scope) }
                        select.toolbar-select #logs-scope-filter name="log_scope" data-testid="logs-scope-filter" {
                            option value="" selected[log_scope.is_none()] { (t.logs_scope_all) }
                            option value="deployment" selected[log_scope == Some(DashboardAuditScope::Deployment)] { (t.logs_scope_deployment) }
                            option value="guild" selected[log_scope == Some(DashboardAuditScope::Guild)] { (t.logs_scope_guild) }
                        }
                    }
                }
                div.field {
                    label for="logs-entity-filter" { (t.logs_filter_entity) }
                    select.toolbar-select #logs-entity-filter name="log_entity" data-testid="logs-entity-filter" {
                        option value="" selected[entity_type.is_none()] { (t.logs_all_entities) }
                        option value="module" selected[entity_type == Some(DashboardAuditEntityType::Module)] { (t.logs_modules) }
                        option value="command" selected[entity_type == Some(DashboardAuditEntityType::Command)] { (t.logs_commands) }
                        @if is_owner {
                            option value="guild" selected[entity_type == Some(DashboardAuditEntityType::Guild)] { (t.logs_guilds) }
                            option value="operations" selected[entity_type == Some(DashboardAuditEntityType::Operations)] { (t.logs_operations) }
                        }
                    }
                }
                div.field {
                    label for="logs-action-filter" { (t.logs_filter_action) }
                    select.toolbar-select #logs-action-filter name="log_action" data-testid="logs-action-filter" {
                        option value="" selected[action.is_none()] { (t.logs_all_actions) }
                        option value="toggle" selected[action == Some(DashboardAuditAction::Toggle)] { (t.logs_toggles) }
                        option value="save_settings" selected[action == Some(DashboardAuditAction::SaveSettings)] { (t.logs_saves) }
                        @if is_owner {
                            option value="sync" selected[action == Some(DashboardAuditAction::Sync)] { (t.audit_action_sync) }
                            option value="reset" selected[action == Some(DashboardAuditAction::Reset)] { (t.audit_action_reset) }
                            option value="leave_guild" selected[action == Some(DashboardAuditAction::LeaveGuild)] { (t.audit_action_leave_guild) }
                            option value="block_guild" selected[action == Some(DashboardAuditAction::BlockGuild)] { (t.audit_action_block_guild) }
                            option value="unblock_guild" selected[action == Some(DashboardAuditAction::UnblockGuild)] { (t.audit_action_unblock_guild) }
                            option value="send_announcement" selected[action == Some(DashboardAuditAction::SendAnnouncement)] { (t.audit_action_send_announcement) }
                        }
                    }
                }
                button.button.button-secondary type="submit" { (t.logs_apply) }
            }
            div.panel.logs-panel {
                table.logs-table data-testid="logs-table" {
                    colgroup {
                        col.col-when;
                        @if is_owner {
                            col.col-scope;
                        }
                        col.col-who; col.col-target; col.col-action; col.col-details;
                    }
                    thead {
                        tr {
                            th scope="col" { (t.logs_col_when) }
                            @if is_owner {
                                th scope="col" { (t.logs_col_scope) }
                            }
                            th scope="col" { (t.logs_col_who) }
                            th scope="col" { (t.logs_col_target) }
                            th scope="col" { (t.logs_col_action) }
                            th scope="col" { (t.logs_col_details) }
                        }
                    }
                    tbody {
                        @if page.entries.is_empty() {
                            tr { td.empty-cell colspan=(col_count) { (t.logs_empty) } }
                        }
                        @for (index, entry) in page.entries.iter().enumerate() {
                            tr data-testid={ "audit-log-row-" (index) } {
                                td { (render_log_time(entry.timestamp)) }
                                @if is_owner {
                                    td { (audit_scope_label(entry, locale)) }
                                }
                                td {
                                    (entry.actor_username)
                                    span.log-sub.mono { (entry.actor_user_id) }
                                }
                                td {
                                    (entity_names(entry.entity_type, &entry.entity_id))
                                    span.log-sub { (audit_entity_label(entry.entity_type, locale)) " · " code { (entry.entity_id) } }
                                }
                                td { (audit_action_label(entry.action, locale)) }
                                td title=(entry.summary) { (localized_audit_summary(entry, locale)) }
                            }
                        }
                    }
                }
                div.logs-mobile-list data-testid="logs-mobile-list" {
                    @if page.entries.is_empty() {
                        p.logs-mobile-empty data-testid="logs-mobile-empty" { (t.logs_empty) }
                    }
                    @for (index, entry) in page.entries.iter().enumerate() {
                        article.audit-log-card data-testid={ "audit-log-card-" (index) } {
                            div.audit-log-card-head {
                                div data-log-field="time" { (render_log_time(entry.timestamp)) }
                                span.badge data-log-field="action" { (audit_action_label(entry.action, locale)) }
                            }
                            dl.audit-log-card-fields {
                                @if is_owner {
                                    div { dt { (t.logs_col_scope) } dd data-log-field="scope" { (audit_scope_label(entry, locale)) } }
                                }
                                div { dt { (t.logs_col_who) } dd data-log-field="actor" { (entry.actor_username) } }
                                div {
                                    dt { (t.logs_col_target) }
                                    dd data-log-field="target" {
                                        (entity_names(entry.entity_type, &entry.entity_id))
                                        " (" (audit_entity_label(entry.entity_type, locale)) ")"
                                    }
                                }
                                div {
                                    dt { (t.logs_col_details) }
                                    dd data-log-field="summary" { (localized_audit_summary(entry, locale)) }
                                }
                            }
                        }
                    }
                }
            }
            nav.logs-pagination aria-label=(t.logs_pagination_label) {
                span.page-count { (fill(t.logs_page, &[("page", &page.page), ("total", &page_total)])) }
                div.actions {
                    @if page.has_prev() {
                        a.button.button-secondary.button-compact data-testid="logs-prev"
                            href=(page_href(page.page.saturating_sub(1))) {
                            (t.logs_previous)
                        }
                    } @else {
                        span.button.button-secondary.button-compact aria-disabled="true" { (t.logs_previous) }
                    }
                    @if page.has_next() {
                        a.button.button-secondary.button-compact data-testid="logs-next"
                            href=(page_href(page.page.saturating_add(1))) {
                            (t.logs_next)
                        }
                    } @else {
                        span.button.button-secondary.button-compact aria-disabled="true" { (t.logs_next) }
                    }
                }
            }
        }
    }
}

fn render_log_time(at: DateTime<Utc>) -> Markup {
    html! {
        span.log-time {
            (render_time(at))
            span.log-time-relative data-relative-time=(at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)) {}
        }
    }
}
