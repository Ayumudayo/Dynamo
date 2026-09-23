use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::Value;
use tokio::sync::oneshot;
use tower::ServiceExt;

use super::*;

fn unique_temp_directory(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    env::temp_dir().join(format!(
        "dynamo-dashboard-perf-{label}-{}-{unique}",
        process::id()
    ))
}

fn valid_environment(ready_file: &Path) -> HashMap<String, String> {
    HashMap::from([
        (ENV_REVISION.to_string(), "a".repeat(40)),
        (ENV_NONCE.to_string(), "b".repeat(64)),
        (ENV_FIXTURE_MODE.to_string(), "GuildDetail".to_string()),
        (
            ENV_FIXTURE_VERSION.to_string(),
            "guild-detail-v1".to_string(),
        ),
        (ENV_FIXTURE_SHA256.to_string(), fixture_bytes_sha256()),
        (
            ENV_READY_FILE.to_string(),
            ready_file.to_string_lossy().into_owned(),
        ),
    ])
}

fn parse_config(values: &HashMap<String, String>) -> anyhow::Result<PerfHarnessConfig> {
    PerfHarnessConfig::from_lookup(|key| values.get(key).cloned())
}

fn fixture_app() -> (
    Router,
    Arc<PerfRuntime>,
    oneshot::Receiver<()>,
    PerfHarnessConfig,
) {
    fixture_app_for_mode(FixtureMode::ReadOnly)
}

fn fixture_app_for_mode(
    fixture_mode: FixtureMode,
) -> (
    Router,
    Arc<PerfRuntime>,
    oneshot::Receiver<()>,
    PerfHarnessConfig,
) {
    let config = PerfHarnessConfig {
        revision: "a".repeat(40),
        nonce: "b".repeat(64),
        fixture_mode,
        fixture: FixtureIdentity {
            version: "guild-detail-v1".to_string(),
            sha256: fixture_bytes_sha256(),
        },
        ready_file: env::temp_dir().join("unused-ready-file.json"),
    };
    let fixture = FixtureData::load(&config).expect("valid compiled fixture");
    let (sender, receiver) = oneshot::channel();
    let runtime =
        Arc::new(PerfRuntime::new(&config, &fixture, sender).expect("performance runtime"));
    let state =
        build_fixture_state(&fixture, runtime.clone(), 45678).expect("fixture dashboard state");
    (
        build_perf_router(state, runtime.clone()),
        runtime,
        receiver,
        config,
    )
}

fn request(method: &str, uri: &str, cookie: Option<(&str, &str)>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some((name, value)) = cookie {
        builder = builder.header(header::COOKIE, format!("{name}={value}"));
    }
    builder.body(Body::empty()).expect("valid request")
}

async fn json_body(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("bounded response body");
    serde_json::from_slice(&bytes).expect("JSON response")
}

#[test]
fn config_rejects_missing_malformed_and_caller_network_values() {
    let directory = unique_temp_directory("config-negative");
    fs::create_dir(&directory).expect("create temp directory");
    let ready_file = directory.join("ready.json");
    let valid = valid_environment(&ready_file);
    assert!(parse_config(&valid).is_ok());

    for key in [
        ENV_REVISION,
        ENV_NONCE,
        ENV_FIXTURE_MODE,
        ENV_FIXTURE_VERSION,
        ENV_FIXTURE_SHA256,
        ENV_READY_FILE,
    ] {
        let mut values = valid.clone();
        values.remove(key);
        assert!(parse_config(&values).is_err(), "missing {key} was accepted");
    }

    let invalid_values = [
        (ENV_REVISION, "A".repeat(40)),
        (ENV_NONCE, "g".repeat(64)),
        (ENV_FIXTURE_MODE, "Production".to_string()),
        (ENV_FIXTURE_VERSION, "Unsafe Version".to_string()),
        (ENV_FIXTURE_SHA256, "0".repeat(64)),
        (ENV_READY_FILE, "relative-ready.json".to_string()),
    ];
    for (key, value) in invalid_values {
        let mut values = valid.clone();
        values.insert(key.to_string(), value);
        assert!(parse_config(&values).is_err(), "invalid {key} was accepted");
    }

    for key in FORBIDDEN_ENVIRONMENT {
        let mut values = valid.clone();
        values.insert((*key).to_string(), "forbidden".to_string());
        assert!(
            parse_config(&values).is_err(),
            "forbidden {key} was accepted"
        );
    }
    fs::remove_dir_all(directory).expect("remove temp directory");
}

