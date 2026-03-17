# Dynamo

`Dynamo` is a slash-first Discord bot template built with `poise + serenity`, MongoDB-backed runtime settings, and a companion dashboard for deployment and guild configuration.

This repository is the active Rust product line. The legacy JavaScript bot and dashboard still exist here temporarily as migration reference material, but the planned repo split is now the inverse of the earlier approach: the current repository stays as the Rust home, and the legacy JS runtime is exported into a separate read-only archive repository named `Dynamo-JS`.

## Workspace Layout

- [`crates/bot`](./crates/bot): Discord runtime and slash command registration
- [`crates/dashboard`](./crates/dashboard): `axum` companion dashboard for deployment and guild settings
- [`crates/bootstrap`](./crates/bootstrap): MongoDB bootstrap utility
- [`crates/core`](./crates/core): shared config, state, module registry, repositories, guards
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

## Paused Modules

- `music`: code remains in the workspace for future DAVE work, but it is intentionally not part of the active public template surface.
- `music` launcher flags, smoke steps, and runtime support are intentionally excluded from the active Rust template path.

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

Paused music notes:

- Music remains out of the active template runtime while DAVE support is unresolved.
- [`docs/music-lavalink-guide.md`](./docs/music-lavalink-guide.md) is kept only as archived reference material.

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
pm2 start ecosystem.pm2.cjs
pm2 save
```

Useful commands:

```bash
pm2 status
pm2 logs dynamo-dashboard
pm2 logs dynamo-bot
pm2 restart ecosystem.pm2.cjs
pm2 delete ecosystem.pm2.cjs
```

Notes:

- The PM2 wrappers run the release binaries from `target/release/`.
- They expect `.env` to exist in the repo root.
- The Rust binaries still load `.env` themselves, so the wrapper scripts only need to `cd` into the repo root before `exec`.
- On a Raspberry Pi, `cargo build --release` can take noticeably longer than debug builds.

## JS Archive Export

Use the export scripts to stage the `Dynamo-JS` read-only JavaScript archive repo in `output/Dynamo-JS/`:

```powershell
./scripts/export-js-archive.ps1
```

```bash
./scripts/export-js-archive.sh
```

The staged archive includes the legacy JS runtime, JS dashboard, package manifests, command docs, and migration helper scripts. It is intended to become the `Dynamo-JS` archive repository once the split is finalized.

Reference documents for the cutover:

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

## Dashboard

The companion dashboard exposes:

- Discord OAuth login with a Dyno-style server selector for guilds you can manage
- selector pages that show only global navigation until you enter a deployment or guild context
- deployment-level module install/enable toggles
- guild-level module enablement and structured settings forms
- deployment-level and guild-level command toggles for individual leaf slash commands
- tabbed `Overview`, `Modules`, `Commands`, and `Logs` views for guild and deployment pages
- dashboard audit logs for dashboard-originated module and command changes
- effective module state rendering shared with the runtime guard layer
- explicit, human-written command descriptions in the dashboard command catalog

Command sync behavior:

- Guild command sets are re-synchronized from dashboard settings on a polling loop.
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

The Rust workspace in this repository is the primary product line. The next repo-level step is exporting the legacy JS runtime into the `Dynamo-JS` archive repository, then removing those JS paths from this repository so `main` can become the clean Rust-only public template history going forward.
