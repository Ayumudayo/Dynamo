//! Browser-rendered dashboard routes, page composition, and query parsing.

use std::collections::HashMap;

use axum::http::{HeaderValue, header};
#[cfg(feature = "local-preview")]
use chrono::Utc;
use dynamo_module_kit::SettingsSchema;
use dynamo_ops::{
    ANNOUNCEMENT_QUEUE_PROVIDER_ID, BOT_HEARTBEAT_PROVIDER_ID, BotHeartbeatSnapshot,
    OwnerAnnouncementStore,
};
use maud::html;

use crate::i18n::{LOCALE_COOKIE_NAME, Locale, fill};
use crate::owner_auth::{
    OwnerUnlockError, clear_owner_elevation_cookie, is_application_owner, load_valid_elevation,
    owner_elevation_cookie, revoke_elevation, verify_live_owner_identity,
    verify_mutation_request_security, verify_unlock_totp,
};
#[cfg(feature = "local-preview")]
use crate::owner_auth::{current_totp_step, totp_code_for_step};
use crate::render::document::{
    Crumb, CurrentGuild, NavItem, PageContext, avatar, render_document, render_owner_section_tabs,
    render_section_tabs,
};
use crate::render::entities::{
    EntityRow, OverviewData, command_rows, module_rows, patch_url, render_audit_logs_section,
    render_entity_section, render_overview, render_owner_audit_logs_section, render_scope_header,
    render_scope_page,
};
use crate::render::forms::{DialogSpec, DialogToggle, render_settings_dialog};
use crate::render::owner::{
    OwnerServerRow, OwnerSystemData, render_guild_announcement_card, render_owner_notices_tab,
    render_owner_overview_extras, render_owner_scope_header, render_owner_servers_tab,
    render_owner_system_tab, render_owner_totp_setup_page, render_owner_totp_unlock_page,
};
use crate::status::{EntityKind, Scope};
use crate::*;

pub(crate) async fn index(
    jar: CookieJar,
    locale: Locale,
    State(state): State<Arc<DashboardState>>,
) -> Response {
    let session = load_session(&state, &jar).await;
    Html(render_landing_page(&state, session.as_ref(), locale)).into_response()
}

#[derive(Debug, Deserialize)]
pub(crate) struct LanguageQuery {
    redirect: Option<String>,
}

/// Stores the chosen language in a cookie and returns to the previous page.
pub(crate) async fn set_language(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Path(code): Path<String>,
    axum::extract::Query(query): axum::extract::Query<LanguageQuery>,
) -> Response {
    let target = match query.redirect.as_deref() {
        Some(redirect) => sanitize_redirect_target(Some(redirect)),
        None => "/".to_string(),
    };
    let Some(locale) = Locale::from_code(&code) else {
        return Redirect::to(&target).into_response();
    };
    let secure_attr = if state.config.cookies_are_secure() {
        "; Secure"
    } else {
        ""
    };
    let Ok(cookie) = Cookie::parse(format!(
        "{LOCALE_COOKIE_NAME}={}; Path=/; Max-Age=31536000; SameSite=Lax{secure_attr}",
        locale.code()
    )) else {
        return Redirect::to(&target).into_response();
    };
    (jar.add(cookie.into_owned()), Redirect::to(&target)).into_response()
}

pub(crate) async fn login(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    axum::extract::Query(query): axum::extract::Query<LoginQuery>,
) -> Response {
    if load_session(&state, &jar).await.is_some() {
        let target = sanitize_redirect_target(query.redirect.as_deref());
        return Redirect::to(&target).into_response();
    }

    #[cfg(feature = "local-preview")]
    if let Some(preview) = state.local_preview.as_ref() {
        let target = sanitize_redirect_target(query.redirect.as_deref());
        let session_id = random_token(64);
        {
            let mut sessions = state.sessions.write().await;
            sessions.retain(|_, value| !is_session_expired(value));
            sessions.insert(session_id.clone(), preview.new_session());
        }
        let jar = jar.add(session_cookie(
            &session_id,
            state.config.cookies_are_secure(),
        ));
        return (jar, Redirect::to(&target)).into_response();
    }

    let state_token = random_token(48);
    let redirect_to = sanitize_redirect_target(query.redirect.as_deref());
    {
        let mut pending = state.oauth_states.write().await;
        pending.retain(|_, value| !is_oauth_state_expired(value));
        pending.insert(
            state_token.clone(),
            PendingOauthState {
                redirect_to,
                created_at: chrono::Utc::now(),
            },
        );
    }

    Redirect::to(&build_discord_authorize_url(&state, &state_token)).into_response()
}

