//! The page shell (sidebar, top bar, breadcrumbs) and the pages that are not
//! scoped to a settings surface: landing, server selector, install prompt, and
//! errors.

use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::assets::{BOOT_SCRIPT, DASHBOARD_CSS_PATH, DASHBOARD_JS_PATH};
use crate::auth::user_can_manage_guild;
use crate::discord::guild_icon_url;
use crate::i18n::{Locale, fill};
use crate::{DashboardSession, DashboardState, DashboardUser};

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

/// Top-level destinations in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavItem {
    Home,
    Servers,
    Deployment,
}

#[derive(Debug, Clone)]
pub(crate) struct Crumb {
    pub(crate) label: String,
    pub(crate) href: Option<String>,
}

impl Crumb {
    pub(crate) fn link(label: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: Some(href.into()),
        }
    }

    pub(crate) fn current(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: None,
        }
    }
}

/// The guild a page belongs to, shown in the sidebar's server switcher.
#[derive(Debug, Clone)]
pub(crate) struct CurrentGuild {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) icon_url: Option<String>,
}

/// Everything the shell needs besides the page content.
pub(crate) struct PageContext<'a> {
    pub(crate) state: &'a DashboardState,
    pub(crate) session: Option<&'a DashboardSession>,
    pub(crate) locale: Locale,
    pub(crate) title: String,
    pub(crate) nav: Option<NavItem>,
    pub(crate) breadcrumbs: Vec<Crumb>,
    pub(crate) guild: Option<CurrentGuild>,
    /// Path and query of the current page, used to return after switching language.
    pub(crate) current_path: String,
}

impl<'a> PageContext<'a> {
    pub(crate) fn new(
        state: &'a DashboardState,
        session: Option<&'a DashboardSession>,
        locale: Locale,
        title: impl Into<String>,
        current_path: impl Into<String>,
    ) -> Self {
        Self {
            state,
            session,
            locale,
            title: title.into(),
            nav: None,
            breadcrumbs: Vec::new(),
            guild: None,
            current_path: current_path.into(),
        }
    }

    pub(crate) fn nav(mut self, nav: NavItem) -> Self {
        self.nav = Some(nav);
        self
    }

    pub(crate) fn crumbs(mut self, crumbs: Vec<Crumb>) -> Self {
        self.breadcrumbs = crumbs;
        self
    }

    pub(crate) fn guild(mut self, guild: CurrentGuild) -> Self {
        self.guild = Some(guild);
        self
    }
}

pub(crate) fn avatar(url: Option<&str>, name: &str, size_class: &str) -> Markup {
    html! {
        @if let Some(url) = url {
            img class={ "avatar " (size_class) } src=(url) alt="" width="40" height="40" loading="lazy" decoding="async";
        } @else {
            span class={ "avatar " (size_class) } aria-hidden="true" { (initials(name)) }
        }
    }
}

