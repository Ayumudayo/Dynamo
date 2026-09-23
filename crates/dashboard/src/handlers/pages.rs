//! Browser-rendered dashboard routes, page composition, and query parsing.

use crate::*;
pub(crate) async fn index(jar: CookieJar, State(state): State<Arc<DashboardState>>) -> Response {
    let session = load_session(&state, &jar).await;
    Html(render_landing_page(&state, session.as_ref())).into_response()
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
    State(state): State<Arc<DashboardState>>,
    axum::extract::Query(query): axum::extract::Query<DiscordCallbackQuery>,
) -> Response {
    if let Some(error) = query.error {
        return Html(render_error_page(
            &state,
            None,
            "Discord Login Failed",
            &format!("Discord returned an OAuth error: {}.", escape_html(&error)),
        ))
        .into_response();
    }

    let Some(code) = query.code.as_deref() else {
        return Html(render_error_page(
            &state,
            None,
            "Discord Login Failed",
            "Discord did not return an authorization code.",
        ))
        .into_response();
    };

    let Some(oauth_state) = query.state.as_deref() else {
        return Html(render_error_page(
            &state,
            None,
            "Discord Login Failed",
            "Missing OAuth state. Please try signing in again.",
        ))
        .into_response();
    };

    let pending = {
        let mut states = state.oauth_states.write().await;
        states.retain(|_, value| !is_oauth_state_expired(value));
        states.remove(oauth_state)
    };

    let Some(pending) = pending else {
        return Html(render_error_page(
            &state,
            None,
            "Discord Login Failed",
            "The login session expired or was already used. Please try again.",
        ))
        .into_response();
    };

    match exchange_oauth_code(&state, code).await {
        Ok(session) => {
            let session_id = random_token(64);
            {
                let mut sessions = state.sessions.write().await;
                sessions.retain(|_, value| !is_session_expired(value));
                sessions.insert(session_id.clone(), session);
            }

            let jar = jar.add(session_cookie(&session_id));
            (jar, Redirect::to(&pending.redirect_to)).into_response()
        }
        Err(error) => {
            warn!(?error, "failed to complete Discord OAuth callback");
            Html(render_error_page(
                &state,
                None,
                "Discord Login Failed",
                "Could not exchange the Discord OAuth code or load your guild list.",
            ))
            .into_response()
        }
    }
}

pub(crate) async fn logout(jar: CookieJar, State(state): State<Arc<DashboardState>>) -> Response {
    if let Some(cookie) = jar.get(SESSION_COOKIE_NAME) {
        state.sessions.write().await.remove(cookie.value());
    }

    let jar = jar.remove(Cookie::from(SESSION_COOKIE_NAME));
    (jar, Redirect::to("/")).into_response()
}