pub(crate) async fn discord_callback(
    jar: CookieJar,
    locale: Locale,
    State(state): State<Arc<DashboardState>>,
    axum::extract::Query(query): axum::extract::Query<DiscordCallbackQuery>,
) -> Response {
    let t = locale.t();
    let failed = |message: &str| {
        Html(render_error_page(
            &state,
            None,
            locale,
            t.error_login_failed,
            message,
        ))
        .into_response()
    };
    if let Some(error) = query.error {
        return failed(&fill(t.error_oauth_returned, &[("error", &error)]));
    }

    let Some(code) = query.code.as_deref() else {
        return failed(t.error_missing_code);
    };

    let Some(oauth_state) = query.state.as_deref() else {
        return failed(t.error_missing_state);
    };

    let pending = {
        let mut states = state.oauth_states.write().await;
        states.retain(|_, value| !is_oauth_state_expired(value));
        states.remove(oauth_state)
    };

    let Some(pending) = pending else {
        return failed(t.error_state_expired);
    };

    match exchange_oauth_code(&state, code).await {
        Ok(session) => {
            let session_id = random_token(64);
            {
                let mut sessions = state.sessions.write().await;
                sessions.retain(|_, value| !is_session_expired(value));
                sessions.insert(session_id.clone(), session);
            }

            let jar = jar.add(session_cookie(
                &session_id,
                state.config.cookies_are_secure(),
            ));
            (jar, Redirect::to(&pending.redirect_to)).into_response()
        }
        Err(error) => {
            warn!(?error, "failed to complete Discord OAuth callback");
            failed(t.error_exchange_failed)
        }
    }
}

pub(crate) async fn logout(jar: CookieJar, State(state): State<Arc<DashboardState>>) -> Response {
    let session = load_session(&state, &jar).await;
    revoke_elevation(&state, &jar, session.as_ref()).await;
    if let Some(cookie) = jar.get(SESSION_COOKIE_NAME) {
        state.sessions.write().await.remove(cookie.value());
    }

    let jar = jar
        .remove(Cookie::from(SESSION_COOKIE_NAME))
        .add(clear_owner_elevation_cookie(
            state.config.cookies_are_secure(),
        ));
    (jar, Redirect::to("/")).into_response()
}

#[derive(Debug, Deserialize)]
pub(crate) struct OwnerVerifyForm {
    csrf_token: String,
    totp_code: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OwnerLockForm {
    csrf_token: String,
}

pub(crate) async fn verify_owner_totp(
    jar: CookieJar,
    headers: HeaderMap,
    locale: Locale,
    State(state): State<Arc<DashboardState>>,
    axum::extract::Form(form): axum::extract::Form<OwnerVerifyForm>,
) -> Response {
    let t = locale.t();
    let Some(session) = load_session(&state, &jar).await else {
        return Redirect::to("/login?redirect=%2Fdeployment").into_response();
    };
    if !is_application_owner(&state, &session) {
        return error_response(
            StatusCode::FORBIDDEN,
            &state,
            Some(&session),
            locale,
            t.error_admin_only_title,
            t.error_admin_only_body,
        );
    }
    if let Err(response) =
        verify_mutation_request_security(&headers, &session, Some(&form.csrf_token))
    {
        return response;
    }
    if let Err(status) = verify_live_owner_identity(&state, &session).await {
        return match status {
            StatusCode::UNAUTHORIZED => {
                Redirect::to("/login?redirect=%2Fdeployment").into_response()
            }
            StatusCode::FORBIDDEN => error_response(
                StatusCode::FORBIDDEN,
                &state,
                Some(&session),
                locale,
                t.error_admin_only_title,
                t.error_admin_only_body,
            ),
            _ => error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &state,
                Some(&session),
                locale,
                t.error_deployment_unavailable_title,
                t.error_deployment_unavailable_body,
            ),
        };
    }
    match verify_unlock_totp(&state, &session, &form.totp_code).await {
        Ok(token) => (
            jar.add(owner_elevation_cookie(
                &token,
                state.config.cookies_are_secure(),
            )),
            Redirect::to("/deployment"),
        )
            .into_response(),
        Err(OwnerUnlockError::NotConfigured) => Redirect::to("/deployment").into_response(),
        Err(OwnerUnlockError::InvalidCode) => {
            Redirect::to("/deployment?auth_notice=invalid").into_response()
        }
        Err(OwnerUnlockError::ReplayedCode) => {
            Redirect::to("/deployment?auth_notice=replayed").into_response()
        }
        Err(OwnerUnlockError::RateLimited) => {
            Redirect::to("/deployment?auth_notice=rate_limited").into_response()
        }
    }
}

pub(crate) async fn lock_owner_console(
    jar: CookieJar,
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    axum::extract::Form(form): axum::extract::Form<OwnerLockForm>,
) -> Response {
    let Some(session) = load_session(&state, &jar).await else {
        return Redirect::to("/").into_response();
    };
    if let Err(response) =
        verify_mutation_request_security(&headers, &session, Some(&form.csrf_token))
    {
        return response;
    }
    revoke_elevation(&state, &jar, Some(&session)).await;
    (
        jar.add(clear_owner_elevation_cookie(
            state.config.cookies_are_secure(),
        )),
        Redirect::to("/deployment"),
    )
        .into_response()
}