#[test]
fn compiled_revision_provenance_is_required_and_must_match() {
    let revision = "a".repeat(40);
    assert!(validate_compiled_revision(&revision, None).is_err());
    assert!(validate_compiled_revision(&revision, Some(&"b".repeat(40))).is_err());
    assert!(validate_compiled_revision(&revision, Some("not-a-revision")).is_err());
    assert!(validate_compiled_revision(&revision, Some(&revision)).is_ok());
}

#[test]
fn ready_file_is_create_new_and_contains_no_uri_or_real_credentials() {
    let directory = unique_temp_directory("ready");
    fs::create_dir(&directory).expect("create temp directory");
    let path = directory.join("ready.json");
    let fixture = FixtureIdentity {
        version: "guild-detail-v1".to_string(),
        sha256: fixture_bytes_sha256(),
    };
    let revision = "a".repeat(40);
    let nonce = "b".repeat(64);
    let cookie_value = format!("perf_{}", "c".repeat(64));
    let ready = ReadyFile {
        schema_version: SCHEMA_VERSION,
        host: "127.0.0.1",
        dynamic_port: true,
        port: 34567,
        pid: process::id(),
        revision: &revision,
        nonce: &nonce,
        fixture_mode: FixtureMode::ReadOnly,
        fixture: &fixture,
        guild_id: 100000000000000003,
        cookie_name: SESSION_COOKIE_NAME,
        cookie_value: &cookie_value,
    };
    write_ready_file(&path, &ready).expect("first ready write");
    let body = fs::read_to_string(&path).expect("read ready file");
    let parsed: Value = serde_json::from_str(&body).expect("ready JSON");
    let keys = parsed
        .as_object()
        .expect("ready object")
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    assert_eq!(
        keys,
        HashSet::from_iter(
            [
                "schema_version",
                "host",
                "dynamic_port",
                "port",
                "pid",
                "revision",
                "nonce",
                "fixture_mode",
                "fixture",
                "guild_id",
                "cookie_name",
                "cookie_value",
            ]
            .map(str::to_string)
        )
    );
    assert!(!body.contains("mongodb://"));
    assert!(!body.contains("discord.com"));
    assert!(!body.contains("client_secret"));
    assert!(write_ready_file(&path, &ready).is_err());
    assert_eq!(
        fs::read_to_string(&path).expect("read preserved file"),
        body
    );
    fs::remove_dir_all(directory).expect("remove temp directory");
}

