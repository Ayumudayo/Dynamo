use std::collections::HashSet;

use super::super::{
    DashboardSession, DashboardState, DashboardUser, count_runtime_notices, dashboard_styles,
    dashboard_ui_script, page_query_for_tab, render_runtime_notices,
};
use super::settings::escape_html;

#[derive(Debug, Clone)]
pub(crate) struct GuildCard {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) icon_url: Option<String>,
    pub(crate) bot_presence: BotGuildPresence,
    pub(crate) manage_url: String,
    pub(crate) invite_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BotGuildPresence {
    Present,
    Missing,
    Unavailable,
}

pub(crate) fn render_landing_page(
    state: &DashboardState,
    session: Option<&DashboardSession>,
) -> String {
    let content = format!(
        "<section class=\"hero\"><div><p class=\"eyebrow\">Discord OAuth Dashboard</p><h1>Manage Dynamo like a real multi-server control panel.</h1><p class=\"lede\">{intro}</p><div class=\"actions\">{primary_action}<a class=\"button button-secondary\" href=\"/healthz\">Health Check</a></div></div><div class=\"hero-card\"><dl><div><dt>Modules</dt><dd>{module_count}</dd></div><div><dt>Leaf Commands</dt><dd>{command_count}</dd></div><div><dt>Runtime Notes</dt><dd>{notice_count}</dd></div></dl></div></section><section class=\"grid two\"><article class=\"panel\"><h2>Server Selector</h2><p>Dyno-like server cards split between servers you can manage now and servers that still need the bot installed.</p></article><article class=\"panel\"><h2>Shared Runtime Guard</h2><p>Dashboard state, runtime checks, and command sync all resolve from the same module and command enablement rules.</p></article></section>{runtime_notices}",
        intro = if session.is_some() {
            "Open the server listing to choose a guild, or review the shared runtime state from this dashboard."
        } else {
            "Sign in with Discord, pick the servers you can manage, and adjust module and command behavior without touching the terminal."
        },
        primary_action = if session.is_some() {
            "<a class=\"button button-primary\" href=\"/selector\">Server Listing</a>"
        } else {
            "<a class=\"button button-primary\" href=\"/login\">Sign in with Discord</a>"
        },
        module_count = state.module_catalog.entries.len(),
        command_count = state.command_catalog.entries.len(),
        notice_count = count_runtime_notices(&state.module_catalog),
        runtime_notices = render_runtime_notices(&state.module_catalog),
    );

    render_document(
        state,
        session,
        &format!("{} Dashboard", state.app_info.name),
        "OAuth-protected control plane for Dynamo.",
        Some("/"),
        None,
        &content,
    )
}

