# Dynamo Rust Template

`Dynamo Rust Template` is a slash-first Discord bot template built with `poise + serenity`, MongoDB-backed runtime settings, and an OAuth-protected dashboard for deployment and guild configuration.

This repository is the Rust-only product line. It intentionally excludes the legacy JavaScript runtime and dashboard from the archived source repository.

## Included Modules

- `currency`
- `stock`
- `gameinfo`
- `greeting`
- `invite`
- `suggestion`
- `stats`
- `moderation`
- `ticket`
- `giveaway`
- `info`

## Paused Areas

- `music` is intentionally out of the active template surface while Discord DAVE support remains unresolved.

## Quick Start

1. Copy `.env.example` to `.env`
2. Run `cargo run -p dynamo-bootstrap`
3. Run `cargo run -p dynamo-dashboard`
4. Run `cargo run -p dynamo-bot`

For local multi-process startup, use:

```powershell
./scripts/dev-up.ps1
```

```bash
./scripts/dev-up.sh
```

## Validation

```powershell
cargo fmt --all --check
cargo check
cargo test --workspace
```

Dashboard smoke:

```powershell
npm install
npm run dashboard:smoke:install
npm run dashboard:smoke:auth
PLAYWRIGHT_GUILD_ID=<guild_id> PLAYWRIGHT_STORAGE_STATE=output/playwright/dashboard-auth.json npm run dashboard:smoke
```