#[test]
fn concurrent_ready_publish_has_one_complete_winner_and_no_temp_residue() {
    let directory = unique_temp_directory("ready-concurrent");
    fs::create_dir(&directory).expect("create temp directory");
    let path = Arc::new(directory.join("ready.json"));
    let bodies = Arc::new(
        (0..8)
            .map(|writer| {
                format!(
                    "{{\"writer\":{writer},\"padding\":\"{}\"}}\n",
                    "x".repeat(64 * 1024)
                )
                .into_bytes()
            })
            .collect::<Vec<_>>(),
    );
    let barrier = Arc::new(Barrier::new(bodies.len()));
    let reader_path = path.clone();
    let reader_bodies = bodies.clone();
    let reader = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match fs::read(reader_path.as_ref()) {
                Ok(body) => {
                    assert!(reader_bodies.iter().any(|candidate| candidate == &body));
                    serde_json::from_slice::<Value>(&body).expect("published JSON is complete");
                    break;
                }
                Err(error) if error.kind() == ErrorKind::NotFound && Instant::now() < deadline => {
                    thread::yield_now();
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    panic!("timed out waiting for a ready-file publisher")
                }
                Err(error) => panic!("polling reader failed: {error}"),
            }
        }
    });

    let publishers = (0..bodies.len())
        .map(|index| {
            let body = bodies[index].clone();
            let barrier = barrier.clone();
            let path = path.clone();
            thread::spawn(move || {
                barrier.wait();
                publish_ready_bytes(path.as_ref(), &body).is_ok()
            })
        })
        .collect::<Vec<_>>();
    let winners = publishers
        .into_iter()
        .map(|publisher| publisher.join().expect("publisher thread"))
        .filter(|won| *won)
        .count();
    reader.join().expect("polling reader thread");

    assert_eq!(winners, 1);
    let published = fs::read(path.as_ref()).expect("published ready file");
    assert!(bodies.iter().any(|candidate| candidate == &published));
    serde_json::from_slice::<Value>(&published).expect("final ready JSON is complete");
    let residue = fs::read_dir(&directory)
        .expect("read ready directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.path() != path.as_path())
        .count();
    assert_eq!(residue, 0);
    fs::remove_dir_all(directory).expect("remove temp directory");
}

#[test]
fn compiled_fixture_version_must_match_environment_version() {
    let mut config = PerfHarnessConfig {
        revision: "a".repeat(40),
        nonce: "b".repeat(64),
        fixture_mode: FixtureMode::GuildDetail,
        fixture: FixtureIdentity {
            version: "wrong-version".to_string(),
            sha256: fixture_bytes_sha256(),
        },
        ready_file: env::temp_dir().join("unused-ready-file.json"),
    };
    assert!(FixtureData::load(&config).is_err());
    config.fixture.version = "guild-detail-v1".to_string();
    let fixture = FixtureData::load(&config).expect("matching rich fixture");
    assert_eq!(fixture.session.guilds.len(), 100);
    assert_eq!(fixture.guild_id(), 9000000000000000101);
    assert_eq!(
        fixture.settings.deployment.modules.get("stock"),
        Some(&DeploymentModuleSettings {
            installed: true,
            enabled: true,
        })
    );
    assert_eq!(
        fixture.settings.deployment.commands.get("etf"),
        Some(&DeploymentCommandSettings {
            installed: true,
            enabled: false,
            configuration: serde_json::json!({ "ticker_1": "DEPLOYMENT-CANARY" }),
        })
    );
    assert_eq!(
        fixture.settings.guild.modules.get("stock"),
        Some(&GuildModuleSettings {
            enabled: false,
            configuration: serde_json::json!({
                "default_symbol": "PERF-STOCK-CANARY",
                "etf_tickers": ["SPY"],
                "refresh_interval_seconds": 3,
                "refresh_duration_seconds": 60,
            }),
        })
    );
    assert_eq!(
        fixture.settings.guild.commands.get("etf"),
        Some(&GuildCommandSettings {
            enabled: true,
            configuration: serde_json::json!({ "ticker_1": "GUILD-ETF-CANARY" }),
        })
    );
    assert_eq!(fixture.route_payloads.public_padding_bytes, 0);
    assert_eq!(fixture.route_payloads.guild_detail_padding_bytes, 4096);
}