pub(crate) async fn selector(jar: CookieJar, State(state): State<Arc<DashboardState>>) -> Response {
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
    let response = Html(render_selector_page(&state, &session, &guild_cards)).into_response();
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

pub(crate) async fn deployment_page(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<DashboardPageQuery>,
) -> Response {
    let Some(session) = load_session(&state, &jar).await else {
        return Redirect::to("/login?redirect=%2Fdeployment").into_response();
    };
    if !user_is_dashboard_admin(&state, &session.user) {
        return (
            StatusCode::FORBIDDEN,
            Html(render_error_page(
                &state,
                Some(&session),
                "Dashboard Access Restricted",
                "Deployment-wide settings are reserved for the bot owner or configured dashboard administrators.",
            )),
        )
            .into_response();
    }

    let active_tab = normalized_tab(query.tab.as_deref());
    let log_entity = parse_audit_entity_filter(query.log_entity.as_deref());
    let log_action = parse_audit_action_filter(query.log_action.as_deref());
    let log_page = query.log_page.unwrap_or(1).max(1);
    let (overview, active_section, modals, include_mutation_script) = match active_tab {
        "logs" => {
            let logs_page = match state
                .persistence
                .list_dashboard_audit_logs(DashboardAuditLogQuery {
                    scope: DashboardAuditScope::Deployment,
                    guild_id: None,
                    entity_type: log_entity,
                    action: log_action,
                    page: log_page,
                    page_size: 20,
                })
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    warn!(?error, "failed to load deployment dashboard audit logs");
                    DashboardAuditLogPage::empty(log_page, 20)
                }
            };
            (
                String::new(),
                render_audit_logs_section("/deployment", &logs_page, log_entity, log_action),
                String::new(),
                false,
            )
        }
        tab => {
            let settings = match state.persistence.deployment_settings_or_default().await {
                Ok(settings) => settings,
                Err(_) => {
                    warn!("failed to load deployment settings page");
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Html(render_error_page(
                            &state,
                            Some(&session),
                            "Deployment Settings Unavailable",
                            "Deployment settings could not be loaded. Please try again.",
                        )),
                    )
                        .into_response();
                }
            };
            match tab {
                "modules" => {
                    let resolved_states =
                        resolve_module_states(&state.module_catalog, &settings, None);
                    let modals = state
                        .module_catalog
                        .entries
                        .iter()
                        .zip(&resolved_states)
                        .map(|(entry, resolved)| {
                            let current = settings.modules.get(entry.module.id).cloned().unwrap_or(
                                DeploymentModuleSettings {
                                    installed: true,
                                    enabled: entry.module.enabled_by_default,
                                },
                            );
                            render_deployment_module_modal(
                                entry,
                                resolved,
                                &render_module_runtime_notice(entry.module.id),
                                &current,
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let cards = render_module_summary_cards(
                        "deployment",
                        &state.module_catalog,
                        &settings,
                        None,
                        &resolved_states,
                    );
                    (
                        String::new(),
                        format!(
                            "<section id=\"modules\" class=\"section-block\" data-testid=\"deployment-modules-section\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Modules</p><h2>Deployment Modules</h2></div><input id=\"module-filter\" class=\"toolbar-search compact-search\" type=\"search\" aria-label=\"Search deployment modules\" aria-describedby=\"module-filter-status module-filter-empty\" placeholder=\"Search modules\" oninput=\"filterModuleCards(this.value)\" /></div><p id=\"module-filter-status\" class=\"filter-feedback\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\"></p><p id=\"module-filter-empty\" class=\"filter-empty\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\" hidden>No modules match this search.</p><div class=\"module-grid compact-grid\">{cards}</div></section>"
                        ),
                        modals,
                        true,
                    )
                }
                "commands" => {
                    let resolved_states = resolve_command_states(
                        &state.module_catalog,
                        &state.command_catalog,
                        &settings,
                        None,
                    );
                    let cards = render_command_summary_cards(
                        "deployment",
                        &state.command_catalog,
                        &settings,
                        None,
                        &resolved_states,
                    );
                    let sync_panel = if state.config.register_globally {
                        let store = load_command_sync_store(&state.persistence).await;
                        let (fingerprint, _) =
                            dynamo_app::application_command_fingerprint_for_scope(&settings, None);
                        render_command_sync_panel(&build_command_sync_panel(
                            SyncScopeKind::Global,
                            &fingerprint,
                            Some(&store.global),
                            &state.config,
                        ))
                    } else {
                        render_command_sync_panel(&build_unsupported_sync_panel(
                            "Deployment command sync is disabled in guild-scoped mode. Open a guild page and run Sync Commands there.",
                        ))
                    };
                    let modals = render_deployment_command_modals(
                        &state.command_catalog,
                        &settings,
                        &resolved_states,
                    );
                    (
                        String::new(),
                        format!(
                            "<section id=\"commands\" class=\"section-block\" data-testid=\"deployment-commands-section\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Commands</p><h2>Deployment Commands</h2></div><input id=\"command-filter\" class=\"toolbar-search compact-search\" type=\"search\" aria-label=\"Search deployment commands\" aria-describedby=\"command-filter-status command-filter-empty\" placeholder=\"Search commands\" oninput=\"filterCommandCards(this.value)\" /></div><p id=\"command-filter-status\" class=\"filter-feedback\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\"></p><p id=\"command-filter-empty\" class=\"filter-empty\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\" hidden>No commands match this search and category.</p>{sync_panel}{tabs}<div class=\"module-grid command-grid compact-grid\" data-testid=\"command-card-grid\">{cards}</div></section>",
                            tabs = render_command_category_tabs(&state.command_catalog)
                        ),
                        modals,
                        true,
                    )
                }
                _ => {
                    let modules = resolve_module_states(&state.module_catalog, &settings, None);
                    let commands = resolve_command_states(
                        &state.module_catalog,
                        &state.command_catalog,
                        &settings,
                        None,
                    );
                    let overview = render_overview_section(
                        "Deployment Control",
                        "Global install state and command availability across every guild.",
                        &[
                            (
                                "Modules Enabled",
                                count_enabled_modules(&modules).to_string(),
                            ),
                            (
                                "Commands Enabled",
                                count_enabled_commands(&commands).to_string(),
                            ),
                            (
                                "Runtime Notes",
                                count_runtime_notices(&state.module_catalog).to_string(),
                            ),
                        ],
                    );
                    let panel = format!(
                        "<section class=\"section-block\" data-testid=\"deployment-overview-section\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Overview</p><h2>Deployment Summary</h2></div></div><div class=\"grid two compact-grid-two\"><article class=\"panel info-panel compact-info-panel\"><h3>Scope</h3><p>Deployment settings define the default module installation and command availability used across every guild.</p></article><article class=\"panel info-panel compact-info-panel\"><h3>Runtime Notes</h3>{}</article></div></section>",
                        render_runtime_notices(&state.module_catalog)
                    );
                    (overview, panel, String::new(), false)
                }
            }
        }
    };
    let script = if include_mutation_script {
        format!("<script>{}</script>", dashboard_script())
    } else {
        String::new()
    };
    let content = format!(
        "{}{modals}{script}",
        render_dashboard_page_shell(
            &overview,
            &render_section_tabs("/deployment", active_tab),
            &active_section,
            active_tab
        )
    );

    Html(render_document(
        &state,
        Some(&session),
        "Deployment Settings",
        "Global module installation, enablement, and command controls.",
        Some("/deployment"),
        Some(active_tab),
        &content,
    ))
    .into_response()
}

pub(crate) async fn guild_page(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
    Query(query): Query<DashboardPageQuery>,
) -> Response {
    let Some(existing_session) = load_session(&state, &jar).await else {
        return Redirect::to(&format!("/login?redirect=%2Fguild%2F{guild_id}")).into_response();
    };
    let session = match refresh_read_session(&state, &jar).await {
        Ok(session) => session,
        Err(ReadGuildAuthorizationError::LoginRequired) => {
            return Redirect::to(&format!("/login?redirect=%2Fguild%2F{guild_id}")).into_response();
        }
        Err(ReadGuildAuthorizationError::Unavailable) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Html(render_error_page(
                    &state,
                    Some(&existing_session),
                    "Guild Access Unavailable",
                    "Discord could not verify your current server access. Please try again.",
                )),
            )
                .into_response();
        }
    };
    if !session_can_manage_guild(&session, guild_id) {
        return (
            StatusCode::FORBIDDEN,
            Html(render_error_page(
                &state,
                Some(&session),
                "Guild Access Restricted",
                "You do not have dashboard access to that server.",
            )),
        )
            .into_response();
    }
    let Some(card) = load_guild_card(&state, &session, guild_id).await else {
        return (
            StatusCode::FORBIDDEN,
            Html(render_error_page(
                &state,
                Some(&session),
                "Guild Access Restricted",
                "You do not have dashboard access to that server.",
            )),
        )
            .into_response();
    };
    match card.bot_presence {
        BotGuildPresence::Present => {}
        BotGuildPresence::Missing => {
            return Html(render_install_required_page(&state, &session, &card)).into_response();
        }
        BotGuildPresence::Unavailable => {
            return Html(render_error_page(
                &state,
                Some(&session),
                "Bot Status Unavailable",
                "Discord did not return the bot's current server status. Please try again later.",
            ))
            .into_response();
        }
    }

    let active_tab = normalized_tab(query.tab.as_deref());
    let log_entity = parse_audit_entity_filter(query.log_entity.as_deref());
    let log_action = parse_audit_action_filter(query.log_action.as_deref());
    let log_page = query.log_page.unwrap_or(1).max(1);
    let (overview, active_section, modals, include_mutation_script) = if active_tab == "logs" {
        let logs_page = match state
            .persistence
            .list_dashboard_audit_logs(DashboardAuditLogQuery {
                scope: DashboardAuditScope::Guild,
                guild_id: Some(guild_id),
                entity_type: log_entity,
                action: log_action,
                page: log_page,
                page_size: 20,
            })
            .await
        {
            Ok(page) => page,
            Err(error) => {
                warn!(
                    guild_id,
                    ?error,
                    "failed to load guild dashboard audit logs"
                );
                DashboardAuditLogPage::empty(log_page, 20)
            }
        };
        (
            String::new(),
            render_audit_logs_section(
                &format!("/guild/{guild_id}"),
                &logs_page,
                log_entity,
                log_action,
            ),
            String::new(),
            false,
        )
    } else {
        let deployment = match state.persistence.deployment_settings_or_default().await {
            Ok(settings) => settings,
            Err(_) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Html(render_error_page(
                        &state,
                        Some(&session),
                        "Deployment Settings Unavailable",
                        "The effective guild state could not be determined. Please try again.",
                    )),
                )
                    .into_response();
            }
        };
        let Some(_) = state.persistence.guild_settings.as_ref() else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Html(render_error_page(
                    &state,
                    Some(&session),
                    "Guild Settings Unavailable",
                    "Guild settings could not be loaded. Please try again.",
                )),
            )
                .into_response();
        };
        let (settings, settings_persisted) = match state.persistence.guild_settings(guild_id).await
        {
            Ok(Some(settings)) => (settings, true),
            Ok(None) => (GuildSettings::for_guild(guild_id), false),
            Err(error) => {
                warn!(?error, guild_id, "failed to load guild settings page");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Html(render_error_page(
                        &state,
                        Some(&session),
                        "Guild Settings Unavailable",
                        "Guild settings could not be loaded. Please try again.",
                    )),
                )
                    .into_response();
            }
        };
        match active_tab {
            "modules" => {
                let states =
                    resolve_module_states(&state.module_catalog, &deployment, Some(&settings));
                let modals = state
                    .module_catalog
                    .entries
                    .iter()
                    .zip(&states)
                    .map(|(entry, resolved)| {
                        let current = settings
                            .modules
                            .get(entry.module.id)
                            .cloned()
                            .unwrap_or_default();
                        let fields = render_structured_fields(entry, &current.configuration);
                        render_guild_module_modal(
                            guild_id,
                            entry,
                            resolved,
                            &render_module_runtime_notice(entry.module.id),
                            &current,
                            &fields,
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let cards = render_module_summary_cards(
                    "guild",
                    &state.module_catalog,
                    &deployment,
                    Some(&settings),
                    &states,
                );
                (
                    String::new(),
                    format!(
                        "<section id=\"modules\" class=\"section-block\" data-testid=\"guild-modules-section\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Modules</p><h2>Guild Modules</h2></div><input id=\"module-filter\" data-testid=\"module-filter\" class=\"toolbar-search compact-search\" type=\"search\" aria-label=\"Search guild modules\" aria-describedby=\"module-filter-status module-filter-empty\" placeholder=\"Search modules\" oninput=\"filterModuleCards(this.value)\" /></div><p id=\"module-filter-status\" class=\"filter-feedback\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\"></p><p id=\"module-filter-empty\" class=\"filter-empty\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\" hidden>No modules match this search.</p><div class=\"module-grid compact-grid compact-module-grid\">{cards}</div></section>"
                    ),
                    modals,
                    true,
                )
            }
            "commands" => {
                let states = resolve_command_states(
                    &state.module_catalog,
                    &state.command_catalog,
                    &deployment,
                    Some(&settings),
                );
                let cards = render_command_summary_cards(
                    "guild",
                    &state.command_catalog,
                    &deployment,
                    Some(&settings),
                    &states,
                );
                let sync_panel = if state.config.register_globally {
                    render_command_sync_panel(&build_unsupported_sync_panel(
                        "This bot is using global command registration. Run Sync Global Commands from the deployment page to refresh Discord.",
                    ))
                } else {
                    let store = load_command_sync_store(&state.persistence).await;
                    let (fingerprint, _) = dynamo_app::application_command_fingerprint_for_scope(
                        &deployment,
                        Some(&settings),
                    );
                    render_command_sync_panel(&build_command_sync_panel(
                        SyncScopeKind::Guild(guild_id),
                        &fingerprint,
                        store.guild(guild_id),
                        &state.config,
                    ))
                };
                let modals = render_guild_command_modals(
                    guild_id,
                    &state.command_catalog,
                    &settings,
                    &states,
                );
                (
                    String::new(),
                    format!(
                        "<section id=\"commands\" class=\"section-block\" data-testid=\"guild-commands-section\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Commands</p><h2>Guild Commands</h2></div><input id=\"command-filter\" data-testid=\"command-filter\" class=\"toolbar-search compact-search\" type=\"search\" aria-label=\"Search guild commands\" aria-describedby=\"command-filter-status command-filter-empty\" placeholder=\"Search commands\" oninput=\"filterCommandCards(this.value)\" /></div><p id=\"command-filter-status\" class=\"filter-feedback\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\"></p><p id=\"command-filter-empty\" class=\"filter-empty\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\" hidden>No commands match this search and category.</p>{sync_panel}{tabs}<div class=\"module-grid command-grid compact-grid compact-command-grid\" data-testid=\"command-card-grid\">{cards}</div></section>",
                        tabs = render_command_category_tabs(&state.command_catalog)
                    ),
                    modals,
                    true,
                )
            }
            _ => {
                let modules =
                    resolve_module_states(&state.module_catalog, &deployment, Some(&settings));
                let commands = resolve_command_states(
                    &state.module_catalog,
                    &state.command_catalog,
                    &deployment,
                    Some(&settings),
                );
                let overview = render_overview_section(
                    &card.name,
                    "Guild-scoped module and command controls for this server.",
                    &[
                        (
                            "Modules Enabled",
                            count_enabled_modules(&modules).to_string(),
                        ),
                        (
                            "Commands Enabled",
                            count_enabled_commands(&commands).to_string(),
                        ),
                        ("Guild ID", guild_id.to_string()),
                    ],
                );
                let state_name = guild_settings_ui_state(&settings, settings_persisted);
                let panel = format!(
                    "<section id=\"overview\" class=\"panel section-block\" data-testid=\"guild-runtime-summary\" data-settings-state=\"{state_name}\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Overview</p><h2>Guild Summary</h2></div><span class=\"pill pill-success\">Bot Connected</span></div>{}<div class=\"grid two compact-grid-two\"><article class=\"panel info-panel compact-info-panel\"><h3>Server Info</h3><p>Guild ID <code>{guild_id}</code></p><p>Guild-specific settings override deployment defaults where enabled.</p></article><article class=\"panel info-panel compact-info-panel\"><h3>Runtime Notes</h3>{}</article></div></section>",
                    guild_settings_notice(state_name),
                    render_runtime_notices(&state.module_catalog)
                );
                (overview, panel, String::new(), false)
            }
        }
    };
    let script = if include_mutation_script {
        format!("<script>{}</script>", dashboard_script())
    } else {
        String::new()
    };
    let content = format!(
        "{}{modals}{script}",
        render_dashboard_page_shell(
            &overview,
            &render_section_tabs(&format!("/guild/{guild_id}"), active_tab),
            &active_section,
            active_tab
        )
    );

    Html(render_document(
        &state,
        Some(&session),
        &format!("Guild Settings: {}", card.name),
        "Guild-scoped module and command controls for this server.",
        Some(&format!("/guild/{guild_id}")),
        Some(active_tab),
        &content,
    ))
    .into_response()
}

pub(crate) fn normalized_tab(value: Option<&str>) -> &'static str {
    match value {
        Some("modules") => "modules",
        Some("commands") => "commands",
        Some("logs") => "logs",
        _ => "overview",
    }
}

pub(crate) fn parse_audit_entity_filter(value: Option<&str>) -> Option<DashboardAuditEntityType> {
    match value {
        Some("module") => Some(DashboardAuditEntityType::Module),
        Some("command") => Some(DashboardAuditEntityType::Command),
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