pub(crate) fn render_selector_page(
    state: &DashboardState,
    session: &DashboardSession,
    guild_cards: &[GuildCard],
) -> String {
    let manageable_now = guild_cards
        .iter()
        .filter(|card| card.bot_presence == BotGuildPresence::Present)
        .count();
    let needs_install = guild_cards
        .iter()
        .filter(|card| card.bot_presence == BotGuildPresence::Missing)
        .count();
    let unavailable = guild_cards
        .iter()
        .filter(|card| card.bot_presence == BotGuildPresence::Unavailable)
        .count();
    let connected_markup = guild_cards
        .iter()
        .filter(|card| card.bot_presence == BotGuildPresence::Present)
        .map(render_guild_card)
        .collect::<Vec<_>>()
        .join("\n");
    let install_markup = guild_cards
        .iter()
        .filter(|card| card.bot_presence == BotGuildPresence::Missing)
        .map(render_guild_card)
        .collect::<Vec<_>>()
        .join("\n");
    let unavailable_markup = guild_cards
        .iter()
        .filter(|card| card.bot_presence == BotGuildPresence::Unavailable)
        .map(render_guild_card)
        .collect::<Vec<_>>()
        .join("\n");

    let content = format!(
        "<section class=\"hero compact dyno-hero\"><div><p class=\"eyebrow\">Server Listing</p><h1>Choose a server to manage.</h1><p class=\"lede\">Only guilds where your account has Manage Server or Administrator are shown. Connected servers can be configured immediately.</p><div class=\"actions\"><a class=\"button button-primary\" href=\"#connected-servers\">Connected Servers</a><a class=\"button button-secondary\" href=\"#install-required\">Needs Install</a></div></div><div class=\"hero-card\"><dl><div><dt>Manage Now</dt><dd>{manageable_now}</dd></div><div><dt>Needs Install</dt><dd>{needs_install}</dd></div><div><dt>Status Unavailable</dt><dd>{unavailable}</dd></div><div><dt>Total Eligible</dt><dd>{total}</dd></div></dl></div></section><section class=\"panel toolbar-panel\"><div class=\"toolbar\"><div><p class=\"eyebrow\">Guild Search</p><h2>Server Listing</h2></div><input class=\"toolbar-search\" id=\"guild-filter\" type=\"search\" aria-label=\"Search guilds\" aria-describedby=\"guild-filter-status guild-filter-empty\" placeholder=\"Search guilds\" oninput=\"filterGuildCards(this.value)\" /></div><p id=\"guild-filter-status\" class=\"filter-feedback\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\"></p><p id=\"guild-filter-empty\" class=\"filter-empty\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\" hidden>No servers match this search.</p></section><section id=\"connected-servers\" class=\"section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Connected</p><h2>Manageable Servers</h2></div><span class=\"pill pill-success\">{manageable_now}</span></div><div class=\"module-grid\">{connected_markup}</div></section><section id=\"install-required\" class=\"section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Install Required</p><h2>Servers Missing The Bot</h2></div><span class=\"pill pill-warn\">{needs_install}</span></div><div class=\"module-grid\">{install_markup}</div></section><section id=\"status-unavailable\" class=\"section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Unavailable</p><h2>Server Status Could Not Be Checked</h2></div><span class=\"pill\">{unavailable}</span></div><div class=\"module-grid\">{unavailable_markup}</div></section>",
        manageable_now = manageable_now,
        needs_install = needs_install,
        unavailable = unavailable,
        total = guild_cards.len(),
        connected_markup = if connected_markup.is_empty() {
            "<article class=\"panel empty-state\"><h3>No connected servers</h3><p>Invite the bot into one of your manageable servers to unlock guild settings here.</p></article>".to_string()
        } else {
            connected_markup
        },
        install_markup = if install_markup.is_empty() {
            "<article class=\"panel empty-state\"><h3>Nothing pending</h3><p>Every eligible server already has the bot installed.</p></article>".to_string()
        } else {
            install_markup
        },
        unavailable_markup = if unavailable_markup.is_empty() {
            "<article class=\"panel empty-state\"><h3>All statuses available</h3><p>Discord returned a current status for every eligible server.</p></article>".to_string()
        } else {
            unavailable_markup
        },
    );

    render_document(
        state,
        Some(session),
        "Server Selector",
        "Pick a guild and move into module-level controls.",
        Some("/selector"),
        None,
        &content,
    )
}

pub(crate) fn render_guild_card(card: &GuildCard) -> String {
    let badge = match card.bot_presence {
        BotGuildPresence::Present => "<span class=\"pill pill-success\">Connected</span>",
        BotGuildPresence::Missing => "<span class=\"pill pill-warn\">Install Required</span>",
        BotGuildPresence::Unavailable => "<span class=\"pill\">Status Unavailable</span>",
    };
    let action = match card.bot_presence {
        BotGuildPresence::Present => format!(
            "<a class=\"button button-primary card-action\" href=\"{}\">Manage Server</a>",
            card.manage_url
        ),
        BotGuildPresence::Missing => format!(
            "<a class=\"button button-secondary card-action\" href=\"{}\">Invite Bot</a>",
            card.invite_url
        ),
        BotGuildPresence::Unavailable => {
            "<a class=\"button button-secondary\" href=\"/selector\">Retry Status</a>".to_string()
        }
    };
    let media = card
        .icon_url
        .as_ref()
        .map(|url| {
            format!(
                "<img class=\"guild-avatar\" src=\"{}\" alt=\"{} icon\" />",
                escape_html(url),
                escape_html(&card.name)
            )
        })
        .unwrap_or_else(|| {
            format!(
                "<div class=\"guild-avatar guild-avatar-fallback\">{}</div>",
                escape_html(&initials(&card.name))
            )
        });

    format!(
        "<article class=\"panel guild-card\" data-guild-name=\"{data_name}\"><div class=\"guild-card-head\">{media}<div><h2>{name}</h2>{badge}</div></div><p>{description}</p><div class=\"guild-card-meta\"><span>Guild ID</span><code>{guild_id}</code></div>{action}</article>",
        data_name = escape_html(&card.name.to_ascii_lowercase()),
        media = media,
        name = escape_html(&card.name),
        badge = badge,
        description = match card.bot_presence {
            BotGuildPresence::Present => "Open guild-scoped module and command settings.",
            BotGuildPresence::Missing => {
                "The bot is not in this server yet. Install it first, then return here."
            }
            BotGuildPresence::Unavailable => {
                "Discord did not return the bot's current status. Try again later."
            }
        },
        guild_id = card.id,
        action = action,
    )
}

