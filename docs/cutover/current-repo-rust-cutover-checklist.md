# Current Repo Rust Cutover Checklist

Use this checklist before splitting the legacy JS runtime into the `Dynamo-JS` archive repository and cleaning this repository into the Rust-only mainline.

## 1. Product Decisions Locked

- This repository remains the canonical Rust repository.
- `currency`, `stock`, `gameinfo`, `ticket`, `suggestion`, `greeting`, `invite`, `stats`, `moderation`, `giveaway`, and `dashboard` stay in scope.
- `giveaway` is a core module.
- `music` stays paused and outside the active public template surface.
- Dashboard OAuth, audit logs, launcher scripts, startup reports, and Playwright smoke remain part of the public template.

## 2. JS Archive Repository Contents

The staged `Dynamo-JS` archive should include:

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

The staged `Dynamo-JS` archive should not be treated as active development output. Its root README should say the archive is read-only and that active development continues in the Rust mainline repository.

## 3. Current Repo Cleanup After Archive Split

After the archive repo is published:

- remove `src/`
- remove `dashboard/`
- remove `bot.js`
- remove `config.js`
- rewrite the root `package.json` into a Playwright-only smoke manifest
- keep `crates/`, launcher scripts, CI, dashboard smoke assets, and Rust docs

## 4. Validation Before Cleanup

- `cargo fmt --all --check`
- `cargo check`
- `cargo test --workspace`
- dashboard Playwright smoke listing
- one real OAuth smoke in a dev guild
- launcher smoke with bootstrap + dashboard + bot
- `Dynamo-JS` export smoke with both PowerShell and shell scripts

## 5. Final Repo Messaging

- Current repo README must describe this repository as the Rust mainline.
- Current repo README must point to the `Dynamo-JS` export flow until the split is complete.
- The eventual `Dynamo-JS` archive repo README must point back to this repository as the active codebase.
