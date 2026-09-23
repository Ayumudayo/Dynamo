use std::collections::{HashMap, HashSet};

use dynamo_enablement::{ResolvedCommandState, ResolvedModuleState};
use dynamo_module_kit::{
    CommandCatalog, CommandCatalogEntry, ModuleCatalog, ModuleCatalogEntry, SettingsField,
    SettingsFieldKind, SettingsSchema,
};
use dynamo_ops::{DashboardAuditAction, DashboardAuditEntityType, DashboardAuditLogPage};
use dynamo_settings::{
    DeploymentModuleSettings, DeploymentSettings, GuildModuleSettings, GuildSettings,
};
use serde_json::Value;

use super::super::{CommandSyncDisplayState, CommandSyncPanel, page_query_for_logs};
pub(crate) fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn render_deployment_module_modal(
    entry: &ModuleCatalogEntry,
    resolved: &ResolvedModuleState,
    runtime_notice: &str,
    current: &DeploymentModuleSettings,
) -> String {
    render_settings_modal(
        &modal_id_for_module("deployment", entry.module.id),
        entry.module.display_name,
        &format!(
            "<div class=\"modal-status-row\"><p class=\"detail-meta\"><strong>Status:</strong> {}</p>{}</div><form class=\"settings-form\" data-testid=\"deployment-module-form-{}\" onsubmit=\"return patchDeploymentModule(event, '{}')\"><div class=\"settings-grid settings-grid-basic\"><div class=\"settings-field settings-field-span-2 settings-field-toggle\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>Installed</strong><small>Controls whether this module is installed deployment-wide.</small></span><input type=\"checkbox\" name=\"installed\" {}/></label></div><div class=\"settings-field settings-field-span-2 settings-field-toggle\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>Enabled</strong><small>Controls whether this module is enabled by default.</small></span><input type=\"checkbox\" name=\"enabled\" {}/></label></div></div><div class=\"modal-actions\"><button class=\"button button-secondary\" data-testid=\"cancel-settings-{}\" type=\"button\" onclick=\"closeSettingsModal('{modal_id}')\">Cancel</button><button class=\"button button-primary\" data-testid=\"save-settings-{}\" type=\"submit\">Save</button><span class=\"modal-status\" id=\"deployment-status-{status_key}\"></span></div></form>",
            render_deployment_status(resolved),
            runtime_notice,
            status_key(entry.module.id),
            escape_html(entry.module.id),
            if current.installed { "checked" } else { "" },
            if current.enabled { "checked" } else { "" },
            status_key(entry.module.id),
            status_key(entry.module.id),
            modal_id = modal_id_for_module("deployment", entry.module.id),
            status_key = escape_html(entry.module.id)
        ),
    )
}

pub(crate) fn render_guild_module_modal(
    guild_id: u64,
    entry: &ModuleCatalogEntry,
    resolved: &ResolvedModuleState,
    runtime_notice: &str,
    current: &GuildModuleSettings,
    structured_fields: &str,
) -> String {
    render_settings_modal(
        &modal_id_for_module("guild", entry.module.id),
        entry.module.display_name,
        &format!(
            "<div class=\"modal-status-row\"><p class=\"detail-meta\"><strong>Status:</strong> {}</p>{}</div><form class=\"settings-form\" data-testid=\"guild-module-form-{}\" onsubmit=\"return patchGuildModule(event, '{}', '{}')\"><div class=\"settings-grid settings-grid-basic\"><div class=\"settings-field settings-field-span-2 settings-field-toggle\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>Enabled in guild</strong><small>Overrides deployment defaults for this server.</small></span><input type=\"checkbox\" name=\"enabled\" {}/></label></div></div>{}<div class=\"modal-actions\"><button class=\"button button-secondary\" data-testid=\"cancel-settings-{}\" type=\"button\" onclick=\"closeSettingsModal('{modal_id}')\">Cancel</button><button class=\"button button-primary\" data-testid=\"save-settings-{}\" type=\"submit\">Save</button><span class=\"modal-status\" id=\"guild-status-{status_key}\"></span></div></form>",
            render_guild_status(resolved),
            runtime_notice,
            status_key(entry.module.id),
            guild_id,
            escape_html(entry.module.id),
            if current.enabled { "checked" } else { "" },
            structured_fields,
            status_key(entry.module.id),
            status_key(entry.module.id),
            modal_id = modal_id_for_module("guild", entry.module.id),
            status_key = escape_html(entry.module.id),
        ),
    )
}

