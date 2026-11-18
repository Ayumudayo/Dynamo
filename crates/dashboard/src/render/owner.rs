use chrono::{Duration, Utc};
use dynamo_ops::{
    AnnouncementStatus, AnnouncementTone, BotHeartbeatSnapshot, CommandSyncResult,
    CommandSyncScopeState, OwnerAnnouncementStore,
};
use dynamo_settings::DeploymentSettings;
use maud::{Markup, html};

use crate::i18n::{Locale, fill};
use crate::render::document::{Crumb, NavItem, PageContext, render_document};
use crate::render::entities::{render_scope_header, render_time};
use crate::{DashboardSession, DashboardState};

pub(crate) struct OwnerServerRow {
    pub(crate) guild_id: u64,
    pub(crate) name: String,
    pub(crate) blocked: bool,
    pub(crate) announcement_channel_id: Option<u64>,
    pub(crate) sync_state: Option<CommandSyncScopeState>,
}

pub(crate) struct OwnerSystemData {
    pub(crate) heartbeat: Option<BotHeartbeatSnapshot>,
    pub(crate) mongo_configured: bool,
    pub(crate) app_owner_configured: bool,
    pub(crate) totp_configured: bool,
    pub(crate) secure_cookies: bool,
    pub(crate) register_globally: bool,
}

pub(crate) fn render_owner_totp_setup_page(
    state: &DashboardState,
    session: &DashboardSession,
    locale: Locale,
) -> String {
    let t = locale.t();
    let content = html! {
        section.panel.owner-lock-card data-testid="owner-totp-setup" aria-labelledby="owner-setup-title" {
            p.eyebrow { (t.nav_deployment) }
            h1 #owner-setup-title { (t.owner_setup_title) }
            p.lede { (t.owner_setup_lede) }
            p.notice { (t.owner_setup_steps) }
            div.actions {
                a.button.button-secondary href="/selector" { (t.back_to_servers) }
            }
        }
    };
    let ctx = PageContext::new(
        state,
        Some(session),
        locale,
        t.owner_setup_title,
        "/deployment",
    )
    .nav(NavItem::Deployment)
    .crumbs(vec![
        Crumb::link(t.nav_home, "/"),
        Crumb::current(t.nav_deployment),
    ]);
    render_document(&ctx, content)
}

pub(crate) fn render_owner_totp_unlock_page(
    state: &DashboardState,
    session: &DashboardSession,
    locale: Locale,
    auth_notice: Option<&str>,
    preview_code: Option<&str>,
) -> String {
    let t = locale.t();
    let display_name = session
        .user
        .global_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&session.user.username);
    let error_banner = match auth_notice {
        Some("invalid") => Some(t.owner_unlock_error_invalid),
        Some("replayed") => Some(t.owner_unlock_error_replayed),
        Some("rate_limited") => Some(t.owner_unlock_error_rate_limited),
        _ => None,
    };

    let content = html! {
        section.panel.owner-lock-card data-testid="owner-totp-unlock" aria-labelledby="owner-unlock-title" {
            p.eyebrow { (t.nav_deployment) }
            h1 #owner-unlock-title { (t.owner_unlock_title) }
            p.lede { (fill(t.owner_unlock_lede, &[("name", &display_name)])) }
            @if let Some(message) = error_banner {
                p.notice data-tone="danger" role="alert" data-testid="owner-unlock-error" { (message) }
            }
            @if let Some(code) = preview_code {
                p.notice data-testid="owner-preview-totp-hint" {
                    (fill(t.owner_unlock_preview_hint, &[("code", &code)]))
                }
            }
            form.stack method="post" action="/deployment/verify" data-testid="owner-unlock-form" {
                input type="hidden" name="csrf_token" value=(session.csrf_token);
                div.field {
                    label for="owner-totp-code" { (t.owner_unlock_code_label) }
                    input.mono #owner-totp-code name="totp_code" type="text" inputmode="numeric"
                        pattern="[0-9]{6}" maxlength="6" autocomplete="one-time-code"
                        placeholder="000000" required autofocus;
                }
                div.actions {
                    button.button.button-primary type="submit" data-testid="owner-unlock-submit" {
                        (t.owner_unlock_submit)
                    }
                    a.button.button-secondary href="/selector" { (t.back_to_servers) }
                }
            }
        }
    };
    let ctx = PageContext::new(
        state,
        Some(session),
        locale,
        t.owner_unlock_title,
        "/deployment",
    )
    .nav(NavItem::Deployment)
    .crumbs(vec![
        Crumb::link(t.nav_home, "/"),
        Crumb::current(t.nav_deployment),
    ]);
    render_document(&ctx, content)
}