pub(crate) async fn selector(
    jar: CookieJar,
    locale: Locale,
    State(state): State<Arc<DashboardState>>,
) -> Response {
    let started_at = std::time::Instant::now();
    let Some(session) = load_session(&state, &jar).await else {
        tracing::info!(
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            outcome = "login_required",
            "dashboard selector request completed"
        );
        return Redirect::to("/login?redirect=%2Fselector").into_response();
    };
    tracing::info!(
        elapsed_ms = started_at.elapsed().as_millis() as u64,
        guilds = session.guilds.len(),
        "dashboard selector using cached guild snapshot"
    );

    let cards_started_at = std::time::Instant::now();
    let guild_cards = load_guild_cards(&state, &session).await;
    tracing::info!(
        elapsed_ms = cards_started_at.elapsed().as_millis() as u64,
        guild_cards = guild_cards.len(),
        "dashboard selector guild status lookups completed"
    );
    let render_started_at = std::time::Instant::now();
    let response =
        Html(render_selector_page(&state, &session, locale, &guild_cards)).into_response();
    tracing::info!(
        elapsed_ms = render_started_at.elapsed().as_millis() as u64,
        "dashboard selector HTML rendered"
    );
    tracing::info!(
        elapsed_ms = started_at.elapsed().as_millis() as u64,
        outcome = "success",
        "dashboard selector request completed"
    );
    response
}

fn error_response(
    status: StatusCode,
    state: &DashboardState,
    session: Option<&DashboardSession>,
    locale: Locale,
    title: &str,
    message: &str,
) -> Response {
    (
        status,
        Html(render_error_page(state, session, locale, title, message)),
    )
        .into_response()
}

/// Looks up the display name for an audit log target.
fn entity_name_lookup(
    state: &DashboardState,
    locale: Locale,
) -> impl Fn(DashboardAuditEntityType, &str) -> String + '_ {
    move |entity_type, entity_id| match entity_type {
        DashboardAuditEntityType::Module => state
            .module_catalog
            .entries
            .iter()
            .find(|entry| entry.module.id == entity_id)
            .map(|entry| locale.catalog(entry.module.display_name).to_string())
            .unwrap_or_else(|| entity_id.to_string()),
        DashboardAuditEntityType::Command => state
            .command_catalog
            .find_by_id(entity_id)
            .map(|entry| entry.command.display_name.clone())
            .unwrap_or_else(|| entity_id.to_string()),
        DashboardAuditEntityType::Guild => format!("#{entity_id}"),
        DashboardAuditEntityType::Operations => locale.t().audit_entity_operations.to_string(),
    }
}

async fn load_audit_page(
    state: &DashboardState,
    scope: Scope,
    entity_type: Option<DashboardAuditEntityType>,
    action: Option<DashboardAuditAction>,
    page: u64,
    page_size: u64,
) -> DashboardAuditLogPage {
    let query = DashboardAuditLogQuery {
        scope: Some(match scope {
            Scope::Deployment => DashboardAuditScope::Deployment,
            Scope::Guild(_) => DashboardAuditScope::Guild,
        }),
        guild_id: scope.guild_id(),
        entity_type,
        action,
        page,
        page_size,
    };
    match state.persistence.list_dashboard_audit_logs(query).await {
        Ok(page) => page,
        Err(error) => {
            warn!(?error, guild_id = ?scope.guild_id(), "failed to load dashboard audit logs");
            DashboardAuditLogPage::empty(page, page_size)
        }
    }
}

fn scope_crumbs(
    locale: Locale,
    root: Crumb,
    scope_label: &str,
    base: &str,
    tab: &str,
) -> Vec<Crumb> {
    let t = locale.t();
    let tab_label = match tab {
        "servers" => t.tab_servers,
        "system" => t.tab_system,
        "notices" => t.tab_notices,
        "modules" => t.tab_modules,
        "commands" => t.tab_commands,
        "logs" => t.tab_logs,
        _ => "",
    };
    if tab_label.is_empty() {
        vec![root, Crumb::current(scope_label)]
    } else {
        vec![
            root,
            Crumb::link(scope_label, base),
            Crumb::current(tab_label),
        ]
    }
}