#[tokio::test]
async fn fixture_modes_enforce_distinct_read_allowlists_and_live_evidence() {
    let (public_app, public_runtime, _shutdown, _) = fixture_app_for_mode(FixtureMode::Public);
    assert_eq!(
        public_app
            .clone()
            .oneshot(request("GET", "/", None))
            .await
            .expect("public root")
            .status(),
        StatusCode::OK
    );
    for denied in ["/selector", public_runtime.guild_path()] {
        assert_eq!(
            public_app
                .clone()
                .oneshot(request("GET", denied, None))
                .await
                .expect("public denied route")
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let public_counters = json_body(
        public_app
            .oneshot(request("GET", "/__perf/counters", None))
            .await
            .expect("public counters"),
    )
    .await;
    assert_eq!(public_counters["repository_reads"], 0);
    assert_eq!(public_counters["repository_mutations"], 0);
    assert_eq!(public_counters["provider_guild_lookups"], 0);

    let (guild_app, guild_runtime, _shutdown, _) = fixture_app_for_mode(FixtureMode::GuildDetail);
    for denied in ["/", "/selector"] {
        assert_eq!(
            guild_app
                .clone()
                .oneshot(request("GET", denied, None))
                .await
                .expect("guild-detail denied route")
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        guild_app
            .clone()
            .oneshot(request(
                "GET",
                guild_runtime.guild_path(),
                Some((SESSION_COOKIE_NAME, &guild_runtime.cookie_value)),
            ))
            .await
            .expect("guild-detail target")
            .status(),
        StatusCode::OK
    );
    let guild_counters = json_body(
        guild_app
            .oneshot(request("GET", "/__perf/counters", None))
            .await
            .expect("guild counters"),
    )
    .await;
    assert_eq!(guild_counters["repository_reads"], 2);
    assert_eq!(guild_counters["repository_mutations"], 0);
    assert_eq!(guild_counters["provider_guild_lookups"], 1);

    let (readonly_app, readonly_runtime, _shutdown, _) =
        fixture_app_for_mode(FixtureMode::ReadOnly);
    for allowed in ["/", "/selector", readonly_runtime.guild_path()] {
        let cookie = (allowed != "/")
            .then_some((SESSION_COOKIE_NAME, readonly_runtime.cookie_value.as_str()));
        assert_eq!(
            readonly_app
                .clone()
                .oneshot(request("GET", allowed, cookie))
                .await
                .expect("read-only allowed route")
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        readonly_app
            .oneshot(request("GET", "/deployment", None))
            .await
            .expect("read-only denied route")
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn instance_has_exact_node_abi_and_allowed_pages_are_read_only() {
    let (app, runtime, _shutdown, _) = fixture_app();
    let instance_response = app
        .clone()
        .oneshot(request("GET", "/__perf/instance", None))
        .await
        .expect("instance response");
    assert_eq!(instance_response.status(), StatusCode::OK);
    let instance = json_body(instance_response).await;
    assert!(!instance.to_string().contains(&runtime.cookie_value));
    let keys = instance
        .as_object()
        .expect("instance object")
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    assert_eq!(
        keys,
        HashSet::from_iter(
            [
                "schema_version",
                "revision",
                "nonce",
                "pid",
                "fixture_mode",
                "fixture",
                "outbound_calls",
                "browser_outbound_attempts",
            ]
            .map(str::to_string)
        )
    );

    let public = app
        .clone()
        .oneshot(request("GET", "/", None))
        .await
        .expect("public page");
    assert_eq!(public.status(), StatusCode::OK);

    let selector = app
        .clone()
        .oneshot(request(
            "GET",
            "/selector",
            Some((SESSION_COOKIE_NAME, &runtime.cookie_value)),
        ))
        .await
        .expect("selector page");
    assert_eq!(selector.status(), StatusCode::OK);
    let selector_html = to_bytes(selector.into_body(), 2 * 1024 * 1024)
        .await
        .expect("bounded selector page");
    let selector_html = String::from_utf8(selector_html.to_vec()).expect("UTF-8 selector page");
    assert!(selector_html.contains(runtime.guild_path()));
    assert!(!selector_html.contains("cdn.discordapp.com"));

    let guild = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("{}?tab=modules", runtime.guild_path()),
            Some((SESSION_COOKIE_NAME, &runtime.cookie_value)),
        ))
        .await
        .expect("guild page");
    assert_eq!(guild.status(), StatusCode::OK);
    let guild_html = to_bytes(guild.into_body(), 2 * 1024 * 1024)
        .await
        .expect("bounded guild page");
    let guild_html = String::from_utf8(guild_html.to_vec()).expect("UTF-8 guild page");
    assert!(!guild_html.contains("cdn.discordapp.com"));
    assert!(
            guild_html.contains(
                "Installed: On | Deployment: On | Local guild: Off | Effective: Off | Blocked by local guild setting"
            )
        );
    assert!(guild_html.contains("value=\"PERF-STOCK-CANARY\""));

    let commands = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("{}?tab=commands", runtime.guild_path()),
            Some((SESSION_COOKIE_NAME, &runtime.cookie_value)),
        ))
        .await
        .expect("guild commands page");
    assert_eq!(commands.status(), StatusCode::OK);
    let commands_html = to_bytes(commands.into_body(), 2 * 1024 * 1024)
        .await
        .expect("bounded guild commands page");
    let commands_html =
        String::from_utf8(commands_html.to_vec()).expect("UTF-8 guild commands page");
    assert!(commands_html.contains("value=\"GUILD-ETF-CANARY\""));
    assert!(commands_html.contains(
            "Parent module: Off | Installed: On | Deployment: Off | Local guild: On | Effective: Off | Blocked by parent module"
        ));

    for query in [
        "tab=overview",
        "tab=modules",
        "tab=commands",
        "tab=logs&log_entity=module&log_action=toggle&log_page=2",
    ] {
        let response = app
            .clone()
            .oneshot(request(
                "GET",
                &format!("{}?{query}", runtime.guild_path()),
                Some((SESSION_COOKIE_NAME, &runtime.cookie_value)),
            ))
            .await
            .expect("canonical guild query response");
        assert_eq!(response.status(), StatusCode::OK, "query {query}");
    }

    let font = app
        .clone()
        .oneshot(request("GET", FIRA_SANS_REGULAR_PATH, None))
        .await
        .expect("font response");
    assert_eq!(font.status(), StatusCode::OK);
    assert_eq!(font.headers()[header::CONTENT_TYPE], "font/woff2");

    let counters = app
        .oneshot(request("GET", "/__perf/counters", None))
        .await
        .expect("counter response");
    let counters = json_body(counters).await;
    let counter_keys = counters
        .as_object()
        .expect("counter object")
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    assert_eq!(
        counter_keys,
        HashSet::from_iter(
            [
                "schema_version",
                "outbound_calls",
                "browser_outbound_attempts",
                "denied_requests",
                "server_write_attempts",
                "repository_reads",
                "repository_mutations",
                "provider_guild_lookups",
            ]
            .map(str::to_string)
        )
    );
    assert_eq!(counters["schema_version"], SCHEMA_VERSION);
    assert_eq!(counters["denied_requests"], 0);
    assert_eq!(counters["server_write_attempts"], 0);
    // The six guild-page loads perform two setting reads each, and the log tab
    // performs one audit-log read.
    assert_eq!(counters["repository_reads"], 13);
    assert_eq!(counters["repository_mutations"], 0);
    // Selector checks 100 guild cards; the six guild-page loads each check the
    // selected guild once after guild-detail presence batching.
    assert_eq!(counters["provider_guild_lookups"], 106);
    assert_eq!(counters["outbound_calls"], 0);
    assert_eq!(counters["browser_outbound_attempts"], 0);
}

#[tokio::test]
async fn outer_guard_blocks_auth_and_business_writes_before_handlers() {
    let (app, _runtime, _shutdown, _) = fixture_app();
    for (method, path) in [
        ("GET", "/login"),
        ("GET", "/logout"),
        ("GET", "/?unexpected=query"),
        ("GET", "/guild/9000000000000000101?tab=logs&tab=modules"),
        ("GET", "/guild/9000000000000000101?tab=logs&unknown=value"),
        ("GET", "/guild/9000000000000000101?tab=logs&log_page=01"),
        ("PATCH", "/api/guild-settings/100000000000000003/info"),
    ] {
        let response = app
            .clone()
            .oneshot(request(method, path, None))
            .await
            .expect("guard response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let counters = app
        .oneshot(request("GET", "/__perf/counters", None))
        .await
        .expect("counter response");
    let counters = json_body(counters).await;
    assert_eq!(counters["denied_requests"], 7);
    assert_eq!(counters["server_write_attempts"], 1);
    assert_eq!(counters["repository_mutations"], 0);
}

#[tokio::test]
async fn browser_control_uses_fresh_secret_and_proves_counter_is_live() {
    let (app, runtime, _shutdown, _) = fixture_app();
    let (_other_app, other_runtime, _other_shutdown, _) = fixture_app();
    let secret_hex = runtime
        .cookie_value
        .strip_prefix("perf_")
        .expect("control secret prefix");
    assert!(is_lower_hex(secret_hex, 64));
    assert_ne!(runtime.cookie_value, other_runtime.cookie_value);
    assert_ne!(runtime.cookie_value, runtime.nonce);

    let public_nonce = Request::builder()
        .method("POST")
        .uri("/__perf/browser-outbound-attempt")
        .header("x-dynamo-perf-nonce", &runtime.nonce)
        .body(Body::empty())
        .expect("old control request");
    let public_nonce = app
        .clone()
        .oneshot(public_nonce)
        .await
        .expect("public nonce response");
    assert_eq!(public_nonce.status(), StatusCode::FORBIDDEN);

    let wrong_secret = Request::builder()
        .method("POST")
        .uri("/__perf/browser-outbound-attempt")
        .header(PERF_CONTROL_HEADER, "wrong")
        .body(Body::empty())
        .expect("wrong control request");
    let wrong_secret = app
        .clone()
        .oneshot(wrong_secret)
        .await
        .expect("wrong secret response");
    assert_eq!(wrong_secret.status(), StatusCode::FORBIDDEN);

    let queried = Request::builder()
        .method("POST")
        .uri("/__perf/browser-outbound-attempt?unexpected=query")
        .header(PERF_CONTROL_HEADER, &runtime.cookie_value)
        .body(Body::empty())
        .expect("queried control request");
    let queried = app
        .clone()
        .oneshot(queried)
        .await
        .expect("queried control response");
    assert_eq!(queried.status(), StatusCode::FORBIDDEN);

    let correct = Request::builder()
        .method("POST")
        .uri("/__perf/browser-outbound-attempt")
        .header(PERF_CONTROL_HEADER, &runtime.cookie_value)
        .body(Body::empty())
        .expect("control request");
    let correct = app
        .clone()
        .oneshot(correct)
        .await
        .expect("correct nonce response");
    assert_eq!(correct.status(), StatusCode::NO_CONTENT);

    let counters = app
        .oneshot(request("GET", "/__perf/counters", None))
        .await
        .expect("counter response");
    let counters = json_body(counters).await;
    assert_eq!(counters["denied_requests"], 3);
    assert_eq!(counters["server_write_attempts"], 3);
    assert_eq!(counters["browser_outbound_attempts"], 1);
}

#[test]
fn runtime_debug_redacts_control_capability_and_shutdown_sender() {
    let (_app, runtime, _shutdown, _) = fixture_app();
    let control_secret = runtime.cookie_value.clone();
    let debug = format!("{runtime:?}");

    assert!(!debug.contains(&control_secret));
    assert!(debug.contains("cookie_value: \"[redacted]\""));
    assert!(debug.contains("shutdown_sender: \"[redacted]\""));
}

#[tokio::test]
async fn authenticated_shutdown_completes_graceful_signal() {
    let (app, runtime, mut shutdown, _) = fixture_app();
    let wrong = Request::builder()
        .method("POST")
        .uri("/__perf/shutdown")
        .header(PERF_CONTROL_HEADER, "wrong")
        .body(Body::empty())
        .expect("shutdown request");
    assert_eq!(
        app.clone()
            .oneshot(wrong)
            .await
            .expect("wrong shutdown response")
            .status(),
        StatusCode::FORBIDDEN
    );
    assert!(shutdown.try_recv().is_err());

    let correct = Request::builder()
        .method("POST")
        .uri("/__perf/shutdown")
        .header(PERF_CONTROL_HEADER, &runtime.cookie_value)
        .body(Body::empty())
        .expect("shutdown request");
    assert_eq!(
        app.oneshot(correct)
            .await
            .expect("shutdown response")
            .status(),
        StatusCode::NO_CONTENT
    );
    shutdown.await.expect("graceful shutdown signal");
}

#[tokio::test]
async fn in_memory_repositories_prove_fixture_reads_and_deny_bypass_mutation() {
    let config = PerfHarnessConfig {
        revision: "a".repeat(40),
        nonce: "b".repeat(64),
        fixture_mode: FixtureMode::ReadOnly,
        fixture: FixtureIdentity {
            version: "guild-detail-v1".to_string(),
            sha256: fixture_bytes_sha256(),
        },
        ready_file: env::temp_dir().join("unused-ready-file.json"),
    };
    let fixture = FixtureData::load(&config).expect("valid compiled fixture");
    let (sender, _receiver) = oneshot::channel();
    let runtime =
        Arc::new(PerfRuntime::new(&config, &fixture, sender).expect("performance runtime"));
    let state = build_fixture_state(&fixture, runtime.clone(), 45678).expect("fixture state");

    let deployment = state
        .persistence
        .deployment_settings
        .as_ref()
        .expect("fixture deployment repository")
        .get()
        .await
        .expect("fixture deployment read");
    assert_eq!(
        deployment.modules.get("stock"),
        Some(&DeploymentModuleSettings {
            installed: true,
            enabled: true,
        })
    );
    assert!(!deployment.commands["etf"].enabled);
    assert_eq!(
        deployment.commands["etf"].configuration,
        serde_json::json!({ "ticker_1": "DEPLOYMENT-CANARY" })
    );
    let guild = state
        .persistence
        .guild_settings
        .as_ref()
        .expect("fixture guild repository")
        .get(fixture.guild_id())
        .await
        .expect("fixture guild read")
        .expect("fixture guild exists");
    assert_eq!(guild.guild_id, fixture.guild_id());
    assert!(!guild.modules["stock"].enabled);
    assert_eq!(
        guild.modules["stock"].configuration,
        serde_json::json!({
            "default_symbol": "PERF-STOCK-CANARY",
            "etf_tickers": ["SPY"],
            "refresh_interval_seconds": 3,
            "refresh_duration_seconds": 60,
        })
    );
    assert!(guild.commands["etf"].enabled);
    assert_eq!(
        guild.commands["etf"].configuration,
        serde_json::json!({ "ticker_1": "GUILD-ETF-CANARY" })
    );
    assert_eq!(runtime.repository_reads.load(Ordering::SeqCst), 2);
    assert_eq!(runtime.repository_mutations.load(Ordering::SeqCst), 0);

    let mutation = state
        .persistence
        .deployment_settings
        .as_ref()
        .expect("fixture deployment repository")
        .upsert_module_settings("info", DeploymentModuleSettings::default())
        .await;
    assert!(mutation.is_err());
    assert_eq!(runtime.repository_mutations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn outbound_http_seam_counts_and_denies_before_send() {
    let config = PerfHarnessConfig {
        revision: "a".repeat(40),
        nonce: "b".repeat(64),
        fixture_mode: FixtureMode::ReadOnly,
        fixture: FixtureIdentity {
            version: "guild-detail-v1".to_string(),
            sha256: fixture_bytes_sha256(),
        },
        ready_file: env::temp_dir().join("unused-ready-file.json"),
    };
    let fixture = FixtureData::load(&config).expect("valid compiled fixture");
    let (sender, _receiver) = oneshot::channel();
    let runtime =
        Arc::new(PerfRuntime::new(&config, &fixture, sender).expect("performance runtime"));
    let state = build_fixture_state(&fixture, runtime.clone(), 45678).expect("fixture state");
    let request = state.http.get("https://example.invalid/must-not-send");
    let error = super::super::discord::send_dashboard_http(&state, request)
        .await
        .expect_err("harness outbound must fail");
    assert!(error.to_string().contains("disabled"));
    assert_eq!(runtime.outbound_calls.load(Ordering::SeqCst), 1);
}