pub(crate) fn render_install_required_page(
    state: &DashboardState,
    session: &DashboardSession,
    guild: &GuildCard,
) -> String {
    let content = format!(
        "<section class=\"hero compact\"><div><p class=\"eyebrow\">Guild Setup</p><h1>{name} is not connected yet.</h1><p class=\"lede\">Install the bot into this server first. When the bot joins, this page will expose guild-level controls automatically.</p><div class=\"actions\"><a class=\"button button-primary\" href=\"{invite_url}\">Invite Bot</a><a class=\"button button-secondary\" href=\"/selector\">Back to Selector</a></div></div></section>",
        name = escape_html(&guild.name),
        invite_url = guild.invite_url,
    );

    render_document(
        state,
        Some(session),
        &format!("Install Bot: {}", guild.name),
        "This guild is eligible for management, but the bot has not been installed yet.",
        Some("/selector"),
        None,
        &content,
    )
}

pub(crate) fn render_error_page(
    state: &DashboardState,
    session: Option<&DashboardSession>,
    title: &str,
    message: &str,
) -> String {
    let content = format!(
        "<section class=\"hero compact\" role=\"alert\"><div><p class=\"eyebrow\">Dashboard</p><h1>{}</h1><p class=\"lede\">{}</p><div class=\"actions\"><a class=\"button button-primary\" href=\"\">Retry</a><a class=\"button button-secondary\" href=\"/selector\">Server Selector</a><a class=\"button button-secondary\" href=\"/\">Home</a></div></div></section>",
        escape_html(title),
        message,
    );

    render_document(state, session, title, message, None, None, &content)
}

pub(crate) fn render_document(
    state: &DashboardState,
    session: Option<&DashboardSession>,
    title: &str,
    subtitle: &str,
    active_path: Option<&str>,
    active_tab: Option<&str>,
    content: &str,
) -> String {
    let nav = render_nav(state, session, active_path, active_tab);
    let session_summary = session.map(render_session_summary).unwrap_or_else(|| {
        "<a class=\"button button-primary\" href=\"/login\">Sign in with Discord</a>".to_string()
    });
    let app_icon = state.app_info.icon.as_ref().map(|icon| {
        format!(
            "https://cdn.discordapp.com/app-icons/{}/{}.png?size=128",
            state.app_info.id, icon
        )
    });

    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" /><title>{title}</title><style>{styles}</style></head><body><div class=\"backdrop\"></div><div class=\"app-shell\" data-testid=\"dashboard-shell\"><aside class=\"sidebar\"><div class=\"sidebar-brand\">{brand_media}<div><p class=\"eyebrow\">Dynamo</p><h1>{app_name}</h1></div></div><nav class=\"sidebar-nav\">{nav}</nav><div class=\"sidebar-footer\"><span class=\"sidebar-footnote\">Rust dashboard control plane</span></div></aside><main class=\"content-shell\"><header class=\"content-topbar\"><div class=\"content-topbar-copy\"><p class=\"eyebrow\">Control Plane</p><h2>{page_title}</h2><p class=\"lede\">{subtitle}</p></div><div class=\"content-topbar-right\"><div class=\"stat-strip\"><div class=\"stat\"><span>Modules</span><strong>{module_count}</strong></div><div class=\"stat\"><span>Commands</span><strong>{command_count}</strong></div></div><div class=\"session-box\">{session_summary}</div></div></header><section class=\"content-body\" data-testid=\"content-body\">{content}</section></main></div><script>{ui_script}</script></body></html>",
        title = escape_html(title),
        styles = dashboard_styles(),
        ui_script = dashboard_ui_script(),
        brand_media =
            app_icon
                .map(|url| format!(
                    "<img class=\"app-avatar\" src=\"{}\" alt=\"app icon\" />",
                    escape_html(&url)
                ))
                .unwrap_or_else(
                    || "<div class=\"app-avatar app-avatar-fallback\">DY</div>".to_string()
                ),
        app_name = escape_html(&state.app_info.name),
        nav = nav,
        session_summary = session_summary,
        page_title = escape_html(title),
        subtitle = escape_html(subtitle),
        module_count = state.module_catalog.entries.len(),
        command_count = state.command_catalog.entries.len(),
        content = content,
    )
}