pub(crate) async fn deployment_page(
    jar: CookieJar,
    locale: Locale,
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<DashboardPageQuery>,
) -> Response {
    let t = locale.t();
    let Some(session) = load_session(&state, &jar).await else {
        return Redirect::to("/login?redirect=%2Fdeployment").into_response();
    };
    if !is_application_owner(&state, &session) {
        return error_response(
            StatusCode::FORBIDDEN,
            &state,
            Some(&session),
            locale,
            t.error_admin_only_title,
            t.error_admin_only_body,
        );
    }
    let Some(secret) = state.config.owner_totp_secret.as_deref() else {
        return Html(render_owner_totp_setup_page(&state, &session, locale)).into_response();
    };
    match load_valid_elevation(&state, &jar, &session).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            #[cfg(feature = "local-preview")]
            let preview_code = state
                .local_preview
                .as_ref()
                .map(|_| totp_code_for_step(secret, current_totp_step(Utc::now())));
            #[cfg(not(feature = "local-preview"))]
            let preview_code: Option<String> = {
                let _ = secret;
                None
            };
            return Html(render_owner_totp_unlock_page(
                &state,
                &session,
                locale,
                query.auth_notice.as_deref(),
                preview_code.as_deref(),
            ))
            .into_response();
        }
        Err(StatusCode::UNAUTHORIZED) => {
            return Redirect::to("/login?redirect=%2Fdeployment").into_response();
        }
        Err(StatusCode::FORBIDDEN) => {
            return error_response(
                StatusCode::FORBIDDEN,
                &state,
                Some(&session),
                locale,
                t.error_admin_only_title,
                t.error_admin_only_body,
            );
        }
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &state,
                Some(&session),
                locale,
                t.error_deployment_unavailable_title,
                t.error_deployment_unavailable_body,
            );
        }
    }

    let scope = Scope::Deployment;
    let active_tab = normalized_owner_tab(query.tab.as_deref());
    let names = entity_name_lookup(&state, locale);
    let section = if active_tab == "logs" {
        let log_scope = parse_audit_scope_filter(query.log_scope.as_deref());
        let log_entity = parse_owner_audit_entity_filter(query.log_entity.as_deref());
        let log_action = parse_owner_audit_action_filter(query.log_action.as_deref());
        let log_page = query.log_page.unwrap_or(1).max(1);
        let audit_query = DashboardAuditLogQuery {
            scope: log_scope,
            guild_id: None,
            entity_type: log_entity,
            action: log_action,
            page: log_page,
            page_size: 20,
        };
        let logs_page = match state
            .persistence
            .list_dashboard_audit_logs(audit_query)
            .await
        {
            Ok(page) => page,
            Err(error) => {
                warn!(?error, "failed to load owner console audit logs");
                DashboardAuditLogPage::empty(log_page, 20)
            }
        };
        render_owner_audit_logs_section(
            "/deployment",
            &logs_page,
            log_scope,
            log_entity,
            log_action,
            locale,
            &names,
        )
    } else if active_tab == "system" {
        let heartbeat = state
            .persistence
            .load_provider_state(BOT_HEARTBEAT_PROVIDER_ID)
            .await
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_value::<BotHeartbeatSnapshot>(value).ok());
        let system_data = OwnerSystemData {
            heartbeat,
            mongo_configured: state.persistence.database_name.is_some(),
            app_owner_configured: state.app_info.owner_user_id.is_some(),
            totp_configured: state.config.owner_totp_secret.is_some(),
            secure_cookies: state.config.cookies_are_secure(),
            register_globally: state.config.register_globally,
        };
        render_owner_system_tab(&system_data, locale)
    } else {
        let settings = match state.persistence.deployment_settings_or_default().await {
            Ok(settings) => settings,
            Err(_) => {
                warn!("failed to load deployment settings page");
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &state,
                    Some(&session),
                    locale,
                    t.error_deployment_unavailable_title,
                    t.error_deployment_unavailable_body,
                );
            }
        };
        match active_tab {
            "servers" => {
                let bot_guilds = match crate::discord::fetch_bot_guilds(&state).await {
                    Ok(guilds) => guilds,
                    Err(error) => {
                        warn!(?error, "failed to load bot guilds for owner servers tab");
                        Vec::new()
                    }
                };
                let all_guild_settings: HashMap<u64, GuildSettings> = state
                    .persistence
                    .list_all_guild_settings()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|g| (g.guild_id, g))
                    .collect();
                let sync_store = load_command_sync_store(&state.persistence).await;
                let mut rows = bot_guilds
                    .into_iter()
                    .map(|guild| OwnerServerRow {
                        guild_id: guild.id,
                        name: guild.name,
                        blocked: settings.blocked_guild_ids.contains(&guild.id),
                        announcement_channel_id: all_guild_settings
                            .get(&guild.id)
                            .and_then(|s| s.announcement_channel_id),
                        sync_state: sync_store.guild(guild.id).cloned(),
                    })
                    .collect::<Vec<_>>();
                rows.sort_by_key(|a| a.name.to_lowercase());
                render_owner_servers_tab(&rows, locale)
            }
            "notices" => {
                let all_guilds = state
                    .persistence
                    .list_all_guild_settings()
                    .await
                    .unwrap_or_default();
                let ready_servers = all_guilds
                    .iter()
                    .filter(|g| g.announcement_channel_id.is_some())
                    .count();
                let total_known_servers = all_guilds.len();
                let store: OwnerAnnouncementStore = state
                    .persistence
                    .load_provider_state(ANNOUNCEMENT_QUEUE_PROVIDER_ID)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default();
                render_owner_notices_tab(
                    &state,
                    &settings,
                    &store,
                    ready_servers,
                    total_known_servers,
                    locale,
                )
            }
            _ => {
                let modules = module_rows(
                    &state.module_catalog,
                    &resolve_module_states(&state.module_catalog, &settings, None),
                    scope,
                    locale,
                );
                let commands = command_rows(
                    &state.command_catalog,
                    &resolve_command_states(
                        &state.module_catalog,
                        &state.command_catalog,
                        &settings,
                        None,
                    ),
                    scope,
                    locale,
                );
                let sync_panel = async {
                    if state.config.register_globally {
                        let store = load_command_sync_store(&state.persistence).await;
                        let (fingerprint, _) =
                            dynamo_app::application_command_fingerprint_for_scope(&settings, None);
                        build_command_sync_panel(
                            SyncScopeKind::Global,
                            &fingerprint,
                            Some(&store.global),
                            &state.config,
                            locale,
                        )
                    } else {
                        build_unsupported_sync_panel(t.sync_unsupported_deployment, locale)
                    }
                };
                match active_tab {
                    "modules" => {
                        render_entity_section(EntityKind::Module, scope, &modules, None, locale)
                    }
                    "commands" => {
                        let panel = sync_panel.await;
                        render_entity_section(
                            EntityKind::Command,
                            scope,
                            &commands,
                            Some(&panel),
                            locale,
                        )
                    }
                    _ => {
                        let panel = sync_panel.await;
                        let all_guilds = state
                            .persistence
                            .list_all_guild_settings()
                            .await
                            .unwrap_or_default();
                        let announce_ready = all_guilds
                            .iter()
                            .filter(|g| g.announcement_channel_id.is_some())
                            .count();
                        let recent_query = DashboardAuditLogQuery {
                            page: 1,
                            page_size: 5,
                            ..DashboardAuditLogQuery::all()
                        };
                        let recent = match state
                            .persistence
                            .list_dashboard_audit_logs(recent_query)
                            .await
                        {
                            Ok(page) => page,
                            Err(error) => {
                                warn!(
                                    ?error,
                                    "failed to load recent audit logs for owner overview"
                                );
                                DashboardAuditLogPage::empty(1, 5)
                            }
                        };
                        render_overview(
                            &OverviewData {
                                scope,
                                modules: &modules,
                                commands: &commands,
                                sync_panel: state.config.register_globally.then_some(&panel),
                                recent: &recent.entries,
                                settings_notice: None,
                                runtime_notices: render_runtime_notices(
                                    &state.module_catalog,
                                    &settings.module_notices,
                                    locale,
                                ),
                                extra_panel: Some(render_owner_overview_extras(
                                    announce_ready,
                                    settings.blocked_guild_ids.len(),
                                    locale,
                                )),
                                entity_names: &names,
                            },
                            locale,
                        )
                    }
                }
            }
        }
    };

    let content = render_scope_page(
        scope,
        render_owner_scope_header(&session.csrf_token, locale),
        render_owner_section_tabs(locale, active_tab),
        section,
    );
    let ctx = PageContext::new(
        &state,
        Some(&session),
        locale,
        t.page_deployment_title,
        format!("/deployment{}", page_query_for_tab(active_tab)),
    )
    .nav(NavItem::Deployment)
    .crumbs(scope_crumbs(
        locale,
        Crumb::link(t.nav_home, "/"),
        t.page_deployment_title,
        "/deployment",
        active_tab,
    ));
    Html(render_document(&ctx, content)).into_response()
}