pub(crate) fn render_document(ctx: &PageContext<'_>, content: Markup) -> String {
    let t = ctx.locale.t();
    let app_name = ctx.state.app_info.name.as_str();
    let app_icon = ctx.state.app_info.icon.as_ref().map(|icon| {
        format!(
            "https://cdn.discordapp.com/app-icons/{}/{}.png?size=128",
            ctx.state.app_info.id, icon
        )
    });
    let is_owner = ctx
        .session
        .is_some_and(|session| crate::owner_auth::is_application_owner(ctx.state, session));
    let title = if ctx.title.is_empty() || ctx.title == app_name {
        app_name.to_string()
    } else {
        format!("{} · {}", ctx.title, app_name)
    };

    let markup = html! {
        (DOCTYPE)
        html lang=(ctx.locale.code()) class="no-js" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="dark";
                meta name="description" content=(fill(t.meta_description, &[("app", &app_name)]));
                @if let Some(session) = ctx.session {
                    meta name="csrf-token" content=(session.csrf_token);
                }
                title { (title) }
                link rel="icon" href=(FAVICON_DATA_URI);
                link rel="stylesheet" href=(DASHBOARD_CSS_PATH);
                script { (PreEscaped(BOOT_SCRIPT)) }
                script src=(DASHBOARD_JS_PATH) defer {}
            }
            body {
                a.skip-link href="#main-content" { (t.skip_to_content) }
                div.app-shell data-testid="dashboard-shell" {
                    aside #sidebar .sidebar aria-label=(t.nav_primary_label) {
                        div.sidebar-head {
                            a.sidebar-brand href="/" {
                                (avatar(app_icon.as_deref(), app_name, ""))
                                span.sidebar-brand-name { (app_name) }
                            }
                            button.icon-button.sidebar-close type="button" data-action="close-nav" aria-label=(t.nav_close_menu) {
                                span.icon-close aria-hidden="true" { "×" }
                            }
                        }
                        @if let (Some(guild), Some(session)) = (&ctx.guild, ctx.session) {
                            (render_guild_switcher(ctx.locale, guild, session))
                        }
                        nav.sidebar-nav aria-label=(t.nav_primary_label) {
                            (nav_link(t.nav_home, "/", ctx.nav == Some(NavItem::Home)))
                            @if ctx.session.is_some() {
                                (nav_link(t.nav_servers, "/selector", ctx.nav == Some(NavItem::Servers)))
                                @if is_owner {
                                    (nav_link(t.nav_deployment, "/deployment", ctx.nav == Some(NavItem::Deployment)))
                                }
                            }
                        }
                        div.sidebar-footer {
                            div {
                                p.sidebar-section-label id="language-label" { (t.language_label) }
                                div.lang-switch role="group" aria-labelledby="language-label" {
                                    @for locale in Locale::ALL {
                                        a href=(language_href(locale, &ctx.current_path))
                                            hreflang=(locale.code())
                                            lang=(locale.code())
                                            aria-current=[(locale == ctx.locale).then_some("true")] {
                                            (locale.native_name())
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div.sidebar-scrim data-action="close-nav" {}
                    div.content-shell {
                        header.topbar {
                            button.icon-button.nav-toggle type="button" data-action="open-nav"
                                aria-controls="sidebar" aria-expanded="false" aria-label=(t.nav_open_menu) {
                                span.icon-bars aria-hidden="true" {}
                            }
                            @if !ctx.breadcrumbs.is_empty() {
                                nav.breadcrumb aria-label=(t.breadcrumb_label) {
                                    ol {
                                        @for crumb in &ctx.breadcrumbs {
                                            li {
                                                @if let Some(href) = &crumb.href {
                                                    a href=(href) { (crumb.label) }
                                                } @else {
                                                    span aria-current="page" { (crumb.label) }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            div.topbar-user {
                                @if let Some(session) = ctx.session {
                                    (render_session_summary(session))
                                    a.button.button-ghost.button-compact href="/logout" { (t.nav_logout) }
                                } @else {
                                    a.button.button-primary.button-compact href="/login" { (t.nav_sign_in) }
                                }
                            }
                        }
                        main #main-content .content-body data-testid="content-body" tabindex="-1" {
                            (content)
                        }
                    }
                }
                div #toast-region .toast-region aria-live="polite" aria-relevant="additions" {}
                div #modal-root {}
                script #dynamo-i18n type="application/json" {
                    (PreEscaped(ctx.locale.client().to_json(ctx.locale)))
                }
            }
        }
    };
    markup.into_string()
}

/// A small inline SVG mark so the browser never requests `/favicon.ico`.
const FAVICON_DATA_URI: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='8' fill='%23bc173d'/%3E%3Cpath d='M10 8h6a8 8 0 0 1 0 16h-6z' fill='none' stroke='%23fff6fa' stroke-width='3'/%3E%3C/svg%3E";

fn language_href(locale: Locale, current_path: &str) -> String {
    let mut url = url::Url::parse("http://local/").expect("static base URL");
    url.set_path(&format!("/lang/{}", locale.code()));
    url.query_pairs_mut().append_pair("redirect", current_path);
    format!("{}?{}", url.path(), url.query().unwrap_or_default())
}

fn nav_link(label: &str, href: &str, active: bool) -> Markup {
    html! {
        a.nav-link.active[active] href=(href) aria-current=[active.then_some("page")] { (label) }
    }
}

fn render_guild_switcher(
    locale: Locale,
    guild: &CurrentGuild,
    session: &DashboardSession,
) -> Markup {
    let t = locale.t();
    let mut guilds = session
        .guilds
        .iter()
        .filter(|candidate| user_can_manage_guild(candidate))
        .collect::<Vec<_>>();
    guilds.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    html! {
        details.guild-switcher data-testid="guild-switcher" {
            summary aria-label=(format!("{}: {}", t.guild_switcher_label, guild.name)) {
                (avatar(guild.icon_url.as_deref(), &guild.name, ""))
                span.guild-switcher-current {
                    strong { (guild.name) }
                    span { (t.guild_switcher_label) }
                }
            }
            ul.guild-switcher-menu {
                @for candidate in guilds {
                    li {
                        @let current = candidate.id == guild.id;
                        a href=(format!("/guild/{}", candidate.id)) aria-current=[current.then_some("page")] {
                            (avatar(guild_icon_url(candidate).as_deref(), &candidate.name, "avatar-sm"))
                            span { (candidate.name) }
                        }
                    }
                }
                li.guild-switcher-all {
                    a href="/selector" { span { (t.guild_switcher_all) } }
                }
            }
        }
    }
}

fn render_session_summary(session: &DashboardSession) -> Markup {
    let display_name = session
        .user
        .global_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&session.user.username);
    html! {
        div.session-summary {
            (avatar(user_avatar_url(&session.user).as_deref(), display_name, "avatar-sm"))
            div.session-summary-text {
                strong { (display_name) }
                span { "@" (session.user.username) }
            }
        }
    }
}

pub(crate) fn render_landing_page(
    state: &DashboardState,
    session: Option<&DashboardSession>,
    locale: Locale,
) -> String {
    let t = locale.t();
    let app_name = state.app_info.name.as_str();
    let runtime_notices =
        crate::render_runtime_notices(&state.module_catalog, &Default::default(), locale);
    let content = html! {
        section.hero {
            div {
                h1 { (fill(t.landing_title, &[("app", &app_name)])) }
                p.lede {
                    @if session.is_some() { (t.landing_lede_signed_in) } @else { (t.landing_lede_signed_out) }
                }
                div.actions {
                    @if session.is_some() {
                        a.button.button-primary href="/selector" { (t.landing_choose_server) }
                    } @else {
                        a.button.button-primary href="/login" { (t.nav_sign_in) }
                    }
                }
            }
            dl.hero-stats {
                div.stat { dt { (t.stat_modules) } dd { (state.module_catalog.entries.len()) } }
                div.stat { dt { (t.stat_commands) } dd { (state.command_catalog.entries.len()) } }
            }
        }
        div.feature-grid {
            article.panel {
                h2 { (t.landing_feature_control_title) }
                p { (t.landing_feature_control_body) }
            }
            article.panel {
                h2 { (t.landing_feature_status_title) }
                p { (t.landing_feature_status_body) }
            }
            article.panel {
                h2 { (t.landing_feature_audit_title) }
                p { (t.landing_feature_audit_body) }
            }
        }
        @if let Some(notices) = runtime_notices {
            (notices)
        }
    };

    let ctx = PageContext::new(state, session, locale, app_name, "/")
        .nav(NavItem::Home)
        .crumbs(vec![Crumb::current(t.nav_home)]);
    render_document(&ctx, content)
}

pub(crate) fn render_selector_page(
    state: &DashboardState,
    session: &DashboardSession,
    locale: Locale,
    guild_cards: &[GuildCard],
) -> String {
    let t = locale.t();
    let by_presence = |presence| {
        guild_cards
            .iter()
            .filter(move |card| card.bot_presence == presence)
            .collect::<Vec<_>>()
    };
    let connected = by_presence(BotGuildPresence::Present);
    let missing = by_presence(BotGuildPresence::Missing);
    let unavailable = by_presence(BotGuildPresence::Unavailable);

    let content = html! {
        div.page-header {
            div.page-header-text {
                h1 { (t.selector_title) }
                p.lede { (t.selector_lede) }
            }
        }
        @if guild_cards.is_empty() {
            div.empty-state {
                h2 { (t.selector_none_title) }
                p { (t.selector_none_body) }
            }
        } @else {
            div.panel.stack data-filter-scope="guild" {
                div.list-toolbar {
                    div.search-field {
                        label.visually-hidden for="guild-filter" { (t.selector_search) }
                        input #guild-filter .search-input type="search" data-filter-input
                            placeholder=(t.selector_search) autocomplete="off"
                            aria-describedby="guild-filter-status";
                    }
                }
                p #guild-filter-status .filter-feedback data-filter-status role="status" aria-live="polite" aria-atomic="true" {}
                p.empty-state.filter-empty data-filter-empty hidden { (t.selector_no_match) }
                section #connected-servers .selector-section aria-labelledby="connected-heading" {
                    div.section-heading {
                        h2 #connected-heading { (t.selector_ready) }
                        span.count-badge { (connected.len()) }
                    }
                    @if connected.is_empty() {
                        div.empty-state {
                            h3 { (t.selector_empty_title) }
                            p { (t.selector_empty_body) }
                        }
                    } @else {
                        div.guild-grid { @for card in &connected { (render_guild_card(card, locale)) } }
                    }
                }
                @if !missing.is_empty() {
                    details #install-required .selector-section open[connected.is_empty()] {
                        summary {
                            div.section-heading {
                                h2 { (t.selector_not_installed) }
                                span.count-badge { (missing.len()) }
                            }
                        }
                        div.guild-grid { @for card in &missing { (render_guild_card(card, locale)) } }
                    }
                }
                @if !unavailable.is_empty() {
                    section #status-unavailable .selector-section aria-labelledby="unavailable-heading" {
                        div.section-heading {
                            h2 #unavailable-heading { (t.selector_unavailable) }
                            span.count-badge { (unavailable.len()) }
                        }
                        div.guild-grid { @for card in &unavailable { (render_guild_card(card, locale)) } }
                    }
                }
            }
        }
    };

    let ctx = PageContext::new(state, Some(session), locale, t.selector_title, "/selector")
        .nav(NavItem::Servers)
        .crumbs(vec![
            Crumb::link(t.nav_home, "/"),
            Crumb::current(t.nav_servers),
        ]);
    render_document(&ctx, content)
}

pub(crate) fn render_guild_card(card: &GuildCard, locale: Locale) -> Markup {
    let t = locale.t();
    let (tone, badge) = match card.bot_presence {
        BotGuildPresence::Present => ("success", t.guild_badge_connected),
        BotGuildPresence::Missing => ("warn", t.guild_badge_missing),
        BotGuildPresence::Unavailable => ("neutral", t.guild_badge_unavailable),
    };
    html! {
        article.guild-card data-filter-item data-search=(card.name.to_lowercase()) data-guild-name=(card.name.to_lowercase()) {
            div.guild-card-head {
                (avatar(card.icon_url.as_deref(), &card.name, ""))
                div {
                    h3 { (card.name) }
                    span.badge data-tone=(tone) { (badge) }
                }
            }
            @if card.bot_presence == BotGuildPresence::Unavailable {
                p { (t.guild_unavailable_body) }
            }
            span.guild-card-id { (t.guild_id_label) " " code { (card.id) } }
            @match card.bot_presence {
                BotGuildPresence::Present => {
                    a.button.button-primary.button-compact href=(card.manage_url) { (t.guild_manage) }
                }
                BotGuildPresence::Missing => {
                    a.button.button-secondary.button-compact href=(card.invite_url) { (t.guild_invite) }
                }
                BotGuildPresence::Unavailable => {
                    a.button.button-secondary.button-compact href="/selector" { (t.guild_retry) }
                }
            }
        }
    }
}

pub(crate) fn render_install_required_page(
    state: &DashboardState,
    session: &DashboardSession,
    locale: Locale,
    guild: &GuildCard,
) -> String {
    let t = locale.t();
    let content = html! {
        section.hero {
            div {
                h1 { (fill(t.install_title, &[("name", &guild.name)])) }
                p.lede { (t.install_lede) }
                div.actions {
                    a.button.button-primary href=(guild.invite_url) { (t.guild_invite) }
                    a.button.button-secondary href="/selector" { (t.back_to_servers) }
                }
            }
        }
    };
    let ctx = PageContext::new(
        state,
        Some(session),
        locale,
        guild.name.clone(),
        format!("/guild/{}", guild.id),
    )
    .nav(NavItem::Servers)
    .crumbs(vec![
        Crumb::link(t.nav_servers, "/selector"),
        Crumb::current(guild.name.clone()),
    ]);
    render_document(&ctx, content)
}

/// Renders an error page. `message` is plain text and is escaped.
pub(crate) fn render_error_page(
    state: &DashboardState,
    session: Option<&DashboardSession>,
    locale: Locale,
    title: &str,
    message: &str,
) -> String {
    let t = locale.t();
    let content = html! {
        section.hero role="alert" {
            div {
                h1 { (title) }
                p.lede { (message) }
                div.actions {
                    a.button.button-primary href="" { (t.error_retry) }
                    @if session.is_some() {
                        a.button.button-secondary href="/selector" { (t.nav_servers) }
                    }
                    a.button.button-secondary href="/" { (t.nav_home) }
                }
            }
        }
    };
    let ctx = PageContext::new(state, session, locale, title, "/")
        .crumbs(vec![Crumb::link(t.nav_home, "/"), Crumb::current(title)]);
    render_document(&ctx, content)
}

/// Section navigation for guild pages.
pub(crate) fn render_section_tabs(locale: Locale, base_path: &str, active_tab: &str) -> Markup {
    let t = locale.t();
    let tabs = [
        ("overview", t.tab_overview),
        ("modules", t.tab_modules),
        ("commands", t.tab_commands),
        ("logs", t.tab_logs),
    ];
    html! {
        nav.tab-row.page-tab-row aria-label=(t.sections_label) {
            @for (tab, label) in tabs {
                @let active = active_tab == tab;
                a.tab-button.active[active] data-testid={ "page-tab-" (tab) }
                    href=(format!("{base_path}{}", crate::page_query_for_tab(tab)))
                    aria-current=[active.then_some("page")] { (label) }
            }
        }
    }
}

/// Section navigation for the 7-tab app-owner console.
pub(crate) fn render_owner_section_tabs(locale: Locale, active_tab: &str) -> Markup {
    let t = locale.t();
    let tabs = [
        ("overview", t.tab_overview),
        ("servers", t.tab_servers),
        ("system", t.tab_system),
        ("notices", t.tab_notices),
        ("modules", t.tab_modules),
        ("commands", t.tab_commands),
        ("logs", t.tab_logs),
    ];
    html! {
        nav.tab-row.page-tab-row aria-label=(t.sections_label) {
            @for (tab, label) in tabs {
                @let active = active_tab == tab;
                a.tab-button.active[active] data-testid={ "page-tab-" (tab) }
                    href=(format!("/deployment{}", crate::page_query_for_tab(tab)))
                    aria-current=[active.then_some("page")] { (label) }
            }
        }
    }
}

fn user_avatar_url(user: &DashboardUser) -> Option<String> {
    user.avatar.as_ref().map(|avatar| {
        format!(
            "https://cdn.discordapp.com/avatars/{}/{}.png?size=64",
            user.id, avatar
        )
    })
}

pub(crate) fn initials(name: &str) -> String {
    let letters = name
        .split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    if letters.is_empty() {
        "?".to_string()
    } else {
        letters
    }
}
