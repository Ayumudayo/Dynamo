# Dynamo

`Dynamo` is a slash-first Discord bot template built with `poise + serenity`, MongoDB-backed runtime settings, and a companion dashboard for deployment and guild configuration.

This repository is the active Rust product line. The legacy JavaScript bot and dashboard have been split into the read-only archive repository [`Dynamo-JS`](https://github.com/Ayumudayo/Dynamo-JS), and this repository now serves as the canonical Rust home.

## Workspace Layout

- [`crates/bot`](./crates/bot): Discord runtime and slash command registration
- [`crates/dashboard`](./crates/dashboard): `axum` companion dashboard for deployment and guild settings
- [`crates/bootstrap`](./crates/bootstrap): MongoDB bootstrap utility
- [`crates/module-kit`](./crates/module-kit): module trait, manifest, settings schema, command/module catalog descriptors
- [`crates/settings`](./crates/settings): deployment/guild install, enablement, and configuration state
- [`crates/repositories`](./crates/repositories): repository traits for persisted bot and dashboard state
- [`crates/service-stock`](./crates/service-stock): stock quote service contract
- [`crates/service-exchange`](./crates/service-exchange): exchange-rate service contract
- [`crates/runtime-api`](./crates/runtime-api): shared app state, persistence/service registries, context, and error surface
- [`crates/config`](./crates/config): runtime configuration loaded from `.env`
- [`crates/enablement`](./crates/enablement): pure module/command effective-state resolution
- [`crates/access`](./crates/access): context-aware module and command guard helpers
- [`crates/registry`](./crates/registry): module registry, command catalog, and intent aggregation
- [`crates/ops`](./crates/ops): dashboard audit log and command sync state models
- [`crates/observability`](./crates/observability): startup reporting and rendering
- [`crates/domain-*`](./crates): shared domain crates for currency, stock, giveaway, invite, stats, suggestion, and moderation
- [`crates/persistence-mongo`](./crates/persistence-mongo): MongoDB repositories and bootstrap
- [`crates/providers/google-finance`](./crates/providers/google-finance): Google Finance exchange-rate provider with persisted USD-base cache
- [`crates/providers/yahoo`](./crates/providers/yahoo): Yahoo Finance provider with persisted crumb/cookie enrichment
- [`crates/modules`](./crates/modules): first-party modules

## Included Core Modules

- `currency`: Google Finance backed `/exchange` and `/rate` commands with cached fallback
- `info`: basic bot diagnostics
- `gameinfo`: FFXIV world transfer, maintenance, and PLL lookups with fallback cache
- `stock`: Yahoo-backed quote lookups, ETF summaries, refresh sessions
- `greeting`: welcome/farewell templates and preview command
- `invite`: invite attribution, reward role evaluation, invite cache tracking
- `suggestion`: suggestion board workflow with moderator buttons and modal reasons
- `stats`: messages, interactions, XP leveling, voice session tracking
- `moderation`: warnings, timeout, kick, ban, unban, softban, nickname changes
- `giveaway`: persisted giveaway workflow with entry buttons and timed completion polling
- `ticket`: ticket panel, category routing, participant management, transcript logging

## Runtime Model

- Slash-first command model. Prefix parity is not a goal for the public template.
- Shared module enablement guard across bot runtime and dashboard state rendering.
- Command-level enablement and per-command configuration storage for leaf slash commands.
- Deployment-level install/enable state plus guild-level enable/config overrides.
- MongoDB is the default persistence layer and defaults to the `dynamo-rs` database name.
- Dashboard and bot are separate processes.

## Required Environment

Copy [`.env.example`](./.env.example) to `.env` and fill in the values.

Minimum variables:

- `DISCORD_TOKEN` or `BOT_TOKEN`
- `MONGODB_URI` or `MONGO_CONNECTION`
- `DISCORD_DEV_GUILD_ID` or `GUILD_ID` when `DISCORD_REGISTER_GLOBALLY=false`

If `DISCORD_REGISTER_GLOBALLY` is omitted and `DISCORD_DEV_GUILD_ID` or `GUILD_ID` is present, the launcher and bot default to guild-scoped command sync for faster development iteration.

Common optional variables:

- `MONGODB_DATABASE` default: `dynamo-rs`
- `DASHBOARD_HOST` default: `127.0.0.1`
- `DASHBOARD_PORT` default: `3000`
- `DASHBOARD_BASE_URL` default: `http://127.0.0.1:3000`
- `DISCORD_CLIENT_SECRET` or `BOT_SECRET` required for dashboard OAuth login
- `DASHBOARD_ADMIN_USER_IDS` optional comma-separated override for deployment-wide dashboard admins
- `DISCORD_COMMAND_SYNC_INTERVAL_SECONDS` default: `15`
- `RUST_LOG`

The checked-in [`.env.example`](./.env.example) uses `DASHBOARD_PORT=4000` and matching `DASHBOARD_BASE_URL` as a sample external dashboard port for home-server deployments. The application defaults are still `3000` unless you set them explicitly.

## Discord Intents

Enable these gateway intents for the application when using the default core module set:

- `GUILDS`
- `GUILD_MEMBERS`
- `GUILD_MESSAGES`
- `GUILD_INVITES`
- `GUILD_VOICE_STATES`

`MESSAGE_CONTENT` is not required for the public slash-first template.

## Quick Start

1. Create `.env` from [`.env.example`](./.env.example).
2. Bootstrap MongoDB collections:

```powershell
cargo run -p dynamo-bootstrap
```

3. Start the dashboard:

```powershell
cargo run -p dynamo-dashboard
```

4. Start the bot:

```powershell
cargo run -p dynamo-bot
```

If `DISCORD_REGISTER_GLOBALLY=false`, commands are registered only in `DISCORD_DEV_GUILD_ID` or `GUILD_ID`.

## Startup Scripts

Use the launcher scripts under [`scripts/`](./scripts) to bootstrap MongoDB and start the dashboard and bot with log files and pid files under `logs/`.
They prebuild `dynamo-bootstrap`, `dynamo-dashboard`, and `dynamo-bot` once with a single `cargo build` invocation, then run the shared binaries from `target/debug/`.
Bot startup logs include the resolved command scope, loaded module count, loaded leaf command count, and loaded module ids. Dashboard startup logs include the listening URL plus loaded module and command counts.
Long startup lists are compacted as `count + preview` so the report stays readable in terminals and server logs.
The bot startup report also shows whether the Google Finance exchange-rate cache service is wired and whether the 30-minute refresh loop is active.

PowerShell:

```powershell
./scripts/dev-up.ps1
```

POSIX shell:

```bash
./scripts/dev-up.sh
```

Useful flags:

- `--skip-build` / `-SkipBuild`
- `--skip-bootstrap` / `-SkipBootstrap`
- `-Headless` for the PowerShell launcher
- `--dry-run` / `-DryRun`

Stop managed dashboard and bot processes:

```powershell
./scripts/dev-down.ps1
```

```bash
./scripts/dev-down.sh
```

The launchers print the effective command scope resolved from `.env`.

## Raspberry Pi / PM2

For a Raspberry Pi or Ubuntu-style server where you want to keep the bot and dashboard under `pm2`, use the release wrappers instead of the development launchers.

1. Build the release binaries:

```bash
./scripts/prod-build.sh
```

2. Run bootstrap once:

```bash
./scripts/prod-bootstrap.sh
```

3. Start the long-running processes with `pm2`:

```bash
pm2 start ecosystem.config.js
pm2 save
```

Useful commands:

```bash
pm2 status
pm2 logs dynamo-dashboard
pm2 logs dynamo-bot
pm2 restart ecosystem.config.js
pm2 delete ecosystem.config.js
```

Notes:

- The PM2 wrappers run the release binaries from `target/release/`.
- They expect `.env` to exist in the repo root.
- The Rust binaries still load `.env` themselves, so the wrapper scripts only need to `cd` into the repo root before `exec`.
- On a Raspberry Pi, `cargo build --release` can take noticeably longer than debug builds.

### Cross-build On This PC And Deploy To Raspberry Pi

If you do not want to compile on the Raspberry Pi itself, build `aarch64-unknown-linux-gnu` binaries on this machine and push them over SSH.

Prerequisites on this machine:

- `zig` on `PATH`
- `cargo-zigbuild` installed:
  - `cargo install cargo-zigbuild`
- SSH access to the Raspberry Pi

Build a deployment bundle:

```powershell
./scripts/build-rpi-aarch64.ps1
```

```bash
./scripts/build-rpi-aarch64.sh
```

Deploy and restart remotely:

```powershell
./scripts/deploy-rpi-aarch64.ps1 -RemoteHost <pi-host> -RemoteUser <pi-user>
```

```bash
./scripts/deploy-rpi-aarch64.sh --host <pi-host> --user <pi-user>
```

Optional flags:

- `--skip-build` / `-SkipBuild`
- `--skip-bootstrap` / `-SkipBootstrap`
- `--force-bootstrap` / `-ForceBootstrap`
- `--port` / `-Port`
- `--app-dir` / `-AppDir`
- `--key` / `-KeyPath`

The deploy script stages a compact bundle under `output/rpi-aarch64/`, uploads it as a single archive, ensures the remote shell scripts are executable, auto-runs bootstrap only on the first successful deploy after `.env` exists, and then calls `pm2 startOrRestart ecosystem.config.js --update-env`.
If you configure SSH key authentication and pass `--key` / `-KeyPath` (or set `RPI_SSH_KEY`), repeated password prompts are eliminated.

## Legacy JS Archive

The old Discord.js runtime and EJS dashboard now live in the read-only archive repository [`Dynamo-JS`](https://github.com/Ayumudayo/Dynamo-JS).

Reference notes for the cutover remain here:

- [`docs/cutover/js-pattern-audit.md`](./docs/cutover/js-pattern-audit.md)
- [`docs/cutover/current-repo-rust-cutover-checklist.md`](./docs/cutover/current-repo-rust-cutover-checklist.md)

## Validation Commands

These are the baseline checks used during development and CI:

```powershell
cargo fmt --all --check
cargo check
cargo test --workspace
```

Live network smoke checks for Yahoo enrichment are available but intentionally ignored by default:

```powershell
cargo test -p dynamo-provider-yahoo live_quote_summary_enrichment_returns_rich_nvda_quote -- --ignored --nocapture
cargo test -p dynamo-provider-yahoo live_quote_summary_persists_yahoo_session_to_mongodb -- --ignored --nocapture
cargo test -p dynamo-provider-google-finance
```

Node tooling at the repository root is now limited to Playwright smoke only. The root `package.json` is no longer a bot runtime manifest.

## Dashboard

The companion dashboard exposes:

- Discord OAuth login with a Dyno-style server selector for guilds you can manage
- selector pages that show only global navigation until you enter a deployment or guild context
- deployment-level module install/enable toggles
- guild-level module enablement and structured settings forms
- deployment-level and guild-level command toggles for individual leaf slash commands
- deployment-level and guild-level manual command sync buttons with sync status panels
- tabbed `Overview`, `Modules`, `Commands`, and `Logs` views for guild and deployment pages
- dashboard audit logs for dashboard-originated module and command changes
- effective module state rendering shared with the runtime guard layer
- explicit, human-written command descriptions in the dashboard command catalog

Command sync behavior:

- Guild command sets are re-synchronized from dashboard settings on a polling loop.
- The Commands tab can queue a manual sync request when you need an immediate guild or global refresh.
- Deployment and command toggle changes are reflected in runtime checks immediately after the next sync cycle.
- Global commands still depend on Discord propagation behavior; guild command sync is the immediate path.

Open:

- [http://127.0.0.1:3000/](http://127.0.0.1:3000/)
- [http://127.0.0.1:3000/selector](http://127.0.0.1:3000/selector)
- [http://127.0.0.1:3000/deployment](http://127.0.0.1:3000/deployment)
- `http://127.0.0.1:3000/guild/<guild_id>`

OAuth notes:

- Add the dashboard callback URL `{DASHBOARD_BASE_URL}/auth/discord/callback` to your Discord application OAuth settings.
- The dashboard signs users in with `identify` and `guilds` scopes.
- Guild pages are available only for servers where the signed-in user has `Manage Server` or `Administrator`.
- The deployment page is restricted to the bot application owner or `DASHBOARD_ADMIN_USER_IDS`.

Playwright smoke:

- Install Chromium once: `npm run dashboard:smoke:install`
- Create a reusable authenticated storage state after manual login:
  - `npm run dashboard:smoke:auth`
- Then run the smoke suite with:
  - `PLAYWRIGHT_GUILD_ID=<guild_id> PLAYWRIGHT_STORAGE_STATE=output/playwright/dashboard-auth.json npm run dashboard:smoke`
- Override the dashboard host if needed with `PLAYWRIGHT_BASE_URL`
- The smoke suite expects the guild page to expose real `Logs` tab entries after a settings save.

## Smoke Checklist

Use [`docs/dev-smoke-checklist.md`](./docs/dev-smoke-checklist.md) for the manual verification flow after changing modules or persistence.

## Current Status

This repository is now the Rust mainline. The legacy JS runtime lives in [`Dynamo-JS`](https://github.com/Ayumudayo/Dynamo-JS), while the current root keeps only Rust runtime code, Rust deployment assets, and Playwright dashboard smoke tooling.