pub(crate) fn render_nav(
    state: &DashboardState,
    session: Option<&DashboardSession>,
    active_path: Option<&str>,
    active_tab: Option<&str>,
) -> String {
    let default_dashboard = "/";
    let show_section_nav = active_path
        .map(|path| path == "/deployment" || path.starts_with("/guild/"))
        .unwrap_or(false);
    let dashboard_admin = session
        .map(|session| user_is_dashboard_admin(state, &session.user))
        .unwrap_or(false);
    let server_listing_active = active_path == Some("/selector")
        || active_path.is_some_and(|path| path.starts_with("/guild/"));
    let mut items = vec![nav_link(
        "Dashboard",
        default_dashboard,
        active_path == Some("/"),
    )];
    if session.is_some() {
        items.push(nav_link(
            "Server Listing",
            "/selector",
            server_listing_active,
        ));
        if dashboard_admin {
            items.push(nav_link(
                "Deployment",
                "/deployment",
                active_path == Some("/deployment"),
            ));
        }
        if show_section_nav {
            let base_path = active_path.unwrap_or(default_dashboard);
            let subnav = [
                (
                    "Modules",
                    format!("{base_path}{}", page_query_for_tab("modules")),
                    active_tab == Some("modules"),
                ),
                (
                    "Commands",
                    format!("{base_path}{}", page_query_for_tab("commands")),
                    active_tab == Some("commands"),
                ),
                (
                    "Logs",
                    format!("{base_path}{}", page_query_for_tab("logs")),
                    active_tab == Some("logs"),
                ),
            ]
            .into_iter()
            .map(|(label, href, active)| nav_sub_link(label, &href, active))
            .collect::<Vec<_>>()
            .join("");
            items.push(format!("<div class=\"nav-submenu\">{subnav}</div>"));
        }
        items.push(nav_link("Logout", "/logout", false));
    } else {
        items.push(nav_link("Sign in", "/login", false));
    }

    items.join("")
}

pub(crate) fn render_section_tabs(base_path: &str, active_tab: &str) -> String {
    let tabs = [("overview", "Overview"), ("modules", "Modules"), ("commands", "Commands"), ("logs", "Logs")]
        .into_iter()
        .map(|(tab, label)| {
            let href = format!("{base_path}{}", page_query_for_tab(tab));
            format!(
                "<a class=\"tab-button{}\" data-testid=\"page-tab-{tab}\" href=\"{href}\"{}>{label}</a>",
                if active_tab == tab { " active" } else { "" },
                if active_tab == tab { " aria-current=\"page\"" } else { "" },
                label = escape_html(label),
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!("<div class=\"tab-row page-tab-row\">{tabs}</div>")
}

fn nav_link(label: &str, href: &str, active: bool) -> String {
    format!(
        "<a class=\"nav-link{}\" href=\"{}\"{}>{}</a>",
        if active { " active" } else { "" },
        href,
        if active { " aria-current=\"page\"" } else { "" },
        escape_html(label)
    )
}

fn nav_sub_link(label: &str, href: &str, active: bool) -> String {
    format!(
        "<a class=\"nav-sub-link{}\" href=\"{}\"{}>{}</a>",
        if active { " active" } else { "" },
        href,
        if active { " aria-current=\"page\"" } else { "" },
        escape_html(label)
    )
}

fn render_session_summary(session: &DashboardSession) -> String {
    let avatar = user_avatar_url(&session.user)
        .map(|url| {
            format!(
                "<img class=\"user-avatar\" src=\"{}\" alt=\"user avatar\" />",
                escape_html(&url)
            )
        })
        .unwrap_or_else(|| {
            format!(
                "<div class=\"user-avatar user-avatar-fallback\">{}</div>",
                escape_html(&initials(
                    session
                        .user
                        .global_name
                        .as_deref()
                        .unwrap_or(&session.user.username)
                ))
            )
        });
    let display_name = session
        .user
        .global_name
        .as_deref()
        .unwrap_or(&session.user.username);

    format!(
        "<div class=\"session-summary\">{avatar}<div><strong>{display_name}</strong><span>{username}</span></div></div>",
        avatar = avatar,
        display_name = escape_html(display_name),
        username = escape_html(&session.user.username),
    )
}

fn user_avatar_url(user: &DashboardUser) -> Option<String> {
    user.avatar.as_ref().map(|avatar| {
        format!(
            "https://cdn.discordapp.com/avatars/{}/{}.png?size=128",
            user.id, avatar
        )
    })
}

pub(crate) fn user_is_dashboard_admin(state: &DashboardState, user: &DashboardUser) -> bool {
    let mut admin_ids: HashSet<u64> = state.config.admin_user_ids.iter().copied().collect();
    if let Some(owner_id) = state.app_info.owner_user_id {
        admin_ids.insert(owner_id);
    }

    admin_ids.contains(&user.id)
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}