pub(crate) async fn guild_page(
    jar: CookieJar,
    locale: Locale,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
    Query(query): Query<DashboardPageQuery>,
) -> Response {
    let t = locale.t();
    let Some(existing_session) = load_session(&state, &jar).await else {
        return Redirect::to(&format!("/login?redirect=%2Fguild%2F{guild_id}")).into_response();
    };
    let session = match refresh_read_session(&state, &jar).await {
        Ok(session) => session,
        Err(ReadGuildAuthorizationError::LoginRequired) => {
            return Redirect::to(&format!("/login?redirect=%2Fguild%2F{guild_id}")).into_response();
        }
        Err(ReadGuildAuthorizationError::Unavailable) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &state,
                Some(&existing_session),
                locale,
                t.error_guild_access_unavailable_title,
                t.error_guild_access_unavailable_body,
            );
        }
    };
    let forbidden = || {
        error_response(
            StatusCode::FORBIDDEN,
            &state,
            Some(&session),
            locale,
            t.error_guild_forbidden_title,
            t.error_guild_forbidden_body,
        )
    };
    if !session_can_manage_guild(&session, guild_id) {
        return forbidden();
    }
    let Some(card) = load_guild_card(&state, &session, guild_id).await else {
        return forbidden();
    };
    match card.bot_presence {
        BotGuildPresence::Present => {}
        BotGuildPresence::Missing => {
            return Html(render_install_required_page(
                &state, &session, locale, &card,
            ))
            .into_response();
        }
        BotGuildPresence::Unavailable => {
            return error_response(
                StatusCode::OK,
                &state,
                Some(&session),
                locale,
                t.error_bot_status_title,
                t.error_bot_status_body,
            );
        }
    }

    let scope = Scope::Guild(guild_id);
    let base_path = scope.base_path();
    let active_tab = normalized_tab(query.tab.as_deref());
    let names = entity_name_lookup(&state, locale);
    let section = if active_tab == "logs" {
        let log_entity = parse_audit_entity_filter(query.log_entity.as_deref());
        let log_action = parse_audit_action_filter(query.log_action.as_deref());
        let log_page = query.log_page.unwrap_or(1).max(1);
        let logs_page = load_audit_page(&state, scope, log_entity, log_action, log_page, 20).await;
        render_audit_logs_section(
            &base_path, &logs_page, log_entity, log_action, locale, &names,
        )
    } else {
        let deployment = match state.persistence.deployment_settings_or_default().await {
            Ok(settings) => settings,
            Err(_) => {
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &state,
                    Some(&session),
                    locale,
                    t.error_deployment_unavailable_title,
                    t.error_effective_unavailable_body,
                );
            }
        };
        let settings_unavailable = || {
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &state,
                Some(&session),
                locale,
                t.error_guild_settings_unavailable_title,
                t.error_guild_settings_unavailable_body,
            )
        };
        if state.persistence.guild_settings.is_none() {
            return settings_unavailable();
        }
        let (settings, settings_persisted) = match state.persistence.guild_settings(guild_id).await
        {
            Ok(Some(settings)) => (settings, true),
            Ok(None) => (GuildSettings::for_guild(guild_id), false),
            Err(error) => {
                warn!(?error, guild_id, "failed to load guild settings page");
                return settings_unavailable();
            }
        };
        let modules = module_rows(
            &state.module_catalog,
            &resolve_module_states(&state.module_catalog, &deployment, Some(&settings)),
            scope,
            locale,
        );
        let commands = command_rows(
            &state.command_catalog,
            &resolve_command_states(
                &state.module_catalog,
                &state.command_catalog,
                &deployment,
                Some(&settings),
            ),
            scope,
            locale,
        );
        let sync_panel = async {
            if state.config.register_globally {
                build_unsupported_sync_panel(t.sync_unsupported_guild, locale)
            } else {
                let store = load_command_sync_store(&state.persistence).await;
                let (fingerprint, _) = dynamo_app::application_command_fingerprint_for_scope(
                    &deployment,
                    Some(&settings),
                );
                build_command_sync_panel(
                    SyncScopeKind::Guild(guild_id),
                    &fingerprint,
                    store.guild(guild_id),
                    &state.config,
                    locale,
                )
            }
        };
        match active_tab {
            "modules" => render_entity_section(EntityKind::Module, scope, &modules, None, locale),
            "commands" => {
                let panel = sync_panel.await;
                render_entity_section(EntityKind::Command, scope, &commands, Some(&panel), locale)
            }
            _ => {
                let panel = sync_panel.await;
                let recent = load_audit_page(&state, scope, None, None, 1, 5).await;
                let state_name = guild_settings_ui_state(&settings, settings_persisted);
                let (testid, notice) = guild_settings_notice(state_name, locale);
                render_overview(
                    &OverviewData {
                        scope,
                        modules: &modules,
                        commands: &commands,
                        sync_panel: (!state.config.register_globally).then_some(&panel),
                        recent: &recent.entries,
                        settings_notice: Some((state_name, testid, notice)),
                        runtime_notices: render_runtime_notices(
                            &state.module_catalog,
                            &deployment.module_notices,
                            locale,
                        ),
                        extra_panel: Some(render_guild_announcement_card(
                            guild_id,
                            settings.announcement_channel_id,
                            locale,
                        )),
                        entity_names: &names,
                    },
                    locale,
                )
            }
        }
    };

    let content = render_scope_page(
        scope,
        render_scope_header(
            &card.name,
            t.page_server_settings,
            Some(avatar(card.icon_url.as_deref(), &card.name, "avatar-lg")),
            Some(("success", t.bot_connected)),
        ),
        render_section_tabs(locale, &base_path, active_tab),
        section,
    );
    let ctx = PageContext::new(
        &state,
        Some(&session),
        locale,
        card.name.clone(),
        format!("{base_path}{}", page_query_for_tab(active_tab)),
    )
    .nav(NavItem::Servers)
    .guild(CurrentGuild {
        id: card.id,
        name: card.name.clone(),
        icon_url: card.icon_url.clone(),
    })
    .crumbs(scope_crumbs(
        locale,
        Crumb::link(t.nav_servers, "/selector"),
        &card.name,
        &base_path,
        active_tab,
    ));
    Html(render_document(&ctx, content)).into_response()
}

