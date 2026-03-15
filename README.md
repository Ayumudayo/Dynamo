# Dynamo

`Dynamo` is a slash-first Discord bot template built with `poise + serenity`, MongoDB-backed runtime settings, and a companion dashboard for deployment and guild configuration.

The repository still contains the legacy JavaScript bot while the Rust migration branch is in flight, but the actively developed template lives in the Rust workspace under [`crates/`](./crates).

## Workspace Layout

- [`crates/bot`](./crates/bot): Discord runtime and slash command registration
- [`crates/dashboard`](./crates/dashboard): `axum` companion dashboard for deployment and guild settings
- [`crates/bootstrap`](./crates/bootstrap): MongoDB bootstrap utility
- [`crates/core`](./crates/core): shared config, state, module registry, repositories, guards
- [`crates/persistence-mongo`](./crates/persistence-mongo): MongoDB repositories and bootstrap
- [`crates/providers/yahoo`](./crates/providers/yahoo): Yahoo Finance provider with persisted crumb/cookie enrichment
- [`crates/modules`](./crates/modules): first-party modules

## Included Core Modules

- `currency`: ExchangeRate-API backed `/exchange` and `/rate` commands
- `info`: basic bot diagnostics
- `gameinfo`: FFXIV world transfer, maintenance, and PLL lookups with fallback cache
- `stock`: Yahoo-backed quote lookups, ETF summaries, refresh sessions
- `greeting`: welcome/farewell templates and preview command
- `invite`: invite attribution, reward role evaluation, invite cache tracking
- `suggestion`: suggestion board workflow with moderator buttons and modal reasons
- `stats`: messages, interactions, XP leveling, voice session tracking
- `moderation`: warnings, timeout, kick, ban, unban, softban, nickname changes
- `music`: deployment-disabled-by-default module with `Songbird` playback and dashboard-configurable guild toggles
- `ticket`: ticket panel, category routing, participant management, transcript logging

## Optional First-Party Modules

These are part of the migration plan but are not registered by default in the public v1 template:

- `giveaway`

Enable them explicitly with environment flags:

- `DYNAMO_ENABLE_GIVEAWAY=true`

Current optional-module status:

- `giveaway`: implemented as an opt-in persisted slash workflow with entry buttons and timed completion polling

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

- `EXCHANGE_API_KEY` required for the currency module
- `MONGODB_DATABASE` default: `dynamo-rs`
- `DASHBOARD_HOST` default: `127.0.0.1`
- `DASHBOARD_PORT` default: `3000`
- `DASHBOARD_BASE_URL` default: `http://127.0.0.1:3000`
- `DISCORD_CLIENT_SECRET` or `BOT_SECRET` required for dashboard OAuth login
- `DASHBOARD_ADMIN_USER_IDS` optional comma-separated override for deployment-wide dashboard admins
- `DISCORD_COMMAND_SYNC_INTERVAL_SECONDS` default: `15`
- `RUST_LOG`

Music runtime notes:

- `Songbird` is the implemented default backend.
- `yt-dlp` must be available on the host path for YouTube URL/search playback.
- Discord non-stage voice channels currently require DAVE/E2EE, which this build does not support yet. `/music join` and `/music play` will refuse regular voice channels and point users at this limitation.
- `Lavalink` is not exposed in the runtime settings UI. If you want to experiment with an external node later, use [`docs/music-lavalink-guide.md`](./docs/music-lavalink-guide.md) as an operational reference only.

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
- `--enable-giveaway` / `-EnableGiveaway`
- `-Headless` for the PowerShell launcher
- `--dry-run` / `-DryRun`

Stop managed dashboard and bot processes:

```powershell
./scripts/dev-down.ps1
```

```bash
./scripts/dev-down.sh
```

The launchers print the effective command scope resolved from `.env`. Optional module state is managed from the dashboard rather than the launcher output.

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
```

## Dashboard

The companion dashboard exposes:

- Discord OAuth login with a Dyno-style server selector for guilds you can manage
- deployment-level module install/enable toggles
- guild-level module enablement and structured settings forms
- deployment-level and guild-level command toggles for individual leaf slash commands
- advanced JSON editor fallback for module configuration
- advanced JSON editor fallback for command configuration
- effective module state rendering shared with the runtime guard layer
- runtime notices for modules with known platform limitations, such as the current DAVE restriction on `music`

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

## Smoke Checklist

Use [`docs/dev-smoke-checklist.md`](./docs/dev-smoke-checklist.md) for the manual verification flow after changing modules or persistence.

## Current Status

The Rust workspace is now the primary architecture target for the public template. The remaining planned work is mostly optional module work (`giveaway`, `music`) and further UX refinement rather than core bot architecture.
