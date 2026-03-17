# Current Repo Rust Cutover Status

This note records the completed repo split where the current repository became the Rust mainline and the legacy JavaScript runtime moved to [`Dynamo-JS`](https://github.com/Ayumudayo/Dynamo-JS).

## 1. Product Decisions Locked

- This repository remains the canonical Rust repository.
- `currency`, `stock`, `gameinfo`, `ticket`, `suggestion`, `greeting`, `invite`, `stats`, `moderation`, `giveaway`, and `dashboard` stay in scope.
- `giveaway` is a core module.
- `music` stays paused and outside the active public template surface.
- Dashboard OAuth, audit logs, launcher scripts, startup reports, and Playwright smoke remain part of the public template.

## 2. Published JS Archive Repository Contents

The published `Dynamo-JS` archive includes:

- `src/`
- `dashboard/`
- `bot.js`
- `config.js`
- `package.json`
- `package-lock.json`
- `docs/commands/`
- `scripts/db-v4-to-v5.js`
- `jsconfig.json`
- lint/format config files needed to inspect the old code

`Dynamo-JS` is read-only. Its root README points back to this repository as the active codebase.

## 3. Current Repo Cleanup Applied

The current repository has removed:

- `src/`
- `dashboard/`
- `bot.js`
- `config.js`
- legacy JS lint/config helper files
- legacy JS archive export scripts and templates

The current repository now keeps:

- `crates/`
- launcher scripts
- CI
- dashboard smoke assets
- Rust docs
- a Playwright-only root `package.json`

## 4. Validation After Cleanup

- `cargo fmt --all --check`
- `cargo check`
- `cargo test --workspace`
- dashboard Playwright smoke listing
- one real OAuth smoke in a dev guild
- launcher smoke with bootstrap + dashboard + bot
- `Dynamo-JS` archive export and initial push

## 5. Final Repo Messaging

- Current repo README describes this repository as the Rust mainline.
- Current repo README links to `Dynamo-JS` as the legacy archive.
- `Dynamo-JS` README points back to this repository as the active codebase.