fn fragment_error(status: StatusCode, message: &str) -> Response {
    let mut response = (status, Html(html! { p { (message) } }.into_string())).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Finds the row, schema, and stored configuration for one dialog.
fn dialog_parts<'a>(
    state: &'a DashboardState,
    scope: Scope,
    kind: EntityKind,
    item_id: &str,
    deployment: &DeploymentSettings,
    guild: Option<&'a GuildSettings>,
    locale: Locale,
) -> Option<(EntityRow, &'a SettingsSchema, serde_json::Value)> {
    match kind {
        EntityKind::Module => {
            let entry = state
                .module_catalog
                .entries
                .iter()
                .find(|entry| entry.module.id == item_id)?;
            let resolved = resolve_module_states(&state.module_catalog, deployment, guild);
            let row = module_rows(&state.module_catalog, &resolved, scope, locale)
                .into_iter()
                .find(|row| row.id == item_id)?;
            let configuration = guild
                .and_then(|settings| settings.modules.get(item_id))
                .map(|settings| settings.configuration.clone())
                .unwrap_or(serde_json::Value::Null);
            Some((row, &entry.settings, configuration))
        }
        EntityKind::Command => {
            let entry = state.command_catalog.find_by_id(item_id)?;
            let resolved = resolve_command_states(
                &state.module_catalog,
                &state.command_catalog,
                deployment,
                guild,
            );
            let row = command_rows(&state.command_catalog, &resolved, scope, locale)
                .into_iter()
                .find(|row| row.id == item_id)?;
            let configuration = match guild {
                Some(settings) => settings
                    .commands
                    .get(item_id)
                    .map(|settings| settings.configuration.clone()),
                None => deployment
                    .commands
                    .get(item_id)
                    .map(|settings| settings.configuration.clone()),
            }
            .unwrap_or(serde_json::Value::Null);
            Some((row, &entry.settings, configuration))
        }
    }
}