pub(crate) fn render_owner_scope_header(csrf_token: &str, locale: Locale) -> Markup {
    let t = locale.t();
    let lock_control = html! {
        div.owner-header-controls {
            span.badge data-tone="success" data-testid="owner-elevated-badge" { (t.owner_elevated_badge) }
            form method="post" action="/deployment/lock" {
                input type="hidden" name="csrf_token" value=(csrf_token);
                button.button.button-secondary.button-compact type="submit" data-testid="owner-lock-button" {
                    (t.owner_lock_button)
                }
            }
        }
    };
    html! {
        div.owner-scope-head {
            (render_scope_header(t.page_deployment_title, t.page_deployment_lede, None, None))
            (lock_control)
        }
    }
}

pub(crate) fn render_owner_overview_extras(
    announce_ready: usize,
    blocked_guilds: usize,
    locale: Locale,
) -> Markup {
    let t = locale.t();
    html! {
        article.panel data-testid="owner-overview-ops" {
            div.panel-head { h3 { (t.owner_overview_stats_title) } }
            ul.plain-list {
                li {
                    span { (t.owner_stat_announce_ready) }
                    strong { (announce_ready) }
                }
                li {
                    span { (t.owner_stat_blocked_guilds) }
                    strong { (blocked_guilds) }
                }
            }
        }
    }
}