pub(crate) fn render_deployment_command_modals(
    command_catalog: &CommandCatalog,
    settings: &DeploymentSettings,
    resolved_states: &[ResolvedCommandState],
) -> String {
    let resolved_by_id = resolved_states
        .iter()
        .map(|state| (state.command.id.as_str(), state))
        .collect::<HashMap<_, _>>();
    command_catalog
        .entries
        .iter()
        .map(|entry| {
            let current = settings
                .commands
                .get(&entry.command.id)
                .cloned()
                .unwrap_or_default();
            let resolved = resolved_by_id.get(entry.command.id.as_str()).copied();
            let structured_fields = render_command_structured_fields(entry, &current.configuration);
            render_settings_modal(
                &modal_id_for_command("deployment", &entry.command.id),
                &entry.command.display_name,
                &format!(
                    "<div class=\"modal-status-row\"><p class=\"detail-meta\"><strong>Status:</strong> {}</p></div><form class=\"settings-form\" data-testid=\"deployment-command-form-{}\" onsubmit=\"return patchDeploymentCommand(event, '{}')\"><div class=\"settings-grid settings-grid-basic\"><div class=\"settings-field settings-field-span-2 settings-field-toggle\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>Installed</strong><small>Controls whether the command is installed deployment-wide.</small></span><input type=\"checkbox\" name=\"installed\" {}/></label></div><div class=\"settings-field settings-field-span-2 settings-field-toggle\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>Enabled</strong><small>Controls whether the command is enabled by default.</small></span><input type=\"checkbox\" name=\"enabled\" {}/></label></div></div>{}<div class=\"modal-actions\"><button class=\"button button-secondary\" data-testid=\"cancel-settings-{}\" type=\"button\" onclick=\"closeSettingsModal('{modal_id}')\">Cancel</button><button class=\"button button-primary\" data-testid=\"save-settings-{}\" type=\"submit\">Save</button><span class=\"modal-status\" id=\"deployment-command-status-{status_key}\"></span></div></form>",
                    resolved
                        .map(render_deployment_command_status)
                        .unwrap_or_else(|| "unknown".to_string()),
                    status_key(&entry.command.id),
                    escape_html(&entry.command.id),
                    if current.installed { "checked" } else { "" },
                    if current.enabled { "checked" } else { "" },
                    structured_fields,
                    status_key(&entry.command.id),
                    status_key(&entry.command.id),
                    modal_id = modal_id_for_command("deployment", &entry.command.id),
                    status_key = status_key(&entry.command.id),
                ),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn render_guild_command_modals(
    guild_id: u64,
    command_catalog: &CommandCatalog,
    settings: &GuildSettings,
    resolved_states: &[ResolvedCommandState],
) -> String {
    let resolved_by_id = resolved_states
        .iter()
        .map(|state| (state.command.id.as_str(), state))
        .collect::<HashMap<_, _>>();
    command_catalog
        .entries
        .iter()
        .map(|entry| {
            let current = settings
                .commands
                .get(&entry.command.id)
                .cloned()
                .unwrap_or_default();
            let resolved = resolved_by_id.get(entry.command.id.as_str()).copied();
            let structured_fields = render_command_structured_fields(entry, &current.configuration);
            render_settings_modal(
                &modal_id_for_command("guild", &entry.command.id),
                &entry.command.display_name,
                &format!(
                    "<div class=\"modal-status-row\"><p class=\"detail-meta\"><strong>Status:</strong> {}</p></div><form class=\"settings-form\" data-testid=\"guild-command-form-{}\" onsubmit=\"return patchGuildCommand(event, '{}', '{}')\"><div class=\"settings-grid settings-grid-basic\"><div class=\"settings-field settings-field-span-2 settings-field-toggle\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>Enabled in guild</strong><small>Overrides deployment command defaults for this server.</small></span><input type=\"checkbox\" name=\"enabled\" {}/></label></div></div>{}<div class=\"modal-actions\"><button class=\"button button-secondary\" data-testid=\"cancel-settings-{}\" type=\"button\" onclick=\"closeSettingsModal('{modal_id}')\">Cancel</button><button class=\"button button-primary\" data-testid=\"save-settings-{}\" type=\"submit\">Save</button><span class=\"modal-status\" id=\"guild-command-status-{status_key}\"></span></div></form>",
                    resolved
                        .map(render_guild_command_status)
                        .unwrap_or_else(|| "unknown".to_string()),
                    status_key(&entry.command.id),
                    guild_id,
                    escape_html(&entry.command.id),
                    if current.enabled { "checked" } else { "" },
                    structured_fields,
                    status_key(&entry.command.id),
                    status_key(&entry.command.id),
                    modal_id = modal_id_for_command("guild", &entry.command.id),
                    status_key = status_key(&entry.command.id),
                ),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn render_settings_modal(modal_id: &str, title: &str, body: &str) -> String {
    format!(
        "<div id=\"{modal_id}\" class=\"settings-modal-overlay\" data-testid=\"settings-modal-{modal_testid}\" hidden onclick=\"dismissSettingsModal(event, '{modal_id}')\"><div class=\"settings-modal\" data-modal-root role=\"dialog\" aria-modal=\"true\" aria-labelledby=\"modal-title-{modal_id}\" tabindex=\"-1\" onclick=\"event.stopPropagation()\"><div class=\"settings-modal-head\"><div><p class=\"eyebrow\">Settings</p><h3 id=\"modal-title-{modal_id}\" title=\"{title}\">{title}</h3></div><button class=\"modal-close\" data-testid=\"modal-close-{modal_testid}\" type=\"button\" aria-label=\"Close settings\" onclick=\"closeSettingsModal('{modal_id}')\">×</button></div><div class=\"settings-modal-body\">{body}</div></div></div>",
        modal_id = modal_id,
        modal_testid = status_key(modal_id),
        title = escape_html(title),
        body = body,
    )
}

pub(crate) fn render_module_toggle(
    scope: &str,
    module_id: &str,
    _deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    resolved: &ResolvedModuleState,
) -> String {
    match scope {
        "guild" => format!(
            "<label class=\"toggle-switch\"><input data-testid=\"module-toggle-{testid}\" type=\"checkbox\" aria-label=\"Enable module {module_id} for this guild\" {checked} onchange=\"toggleGuildModule('{guild_id}', '{module_id}', this.checked, this)\" /><span class=\"toggle-slider\"></span></label>",
            testid = status_key(module_id),
            checked = if resolved.guild_enabled {
                "checked"
            } else {
                ""
            },
            guild_id = guild.map(|g| g.guild_id.to_string()).unwrap_or_default(),
            module_id = escape_html(module_id),
        ),
        _ => format!(
            "<label class=\"toggle-switch\"><input data-testid=\"module-toggle-{testid}\" type=\"checkbox\" aria-label=\"Enable module {module_id} deployment-wide\" {checked} onchange=\"toggleDeploymentModule('{module_id}', this.checked, this)\" /><span class=\"toggle-slider\"></span></label>",
            testid = status_key(module_id),
            checked = if resolved.deployment_enabled {
                "checked"
            } else {
                ""
            },
            module_id = escape_html(module_id),
        ),
    }
}

pub(crate) fn render_command_toggle(
    scope: &str,
    command_id: &str,
    _deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    resolved: &ResolvedCommandState,
) -> String {
    match scope {
        "guild" => format!(
            "<label class=\"toggle-switch\"><input data-testid=\"command-toggle-{testid}\" type=\"checkbox\" aria-label=\"Enable command {command_id} for this guild\" {checked} onchange=\"toggleGuildCommand('{guild_id}', '{command_id}', this.checked, this)\" /><span class=\"toggle-slider\"></span></label>",
            testid = status_key(command_id),
            checked = if resolved.guild_enabled {
                "checked"
            } else {
                ""
            },
            guild_id = guild.map(|g| g.guild_id.to_string()).unwrap_or_default(),
            command_id = escape_html(command_id),
        ),
        _ => format!(
            "<label class=\"toggle-switch\"><input data-testid=\"command-toggle-{testid}\" type=\"checkbox\" aria-label=\"Enable command {command_id} deployment-wide\" {checked} onchange=\"toggleDeploymentCommand('{command_id}', this.checked, this)\" /><span class=\"toggle-slider\"></span></label>",
            testid = status_key(command_id),
            checked = if resolved.deployment_enabled {
                "checked"
            } else {
                ""
            },
            command_id = escape_html(command_id),
        ),
    }
}

pub(crate) fn render_command_category_tabs(catalog: &CommandCatalog) -> String {
    let mut seen = HashSet::new();
    let mut tabs = vec![
        "<button class=\"tab-button active\" data-testid=\"command-tab-all\" type=\"button\" aria-pressed=\"true\" onclick=\"setCommandCategory('all', this)\">All</button>".to_string(),
    ];

    for entry in &catalog.entries {
        let key = command_category_key(entry);
        let label = command_category_label(entry);
        if seen.insert(key.clone()) {
            tabs.push(format!(
                "<button class=\"tab-button command-tab\" data-testid=\"command-tab-{key}\" type=\"button\" aria-pressed=\"false\" onclick=\"setCommandCategory('{key}', this)\">{label}</button>",
                key = escape_html(&key),
                label = escape_html(&label),
            ));
        }
    }

    format!(
        "<div class=\"tab-row command-category-row\">{}</div>",
        tabs.join("")
    )
}

pub(crate) fn render_command_sync_panel(panel: &CommandSyncPanel) -> String {
    let status_class = match panel.state {
        CommandSyncDisplayState::InSync => "sync-panel sync-panel-ok",
        CommandSyncDisplayState::Required => "sync-panel sync-panel-warn",
        CommandSyncDisplayState::Pending => "sync-panel sync-panel-info",
        CommandSyncDisplayState::Failed => "sync-panel sync-panel-error",
        CommandSyncDisplayState::Unsupported => "sync-panel sync-panel-muted",
    };
    let action = match (&panel.button_label, &panel.button_action) {
        (Some(label), Some(action)) => format!(
            "<button class=\"button button-primary button-compact\" type=\"button\" onclick=\"{action}\">{label}</button>",
            action = action,
            label = escape_html(label),
        ),
        _ => String::new(),
    };
    let status_text = panel
        .status_text
        .as_ref()
        .map(|value| escape_html(value))
        .unwrap_or_default();

    format!(
        "<div class=\"{status_class}\" data-testid=\"command-sync-panel\"><div class=\"sync-panel-copy\"><h3>{title}</h3><p>{message}</p><p class=\"sync-panel-status\"><span>{status_text}</span><span id=\"command-sync-inline-status\" class=\"card-status\"></span></p></div><div class=\"sync-panel-actions\">{action}</div></div>",
        status_class = status_class,
        title = escape_html(&panel.title),
        message = escape_html(&panel.message),
        status_text = status_text,
        action = action,
    )
}

pub(crate) fn render_audit_logs_section(
    base_path: &str,
    page: &DashboardAuditLogPage,
    entity_type: Option<DashboardAuditEntityType>,
    action: Option<DashboardAuditAction>,
) -> String {
    let rows = if page.entries.is_empty() {
        "<tr><td colspan=\"5\" class=\"empty-cell\">No dashboard audit events recorded yet.</td></tr>"
            .to_string()
    } else {
        page.entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                format!(
                    "<tr data-testid=\"audit-log-row-{index}\"><td>{time}</td><td>{actor}</td><td>{target}</td><td>{action}</td><td>{summary}</td></tr>",
                    index = index,
                    time = escape_html(&entry.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
                    actor = escape_html(&format!("{} ({})", entry.actor_username, entry.actor_user_id)),
                    target = escape_html(&format!(
                        "{} / {}",
                        audit_entity_label(entry.entity_type),
                        entry.entity_id
                    )),
                    action = escape_html(audit_action_label(entry.action)),
                    summary = escape_html(&entry.summary),
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };

    let prev_link = if page.has_prev() {
        format!(
            "<a class=\"button button-secondary button-compact\" href=\"{base}{query}\" data-testid=\"logs-prev\">Previous</a>",
            base = base_path,
            query = page_query_for_logs(entity_type, action, page.page.saturating_sub(1)),
        )
    } else {
        "<span class=\"button button-secondary button-compact button-disabled\">Previous</span>"
            .to_string()
    };
    let next_link = if page.has_next() {
        format!(
            "<a class=\"button button-secondary button-compact\" href=\"{base}{query}\" data-testid=\"logs-next\">Next</a>",
            base = base_path,
            query = page_query_for_logs(entity_type, action, page.page.saturating_add(1)),
        )
    } else {
        "<span class=\"button button-secondary button-compact button-disabled\">Next</span>"
            .to_string()
    };

    format!(
        "<section class=\"section-block\" data-testid=\"logs-section\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Logs</p><h2>Dashboard Audit Trail</h2></div></div><form class=\"toolbar compact-toolbar\" method=\"get\" action=\"{base_path}\"><input type=\"hidden\" name=\"tab\" value=\"logs\" /><select name=\"log_entity\" class=\"toolbar-select\" data-testid=\"logs-entity-filter\"><option value=\"\" {all_entity}>All entities</option><option value=\"module\" {module_selected}>Modules</option><option value=\"command\" {command_selected}>Commands</option></select><select name=\"log_action\" class=\"toolbar-select\" data-testid=\"logs-action-filter\"><option value=\"\" {all_action}>All actions</option><option value=\"toggle\" {toggle_selected}>Toggles</option><option value=\"save_settings\" {save_selected}>Settings saves</option></select><button class=\"button button-secondary button-compact\" type=\"submit\">Apply</button></form><div class=\"panel logs-panel\"><table class=\"logs-table\" data-testid=\"logs-table\"><thead><tr><th>Time</th><th>Actor</th><th>Target</th><th>Action</th><th>Summary</th></tr></thead><tbody>{rows}</tbody></table><div class=\"logs-mobile-list\" data-testid=\"logs-mobile-list\">{mobile_cards}</div></div><div class=\"logs-pagination\"><span class=\"page-count\">Page {page_number} of {page_total}</span><div class=\"actions compact-actions\">{prev_link}{next_link}</div></div></section>",
        base_path = base_path,
        all_entity = if entity_type.is_none() {
            "selected"
        } else {
            ""
        },
        module_selected = if entity_type == Some(DashboardAuditEntityType::Module) {
            "selected"
        } else {
            ""
        },
        command_selected = if entity_type == Some(DashboardAuditEntityType::Command) {
            "selected"
        } else {
            ""
        },
        all_action = if action.is_none() { "selected" } else { "" },
        toggle_selected = if action == Some(DashboardAuditAction::Toggle) {
            "selected"
        } else {
            ""
        },
        save_selected = if action == Some(DashboardAuditAction::SaveSettings) {
            "selected"
        } else {
            ""
        },
        rows = rows,
        mobile_cards = render_audit_log_mobile_cards(page),
        page_number = page.page,
        page_total = page.total.max(1).div_ceil(page.page_size.max(1)),
        prev_link = prev_link,
        next_link = next_link,
    )
}

pub(crate) fn render_audit_log_mobile_cards(page: &DashboardAuditLogPage) -> String {
    if page.entries.is_empty() {
        return "<article class=\"logs-mobile-empty\" data-testid=\"logs-mobile-empty\">No dashboard audit events recorded yet.</article>".to_string();
    }

    page.entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            format!(
                "<article class=\"audit-log-card\" data-testid=\"audit-log-card-{index}\"><div class=\"audit-log-card-head\"><p class=\"audit-log-card-time\" data-log-field=\"time\">{time}</p><span class=\"pill\" data-log-field=\"action\">{action}</span></div><dl class=\"audit-log-card-fields\"><div class=\"audit-log-card-field\"><dt>Actor</dt><dd data-log-field=\"actor\">{actor}</dd></div><div class=\"audit-log-card-field\"><dt>Target</dt><dd data-log-field=\"target\">{target}</dd></div><div class=\"audit-log-card-field audit-log-card-field-summary\"><dt>Summary</dt><dd data-log-field=\"summary\">{summary}</dd></div></dl></article>",
                index = index,
                time = escape_html(&entry.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
                actor = escape_html(&format!("{} ({})", entry.actor_username, entry.actor_user_id)),
                target = escape_html(&format!(
                    "{} / {}",
                    audit_entity_label(entry.entity_type),
                    entry.entity_id
                )),
                action = escape_html(audit_action_label(entry.action)),
                summary = escape_html(&entry.summary),
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

pub(crate) fn render_dashboard_page_shell(
    overview: &str,
    page_tabs: &str,
    active_section: &str,
    active_tab: &str,
) -> String {
    if active_tab == "overview" {
        return format!(
            "<div class=\"dashboard-page-shell\"><div class=\"dashboard-page-overview\" data-testid=\"dashboard-page-overview\">{overview}</div><div class=\"dashboard-page-tabs\" data-testid=\"dashboard-page-tabs\">{page_tabs}</div><div class=\"dashboard-page-active\" data-testid=\"dashboard-page-active\">{active_section}</div></div>",
            overview = overview,
            page_tabs = page_tabs,
            active_section = active_section,
        );
    }

    format!(
        "<div class=\"dashboard-page-shell dashboard-page-shell-task-first\"><div class=\"dashboard-page-tabs\" data-testid=\"dashboard-page-tabs\">{page_tabs}</div><div class=\"dashboard-page-active\" data-testid=\"dashboard-page-active\">{active_section}</div></div>",
        page_tabs = page_tabs,
        active_section = active_section,
    )
}

pub(crate) fn audit_entity_label(entity_type: DashboardAuditEntityType) -> &'static str {
    match entity_type {
        DashboardAuditEntityType::Module => "Module",
        DashboardAuditEntityType::Command => "Command",
    }
}

pub(crate) fn audit_action_label(action: DashboardAuditAction) -> &'static str {
    match action {
        DashboardAuditAction::Toggle => "Toggle",
        DashboardAuditAction::SaveSettings => "Save settings",
    }
}

pub(crate) fn render_overview_section(
    title: &str,
    subtitle: &str,
    stats: &[(&str, String)],
) -> String {
    let stat_markup = stats
        .iter()
        .map(|(label, value)| {
            format!(
                "<div class=\"stat\"><span>{}</span><strong>{}</strong></div>",
                escape_html(label),
                escape_html(value)
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        "<section class=\"hero compact dyno-hero\"><div><p class=\"eyebrow\">Dashboard</p><h1>{}</h1><p class=\"lede\">{}</p></div><div class=\"hero-card\"><dl>{}</dl></div></section>",
        escape_html(title),
        escape_html(subtitle),
        stat_markup
    )
}

pub(crate) fn render_module_summary_cards(
    scope: &str,
    catalog: &ModuleCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    resolved_states: &[ResolvedModuleState],
) -> String {
    catalog
        .entries
        .iter()
        .zip(resolved_states.iter())
        .map(|(entry, resolved)| {
            let toggle = render_module_toggle(scope, entry.module.id, deployment, guild, resolved);
            let state_summary = if scope == "guild" {
                format!(
                    "<p class=\"detail-meta state-summary\" data-testid=\"module-state-{testid}\">{}</p>",
                    escape_html(&render_guild_status(resolved)),
                    testid = status_key(entry.module.id),
                )
            } else {
                String::new()
            };
            format!(
                "<article class=\"panel summary-card module-card\" data-testid=\"module-card-{testid}\" data-module-name=\"{data_name}\"><div class=\"summary-card-head\"><h3 title=\"{name}\">{name}</h3>{toggle}</div><p title=\"{description}\">{description}</p>{state_summary}<div class=\"actions compact-actions\"><button class=\"button button-secondary button-compact\" data-testid=\"module-settings-button-{testid}\" type=\"button\" onclick=\"openSettingsModal('{modal_id}', this)\">Settings</button><span class=\"card-status\" id=\"card-status-module-{testid}\"></span></div></article>",
                testid = status_key(entry.module.id),
                data_name = escape_html(&entry.module.display_name.to_ascii_lowercase()),
                name = escape_html(entry.module.display_name),
                toggle = toggle,
                description = escape_html(entry.module.description),
                state_summary = state_summary,
                modal_id = modal_id_for_module(scope, entry.module.id),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn render_command_summary_cards(
    scope: &str,
    catalog: &CommandCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    resolved_states: &[ResolvedCommandState],
) -> String {
    catalog
        .entries
        .iter()
        .zip(resolved_states.iter())
        .map(|(entry, resolved)| {
            let toggle = render_command_toggle(scope, &entry.command.id, deployment, guild, resolved);
            let state_summary = if scope == "guild" {
                format!(
                    "<p class=\"detail-meta state-summary\" data-testid=\"command-state-{testid}\">{}</p>",
                    escape_html(&render_guild_command_status(resolved)),
                    testid = status_key(&entry.command.id),
                )
            } else {
                String::new()
            };
            format!(
                "<article class=\"panel summary-card command-card\" data-testid=\"command-card-{testid}\" data-command-name=\"{command_name}\" data-command-category=\"{category_key}\"><div class=\"summary-card-head\"><h3 title=\"{display_name}\">{display_name}</h3>{toggle}</div><p title=\"{description}\">{description}</p><div class=\"summary-card-subtitle\">{category_label}</div>{state_summary}<div class=\"actions compact-actions\"><button class=\"button button-secondary button-compact\" data-testid=\"command-settings-button-{testid}\" type=\"button\" onclick=\"openSettingsModal('{modal_id}', this)\">Settings</button><span class=\"card-status\" id=\"card-status-command-{testid}\"></span></div></article>",
                testid = status_key(&entry.command.id),
                command_name = escape_html(&entry.command.display_name.to_ascii_lowercase()),
                category_key = escape_html(&command_category_key(entry)),
                display_name = escape_html(&entry.command.display_name),
                toggle = toggle,
                description = escape_html(entry.command.description.as_deref().unwrap_or("No description provided.")),
                category_label = escape_html(&command_category_label(entry)),
                state_summary = state_summary,
                modal_id = modal_id_for_command(scope, &entry.command.id),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn command_category_key(entry: &CommandCatalogEntry) -> String {
    entry
        .command
        .category
        .clone()
        .unwrap_or_else(|| {
            module_category_label_from_name(entry.command.module_display_name).to_string()
        })
        .to_ascii_lowercase()
        .replace(' ', "-")
}

pub(crate) fn command_category_label(entry: &CommandCatalogEntry) -> String {
    entry
        .command
        .category
        .clone()
        .unwrap_or_else(|| entry.command.module_display_name.to_string())
}

pub(crate) fn module_category_label_from_name(name: &str) -> &str {
    name
}

pub(crate) fn modal_id_for_module(scope: &str, module_id: &str) -> String {
    format!("modal-{}-module-{}", scope, status_key(module_id))
}

pub(crate) fn modal_id_for_command(scope: &str, command_id: &str) -> String {
    format!("modal-{}-command-{}", scope, status_key(command_id))
}

pub(crate) fn count_enabled_modules(states: &[ResolvedModuleState]) -> usize {
    states
        .iter()
        .filter(|state| state.effective_enabled)
        .count()
}

pub(crate) fn count_enabled_commands(states: &[ResolvedCommandState]) -> usize {
    states
        .iter()
        .filter(|state| state.effective_enabled)
        .count()
}

pub(crate) fn render_module_runtime_notice(module_id: &str) -> String {
    runtime_notice_text(module_id)
        .map(|note| {
            format!(
                "<p style=\"padding:8px 12px; border:1px solid #d99; background:#fff6f6\"><strong>Runtime notice:</strong> {}</p>",
                escape_html(note)
            )
        })
        .unwrap_or_default()
}

pub(crate) fn runtime_notice_text(module_id: &str) -> Option<&'static str> {
    let _ = module_id;
    None
}

pub(crate) fn render_structured_fields(
    entry: &ModuleCatalogEntry,
    configuration: &Value,
) -> String {
    render_settings_sections(
        &entry.settings,
        configuration,
        "<p>No configurable fields for this module.</p>",
    )
}

pub(crate) fn render_settings_sections(
    settings: &SettingsSchema,
    configuration: &Value,
    empty_markup: &str,
) -> String {
    let fields = settings
        .sections
        .iter()
        .map(|section| {
            let rendered_fields = section
                .fields
                .iter()
                .map(|field| render_field(field, configuration))
                .collect::<Vec<_>>()
                .join("\n");

            format!(
                "<fieldset class=\"settings-section\"><legend>{}</legend><p class=\"section-help\">{}</p><div class=\"settings-grid\">{}</div></fieldset>",
                escape_html(section.title),
                escape_html(section.description.unwrap_or("")),
                rendered_fields
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    if fields.is_empty() {
        empty_markup.to_string()
    } else {
        fields
    }
}

pub(crate) fn render_command_structured_fields(
    entry: &CommandCatalogEntry,
    configuration: &Value,
) -> String {
    render_settings_sections(
        &entry.settings,
        configuration,
        "<p>No configurable fields for this command.</p>",
    )
}

pub(crate) fn render_field(field: &SettingsField, configuration: &Value) -> String {
    let testid = status_key(field.key);
    let control_id = format!("field-{testid}-control");
    let help_id = format!("field-{testid}-help");
    let help_text = field
        .help_text
        .map(escape_html)
        .map(|text| format!("<small id=\"{help_id}\">{text}</small>"))
        .unwrap_or_default();
    let described_by = if field.help_text.is_some() {
        format!(" aria-describedby=\"{help_id}\"")
    } else {
        String::new()
    };
    let required = if field.required { "required" } else { "" };
    let field_key = escape_html(field.key);
    let field_label = escape_html(field.label);

    match &field.kind {
        SettingsFieldKind::Toggle => {
            let checked = field_bool_value(configuration, field.key).unwrap_or(false);
            format!(
                "<div class=\"settings-field settings-field-span-2 settings-field-toggle\" data-testid=\"field-{testid}\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>{label}</strong>{help_text}</span><input type=\"checkbox\" data-setting-key=\"{key}\" data-setting-kind=\"toggle\" {checked}/></label></div>",
                testid = status_key(field.key),
                key = field_key,
                label = field_label,
                checked = if checked { "checked" } else { "" },
                help_text = help_text,
            )
        }
        SettingsFieldKind::Integer { min, max } => {
            let value = field_string_value(configuration, field.key);
            let min_attr = min
                .map(|value| format!(" min=\"{value}\""))
                .unwrap_or_default();
            let max_attr = max
                .map(|value| format!(" max=\"{value}\""))
                .unwrap_or_default();
            format!(
                "<div class=\"settings-field\" data-testid=\"field-{testid}\"><label for=\"{control_id}\">{label}</label>{help_text}<input id=\"{control_id}\" type=\"number\" data-setting-key=\"{key}\" data-setting-kind=\"integer\" value=\"{value}\"{min_attr}{max_attr}{described_by} {required}/></div>",
                testid = testid,
                control_id = control_id,
                described_by = described_by,
                label = field_label,
                help_text = help_text,
                key = field_key,
                value = escape_html(&value.unwrap_or_default()),
                min_attr = min_attr,
                max_attr = max_attr,
                required = required,
            )
        }
        SettingsFieldKind::Text => {
            let value = field_string_value(configuration, field.key).unwrap_or_default();
            if value.len() > 40 || value.starts_with('[') || value.starts_with('{') {
                format!(
                    "<div class=\"settings-field settings-field-span-2 settings-field-textarea\" data-testid=\"field-{testid}\"><label for=\"{control_id}\">{label}</label>{help_text}<textarea id=\"{control_id}\" data-setting-key=\"{key}\" data-setting-kind=\"text\" rows=\"4\" cols=\"80\"{described_by} {required}>{value}</textarea></div>",
                    testid = testid,
                    control_id = control_id,
                    described_by = described_by,
                    label = field_label,
                    help_text = help_text,
                    key = field_key,
                    required = required,
                    value = escape_html(&value),
                )
            } else {
                format!(
                    "<div class=\"settings-field\" data-testid=\"field-{testid}\"><label for=\"{control_id}\">{label}</label>{help_text}<input id=\"{control_id}\" type=\"text\" data-setting-key=\"{key}\" data-setting-kind=\"text\" value=\"{value}\"{described_by} {required}/></div>",
                    testid = testid,
                    control_id = control_id,
                    described_by = described_by,
                    label = field_label,
                    help_text = help_text,
                    key = field_key,
                    value = escape_html(&value),
                    required = required,
                )
            }
        }
        SettingsFieldKind::Select { options } => {
            let current = field_string_value(configuration, field.key).unwrap_or_default();
            let options = options
                .iter()
                .map(|option| {
                    format!(
                        "<option value=\"{value}\" {selected}>{label}</option>",
                        value = escape_html(option.value),
                        selected = if current == option.value {
                            "selected"
                        } else {
                            ""
                        },
                        label = escape_html(option.label),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");

            format!(
                "<div class=\"settings-field\" data-testid=\"field-{testid}\"><label for=\"{control_id}\">{label}</label>{help_text}<select id=\"{control_id}\" data-setting-key=\"{key}\" data-setting-kind=\"select\"{described_by} {required}>{options}</select></div>",
                testid = testid,
                control_id = control_id,
                described_by = described_by,
                label = field_label,
                help_text = help_text,
                key = field_key,
                required = required,
                options = options,
            )
        }
    }
}

pub(crate) fn render_deployment_status(state: &ResolvedModuleState) -> String {
    format!(
        "installed: {} | deployment: {} | effective: {}",
        yes_no(state.installed),
        yes_no(state.deployment_enabled),
        yes_no(state.effective_enabled),
    )
}

pub(crate) fn render_guild_status(state: &ResolvedModuleState) -> String {
    format!(
        "Installed: {} | Deployment: {} | Local guild: {} | Effective: {} | {}",
        on_off(state.installed),
        on_off(state.deployment_enabled),
        on_off(state.guild_enabled),
        on_off(state.effective_enabled),
        module_blocker(state),
    )
}

pub(crate) fn render_deployment_command_status(state: &ResolvedCommandState) -> String {
    format!(
        "module: {} | installed: {} | deployment: {} | effective: {}",
        yes_no(state.module_effective_enabled),
        yes_no(state.installed),
        yes_no(state.deployment_enabled),
        yes_no(state.effective_enabled),
    )
}

pub(crate) fn render_guild_command_status(state: &ResolvedCommandState) -> String {
    format!(
        "Parent module: {} | Installed: {} | Deployment: {} | Local guild: {} | Effective: {} | {}",
        on_off(state.module_effective_enabled),
        on_off(state.installed),
        on_off(state.deployment_enabled),
        on_off(state.guild_enabled),
        on_off(state.effective_enabled),
        command_blocker(state),
    )
}

pub(crate) fn module_blocker(state: &ResolvedModuleState) -> &'static str {
    if !state.installed {
        "Blocked by deployment installation"
    } else if !state.deployment_enabled {
        "Blocked by deployment"
    } else if !state.guild_enabled {
        "Blocked by local guild setting"
    } else {
        "No blocker"
    }
}

pub(crate) fn command_blocker(state: &ResolvedCommandState) -> &'static str {
    if !state.module_effective_enabled {
        "Blocked by parent module"
    } else if !state.installed {
        "Blocked by deployment installation"
    } else if !state.deployment_enabled {
        "Blocked by deployment"
    } else if !state.guild_enabled {
        "Blocked by local guild setting"
    } else {
        "No blocker"
    }
}

pub(crate) fn on_off(value: bool) -> &'static str {
    if value { "On" } else { "Off" }
}

pub(crate) fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

pub(crate) fn field_bool_value(configuration: &Value, key: &str) -> Option<bool> {
    value_at_path(configuration, key).and_then(Value::as_bool)
}

pub(crate) fn field_string_value(configuration: &Value, key: &str) -> Option<String> {
    let value = value_at_path(configuration, key)?;
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Array(_) | Value::Object(_) => serde_json::to_string_pretty(value).ok(),
    }
}

pub(crate) fn value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

pub(crate) fn status_key(value: &str) -> String {
    value.replace(':', "-")
}