fn deployment_toggles(
    kind: EntityKind,
    row: &EntityRow,
    locale: Locale,
) -> Vec<DialogToggle<'static>> {
    let t = locale.t();
    let installed = row
        .status
        .chain
        .iter()
        .find(|(link, _)| *link == crate::status::ChainLink::Installed)
        .is_some_and(|(_, on)| *on);
    let (installed_help, enabled_help) = match kind {
        EntityKind::Module => (t.dialog_module_installed_help, t.dialog_module_enabled_help),
        EntityKind::Command => (
            t.dialog_command_installed_help,
            t.dialog_command_enabled_help,
        ),
    };
    vec![
        DialogToggle {
            name: "installed",
            label: t.dialog_installed,
            help: installed_help,
            checked: installed,
        },
        DialogToggle {
            name: "enabled",
            label: t.dialog_enabled_default,
            help: enabled_help,
            checked: row.status.own_enabled,
        },
    ]
}

pub(crate) async fn guild_settings_fragment(
    jar: CookieJar,
    locale: Locale,
    State(state): State<Arc<DashboardState>>,
    Path((guild_id, kind, item_id)): Path<(u64, String, String)>,
) -> Response {
    let t = locale.t();
    let Some(session) = load_session(&state, &jar).await else {
        return fragment_error(StatusCode::UNAUTHORIZED, locale.client().http_401);
    };
    if !session_can_manage_guild(&session, guild_id) {
        return fragment_error(StatusCode::FORBIDDEN, t.error_guild_forbidden_body);
    }
    let Some(kind) = EntityKind::from_key(&kind) else {
        return fragment_error(StatusCode::NOT_FOUND, t.error_not_found_body);
    };
    let Ok(deployment) = state.persistence.deployment_settings_or_default().await else {
        return fragment_error(
            StatusCode::SERVICE_UNAVAILABLE,
            t.error_deployment_unavailable_body,
        );
    };
    let settings = match state.persistence.guild_settings(guild_id).await {
        Ok(settings) => settings.unwrap_or_else(|| GuildSettings::for_guild(guild_id)),
        Err(error) => {
            warn!(?error, guild_id, "failed to load guild settings dialog");
            return fragment_error(
                StatusCode::SERVICE_UNAVAILABLE,
                t.error_guild_settings_unavailable_body,
            );
        }
    };
    let scope = Scope::Guild(guild_id);
    let Some((row, schema, configuration)) = dialog_parts(
        &state,
        scope,
        kind,
        &item_id,
        &deployment,
        Some(&settings),
        locale,
    ) else {
        return fragment_error(StatusCode::NOT_FOUND, t.error_not_found_body);
    };
    let spec = DialogSpec {
        scope,
        row: &row,
        toggles: vec![DialogToggle {
            name: "enabled",
            label: t.dialog_guild_enabled,
            help: t.dialog_guild_enabled_help,
            checked: row.status.own_enabled,
        }],
        schema,
        configuration: &configuration,
        patch_url: patch_url(scope, kind, &row.id),
        reconcile_url: format!("/api/guild-settings/{guild_id}"),
        sends_configuration: true,
    };
    Html(render_settings_dialog(&spec, locale).into_string()).into_response()
}

pub(crate) async fn deployment_settings_fragment(
    jar: CookieJar,
    locale: Locale,
    State(state): State<Arc<DashboardState>>,
    Path((kind, item_id)): Path<(String, String)>,
) -> Response {
    let t = locale.t();
    let Some(session) = load_session(&state, &jar).await else {
        return fragment_error(StatusCode::UNAUTHORIZED, locale.client().http_401);
    };
    if !is_application_owner(&state, &session) {
        return fragment_error(StatusCode::FORBIDDEN, t.error_admin_only_body);
    }
    if state.config.owner_totp_secret.is_none() {
        return fragment_error(StatusCode::FORBIDDEN, t.owner_setup_lede);
    }
    match load_valid_elevation(&state, &jar, &session).await {
        Ok(Some(_)) => {}
        Ok(None) => return fragment_error(StatusCode::FORBIDDEN, t.owner_unlock_title),
        Err(StatusCode::UNAUTHORIZED) => {
            return fragment_error(StatusCode::UNAUTHORIZED, locale.client().http_401);
        }
        Err(StatusCode::FORBIDDEN) => {
            return fragment_error(StatusCode::FORBIDDEN, t.error_admin_only_body);
        }
        Err(_) => {
            return fragment_error(
                StatusCode::SERVICE_UNAVAILABLE,
                t.error_deployment_unavailable_body,
            );
        }
    }
    let Some(kind) = EntityKind::from_key(&kind) else {
        return fragment_error(StatusCode::NOT_FOUND, t.error_not_found_body);
    };
    let Ok(deployment) = state.persistence.deployment_settings_or_default().await else {
        return fragment_error(
            StatusCode::SERVICE_UNAVAILABLE,
            t.error_deployment_unavailable_body,
        );
    };
    let scope = Scope::Deployment;
    let Some((row, schema, configuration)) =
        dialog_parts(&state, scope, kind, &item_id, &deployment, None, locale)
    else {
        return fragment_error(StatusCode::NOT_FOUND, t.error_not_found_body);
    };
    let spec = DialogSpec {
        scope,
        row: &row,
        toggles: deployment_toggles(kind, &row, locale),
        schema,
        configuration: &configuration,
        patch_url: patch_url(scope, kind, &row.id),
        reconcile_url: "/api/deployment-settings".to_string(),
        sends_configuration: kind == EntityKind::Command,
    };
    Html(render_settings_dialog(&spec, locale).into_string()).into_response()
}