pub(crate) fn render_owner_servers_tab(rows: &[OwnerServerRow], locale: Locale) -> Markup {
    let t = locale.t();
    html! {
        section.panel.stack data-testid="deployment-servers-section" aria-labelledby="owner-servers-heading" {
            div.panel-head {
                div {
                    h2 #owner-servers-heading { (t.owner_servers_heading) " " span.count-badge { (rows.len()) } }
                }
                button.button.button-secondary.button-compact type="button"
                    data-action="owner-batch-sync" data-testid="owner-batch-sync-button" {
                    (t.owner_servers_batch_sync)
                }
            }
            @if rows.is_empty() {
                p.empty-state { (t.owner_servers_empty) }
            } @else {
                div.panel.logs-panel {
                    table.logs-table.owner-servers-table data-testid="owner-servers-table" {
                        thead {
                            tr {
                                th scope="col" { (t.owner_servers_col_server) }
                                th scope="col" { (t.owner_servers_col_announce) }
                                th scope="col" { (t.owner_servers_col_sync) }
                                th scope="col" { (t.owner_servers_col_status) }
                                th scope="col" { (t.owner_servers_col_actions) }
                            }
                        }
                        tbody {
                            @for row in rows {
                                @let sync_label = match row.sync_state.as_ref().and_then(|s| s.last_result) {
                                    Some(CommandSyncResult::Success) => t.sync_ok_title,
                                    Some(CommandSyncResult::Failed) => t.sync_failed_title,
                                    None if row.sync_state.as_ref().is_some_and(CommandSyncScopeState::has_pending_request) => t.sync_pending_title,
                                    None => t.sync_required_title,
                                };
                                tr data-testid={ "owner-server-row-" (row.guild_id) } {
                                    td {
                                        strong { (row.name) }
                                        span.log-sub.mono { (row.guild_id) }
                                    }
                                    td {
                                        @if let Some(channel_id) = row.announcement_channel_id {
                                            span.badge data-tone="success" { (t.owner_server_announce_set) }
                                            span.log-sub.mono { "#" (channel_id) }
                                        } @else {
                                            span.badge data-tone="neutral" { (t.owner_server_announce_unset) }
                                        }
                                    }
                                    td { (sync_label) }
                                    td {
                                        @if row.blocked {
                                            span.badge data-tone="danger" { (t.owner_server_status_blocked) }
                                        } @else {
                                            span.badge data-tone="success" { (t.owner_server_status_active) }
                                        }
                                    }
                                    td {
                                        div.owner-server-actions {
                                            button.button.button-secondary.button-compact type="button"
                                                data-action="owner-guild-sync"
                                                data-guild-id=(row.guild_id) {
                                                (t.owner_server_action_sync)
                                            }
                                            button.button.button-secondary.button-compact type="button"
                                                data-action="open-owner-totp-modal"
                                                data-owner-op=(if row.blocked { "unblock" } else { "block" })
                                                data-url={ "/api/owner/guilds/" (row.guild_id) "/block" }
                                                data-guild-name=(row.name)
                                                data-blocked=(if row.blocked { "false" } else { "true" }) {
                                                (if row.blocked { t.owner_server_action_unblock } else { t.owner_server_action_block })
                                            }
                                            button.button.button-secondary.button-compact type="button"
                                                data-action="open-owner-totp-modal"
                                                data-owner-op="reset"
                                                data-url={ "/api/owner/guilds/" (row.guild_id) "/reset" }
                                                data-guild-name=(row.name) {
                                                (t.owner_server_action_reset)
                                            }
                                            button.button.button-secondary.button-compact type="button"
                                                data-action="open-owner-totp-modal"
                                                data-owner-op="leave"
                                                data-url={ "/api/owner/guilds/" (row.guild_id) "/leave" }
                                                data-guild-name=(row.name) {
                                                (t.owner_server_action_leave)
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            (render_owner_totp_action_modal(locale))
        }
    }
}

fn render_owner_totp_action_modal(locale: Locale) -> Markup {
    let t = locale.t();
    html! {
        div.settings-modal-overlay #modal-owner-totp-action data-modal-overlay hidden {
            div.settings-modal data-modal-root role="dialog" aria-modal="true"
                aria-labelledby="modal-title-owner-totp-action" tabindex="-1" {
                form.settings-form data-owner-totp-form novalidate {
                    div.settings-modal-head {
                        div {
                            p.eyebrow { (t.owner_totp_modal_eyebrow) }
                            h2 #modal-title-owner-totp-action data-owner-modal-title { (t.owner_totp_modal_eyebrow) }
                        }
                        button.icon-button type="button" data-action="close-modal" aria-label=(t.dialog_close) {
                            span.icon-close aria-hidden="true" { "×" }
                        }
                    }
                    div.settings-modal-body {
                        p.field-help data-owner-modal-target {}
                        div.field {
                            label for="owner-action-totp-code" { (t.owner_totp_modal_code_label) }
                            input.mono #owner-action-totp-code name="totp_code" type="text" inputmode="numeric"
                                pattern="[0-9]{6}" maxlength="6" placeholder="000000" required;
                            p.field-error #owner-action-totp-code-error data-field-error aria-live="polite" {}
                        }
                    }
                    div.settings-modal-foot {
                        span.modal-status data-modal-status role="status" aria-live="polite" {}
                        button.button.button-secondary type="button" data-action="cancel-modal" { (t.dialog_cancel) }
                        button.button.button-primary type="submit" { (t.owner_totp_modal_confirm) }
                    }
                }
            }
        }
    }
}

pub(crate) fn render_owner_system_tab(data: &OwnerSystemData, locale: Locale) -> Markup {
    let t = locale.t();
    let now = Utc::now();
    let heartbeat_fresh = data
        .heartbeat
        .as_ref()
        .is_some_and(|hb| now - hb.updated_at <= Duration::seconds(90));
    let bool_badge = |ok: bool| -> Markup {
        html! {
            span.badge data-tone=(if ok { "success" } else { "warn" }) {
                (if ok { t.audit_enabled_on } else { t.audit_enabled_off })
            }
        }
    };
    html! {
        section.stack data-testid="deployment-system-section" aria-labelledby="owner-system-heading" {
            h2 #owner-system-heading { (t.owner_system_heading) }
            div.system-grid {
                article.panel {
                    div.panel-head {
                        h3 { (t.owner_system_heartbeat_title) }
                        @if let Some(hb) = &data.heartbeat {
                            span.badge data-tone=(if heartbeat_fresh { "success" } else { "warn" }) {
                                (if heartbeat_fresh { t.owner_system_heartbeat_online } else { t.owner_system_heartbeat_stale })
                            }
                            @let _ = hb;
                        }
                    }
                    @if let Some(hb) = &data.heartbeat {
                        ul.plain-list {
                            li {
                                span { "Bot account" }
                                strong.mono { (hb.bot_username.as_deref().unwrap_or("—")) }
                            }
                            li {
                                span { "Last heartbeat" }
                                (render_time(hb.updated_at))
                            }
                            li {
                                span { "Gateway guilds" }
                                strong { (hb.connected_guilds) }
                            }
                            li {
                                span { "Loaded modules / commands" }
                                strong.mono { (format!("{} / {}", hb.loaded_modules, hb.loaded_commands)) }
                            }
                            li {
                                span { "Uptime (s)" }
                                strong.mono { ((now - hb.started_at).num_seconds().max(0)) }
                            }
                            li {
                                span { "Exchange rate refresh" }
                                (bool_badge(hb.exchange_refresh_active))
                            }
                            li {
                                span { "Toss Invest feed" }
                                (bool_badge(hb.tossinvest_wired))
                            }
                            li {
                                span { "Google Translate key" }
                                (bool_badge(hb.google_translate_configured))
                            }
                        }
                    } @else {
                        p.muted { (t.owner_system_heartbeat_missing) }
                    }
                }
                article.panel {
                    div.panel-head { h3 { (t.owner_system_integrations_title) } }
                    ul.plain-list {
                        li { span { "MongoDB" } (bool_badge(data.mongo_configured)) }
                        li { span { "Discord Application Owner ID" } (bool_badge(data.app_owner_configured)) }
                        li { span { "DASHBOARD_OWNER_TOTP_SECRET" } (bool_badge(data.totp_configured)) }
                        li { span { "HTTPS Secure cookies" } (bool_badge(data.secure_cookies)) }
                        li {
                            span { "Command registration" }
                            strong { (if data.register_globally { "Global" } else { "Per-guild" }) }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn render_owner_notices_tab(
    state: &DashboardState,
    deployment: &DeploymentSettings,
    store: &OwnerAnnouncementStore,
    ready_servers: usize,
    total_known_servers: usize,
    locale: Locale,
) -> Markup {
    let t = locale.t();
    let global_notice = deployment
        .module_notices
        .get("__global")
        .map(String::as_str)
        .unwrap_or_default();

    html! {
        section.stack data-testid="deployment-notices-section" aria-labelledby="owner-notices-heading" {
            article.panel.stack {
                div.panel-head {
                    div {
                        h2 #owner-notices-heading { (t.owner_notices_heading) }
                        p.field-help { (t.owner_notices_lede) }
                    }
                }
                form.stack data-owner-notices-form data-action-url="/api/owner/notices" novalidate {
                    div.field {
                        label for="owner-notice-global" { (t.owner_notices_global_label) }
                        textarea #owner-notice-global rows="2" data-owner-notice-key="__global" {
                            (global_notice)
                        }
                    }
                    div.settings-grid {
                        @for entry in &state.module_catalog.entries {
                            @let module_name = locale.catalog(entry.module.display_name);
                            @let value = deployment.module_notices.get(entry.module.id).map(String::as_str).unwrap_or_default();
                            @let field_id = format!("owner-notice-mod-{}", entry.module.id);
                            div.field {
                                label for=(field_id) { (fill(t.owner_notices_module_label, &[("module", &module_name)])) }
                                input id=(field_id) type="text" data-owner-notice-key=(entry.module.id) value=(value);
                            }
                        }
                    }
                    div.actions {
                        button.button.button-primary type="submit" data-testid="owner-notices-save" {
                            (t.owner_notices_save)
                        }
                    }
                }
            }
            article.panel.stack {
                div.panel-head {
                    div {
                        h2 { (t.owner_broadcast_heading) }
                        p.field-help { (t.owner_broadcast_lede) }
                    }
                }
                p.notice data-testid="owner-broadcast-readiness" {
                    (fill(t.owner_broadcast_target_info, &[("ready", &ready_servers), ("total", &total_known_servers)]))
                }
                form.stack data-owner-broadcast-form data-action-url="/api/owner/announcements" novalidate {
                    div.settings-grid {
                        div.field {
                            label for="owner-broadcast-title" { (t.owner_broadcast_title_label) }
                            input #owner-broadcast-title name="title" type="text" maxlength="120";
                        }
                        div.field {
                            label for="owner-broadcast-tone" { (t.owner_broadcast_tone_label) }
                            select #owner-broadcast-tone name="tone" {
                                option value="info" { (t.owner_broadcast_tone_info) }
                                option value="warning" { (t.owner_broadcast_tone_maintenance) }
                                option value="critical" { (t.owner_broadcast_tone_warning) }
                            }
                        }
                        div.field.span-2 {
                            label for="owner-broadcast-body" { (t.owner_broadcast_body_label) }
                            textarea #owner-broadcast-body name="body" rows="4" maxlength="3500" required {}
                        }
                        div.field {
                            label for="owner-broadcast-totp" { (t.owner_broadcast_totp_label) }
                            input.mono #owner-broadcast-totp name="totp_code" type="text" inputmode="numeric"
                                pattern="[0-9]{6}" maxlength="6" placeholder="000000" required;
                        }
                    }
                    div.actions {
                        button.button.button-primary type="submit" data-testid="owner-broadcast-submit" {
                            (t.owner_broadcast_submit)
                        }
                    }
                }
                div.stack {
                    h3 { (t.owner_broadcast_history_heading) }
                    @if store.entries.is_empty() {
                        p.muted { (t.owner_broadcast_history_empty) }
                    } @else {
                        ul.plain-list data-testid="owner-broadcast-history" {
                            @for item in store.entries.iter().take(10) {
                                @let tone_label = match item.tone {
                                    AnnouncementTone::Info => t.owner_broadcast_tone_info,
                                    AnnouncementTone::Warning => t.owner_broadcast_tone_maintenance,
                                    AnnouncementTone::Critical => t.owner_broadcast_tone_warning,
                                };
                                @let status_tone = match item.status {
                                    AnnouncementStatus::Queued => "info",
                                    AnnouncementStatus::Delivered => "success",
                                    AnnouncementStatus::PartialFailure => "warn",
                                    AnnouncementStatus::NoSubscribers => "neutral",
                                };
                                @let heading = if item.title.trim().is_empty() { tone_label } else { item.title.as_str() };
                                li {
                                    div {
                                        strong { (heading) }
                                        " · "
                                        span.badge data-tone=(status_tone) {
                                            (format!("{} ({}/{})", item.status.as_str(), item.delivered_guilds, item.targeted_guilds))
                                        }
                                        p.muted.small { (item.body) }
                                    }
                                    (render_time(item.created_at))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn render_guild_announcement_card(
    guild_id: u64,
    channel_id: Option<u64>,
    locale: Locale,
) -> Markup {
    let t = locale.t();
    let current_text = match channel_id {
        Some(id) => fill(t.guild_announce_set, &[("channel_id", &id)]),
        None => t.guild_announce_unset.to_string(),
    };
    let channel_str = channel_id.map(|id| id.to_string()).unwrap_or_default();
    html! {
        article.panel data-testid="guild-announcement-channel-card" {
            div.panel-head {
                div {
                    h3 { (t.guild_announce_title) }
                    p.field-help { (t.guild_announce_desc) }
                }
            }
            p data-announcement-channel-display { (current_text) }
            div.panel-foot {
                button.button.button-secondary.button-compact type="button"
                    data-testid="guild-announce-configure-button"
                    data-action="open-local-modal"
                    data-modal-id="modal-guild-announcement-channel" {
                    (t.guild_announce_configure)
                }
            }
            div.settings-modal-overlay #modal-guild-announcement-channel data-modal-overlay hidden {
                div.settings-modal data-modal-root role="dialog" aria-modal="true"
                    aria-labelledby="modal-title-guild-announcement-channel" tabindex="-1" {
                    form.settings-form
                        data-announcement-channel-form
                        data-guild-id=(guild_id)
                        data-patch-url={ "/api/guild-announcement-channel/" (guild_id) }
                        novalidate {
                        div.settings-modal-head {
                            div {
                                p.eyebrow { (t.tab_overview) }
                                h2 #modal-title-guild-announcement-channel { (t.guild_announce_modal_title) }
                            }
                            button.icon-button type="button" data-action="close-modal" aria-label=(t.dialog_close) {
                                span.icon-close aria-hidden="true" { "×" }
                            }
                        }
                        div.settings-modal-body {
                            div.field {
                                label for="guild-announcement-channel-input" { (t.guild_announce_channel_label) }
                                span.field-help #guild-announcement-channel-help { (t.guild_announce_channel_help) }
                                input #guild-announcement-channel-input type="text" inputmode="numeric"
                                    spellcheck="false" autocomplete="off"
                                    data-setting-key="announcement_channel_id"
                                    data-setting-kind="channel"
                                    data-picker="channel"
                                    value=(channel_str)
                                    placeholder=(t.field_channel_placeholder)
                                    aria-describedby="guild-announcement-channel-help guild-announcement-channel-input-error";
                                p.field-error #guild-announcement-channel-input-error data-field-error aria-live="polite" {}
                            }
                        }
                        div.settings-modal-foot {
                            span.modal-status data-modal-status role="status" aria-live="polite" {}
                            button.button.button-secondary type="button" data-action="cancel-modal" { (t.dialog_cancel) }
                            button.button.button-primary type="submit" data-testid="save-guild-announcement-channel" { (t.dialog_save) }
                        }
                    }
                }
            }
        }
    }
}