pub(crate) fn normalized_tab(value: Option<&str>) -> &'static str {
    match value {
        Some("modules") => "modules",
        Some("commands") => "commands",
        Some("logs") => "logs",
        _ => "overview",
    }
}

pub(crate) fn normalized_owner_tab(value: Option<&str>) -> &'static str {
    match value {
        Some("servers") => "servers",
        Some("system") => "system",
        Some("notices") => "notices",
        Some("modules") => "modules",
        Some("commands") => "commands",
        Some("logs") => "logs",
        _ => "overview",
    }
}

pub(crate) fn parse_audit_scope_filter(value: Option<&str>) -> Option<DashboardAuditScope> {
    match value {
        Some("deployment") => Some(DashboardAuditScope::Deployment),
        Some("guild") => Some(DashboardAuditScope::Guild),
        _ => None,
    }
}

pub(crate) fn parse_audit_entity_filter(value: Option<&str>) -> Option<DashboardAuditEntityType> {
    match value {
        Some("module") => Some(DashboardAuditEntityType::Module),
        Some("command") => Some(DashboardAuditEntityType::Command),
        _ => None,
    }
}

pub(crate) fn parse_owner_audit_entity_filter(
    value: Option<&str>,
) -> Option<DashboardAuditEntityType> {
    match value {
        Some("module") => Some(DashboardAuditEntityType::Module),
        Some("command") => Some(DashboardAuditEntityType::Command),
        Some("guild") => Some(DashboardAuditEntityType::Guild),
        Some("operations") => Some(DashboardAuditEntityType::Operations),
        _ => None,
    }
}

pub(crate) fn parse_audit_action_filter(value: Option<&str>) -> Option<DashboardAuditAction> {
    match value {
        Some("toggle") => Some(DashboardAuditAction::Toggle),
        Some("save_settings") => Some(DashboardAuditAction::SaveSettings),
        _ => None,
    }
}

pub(crate) fn parse_owner_audit_action_filter(value: Option<&str>) -> Option<DashboardAuditAction> {
    match value {
        Some("toggle") => Some(DashboardAuditAction::Toggle),
        Some("save_settings") => Some(DashboardAuditAction::SaveSettings),
        Some("sync") => Some(DashboardAuditAction::Sync),
        Some("reset") => Some(DashboardAuditAction::Reset),
        Some("leave_guild") => Some(DashboardAuditAction::LeaveGuild),
        Some("block_guild") => Some(DashboardAuditAction::BlockGuild),
        Some("unblock_guild") => Some(DashboardAuditAction::UnblockGuild),
        Some("send_announcement") => Some(DashboardAuditAction::SendAnnouncement),
        _ => None,
    }
}

pub(crate) fn page_query_for_tab(tab: &str) -> String {
    format!("?tab={tab}")
}

pub(crate) fn page_query_for_logs(
    entity_type: Option<DashboardAuditEntityType>,
    action: Option<DashboardAuditAction>,
    page: u64,
) -> String {
    let mut params = vec!["tab=logs".to_string()];
    if let Some(entity_type) = entity_type {
        params.push(format!("log_entity={}", entity_type.as_str()));
    }
    if let Some(action) = action {
        params.push(format!("log_action={}", action.as_str()));
    }
    if page > 1 {
        params.push(format!("log_page={page}"));
    }
    format!("?{}", params.join("&"))
}

pub(crate) fn page_query_for_owner_logs(
    scope: Option<DashboardAuditScope>,
    entity_type: Option<DashboardAuditEntityType>,
    action: Option<DashboardAuditAction>,
    page: u64,
) -> String {
    let mut params = vec!["tab=logs".to_string()];
    if let Some(scope) = scope {
        params.push(format!("log_scope={}", scope.as_str()));
    }
    if let Some(entity_type) = entity_type {
        params.push(format!("log_entity={}", entity_type.as_str()));
    }
    if let Some(action) = action {
        params.push(format!("log_action={}", action.as_str()));
    }
    if page > 1 {
        params.push(format!("log_page={page}"));
    }
    format!("?{}", params.join("&"))
}
