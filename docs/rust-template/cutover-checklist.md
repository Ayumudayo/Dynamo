# Rust Template Cutover Checklist

Use this checklist before exporting or publishing the fresh Rust-only repository.

## 1. Product Decisions Locked

- `currency`, `stock`, `gameinfo`, `ticket`, `suggestion`, `greeting`, `invite`, `stats`, `moderation`, `giveaway`, and `dashboard` are in scope.
- `giveaway` is a core module.
- `music` is paused and not part of the active public template surface.
- Dashboard OAuth, audit logs, launcher scripts, startup reports, and Playwright smoke remain part of the public template.

## 2. Runtime Baseline

- Giveaway is included in the default module registry.
- Music is not included in the active runtime registry.
- Startup reports show compact service/repository summaries.
- Dashboard command/module settings and audit logs work without JS dashboard dependencies.

## 3. Fresh Repo Export Contents

- Include:
  - `crates/`
  - `.cargo/`
  - `.github/workflows/rust-ci.yml`
  - launcher scripts
  - Rust `README.md`
  - Rust `.env.example`
  - minimal smoke-only `package.json`
  - Playwright config and dashboard smoke specs
  - `docs/dev-smoke-checklist.md`
- Exclude:
  - `src/`
  - `dashboard/`
  - `bot.js`
  - `config.js`
  - JS bot dependencies and runtime docs

## 4. Validation Before Publish

- `cargo fmt --all --check`
- `cargo check`
- `cargo test --workspace`
- dashboard Playwright smoke listing
- one real OAuth smoke in a dev guild
- launcher smoke with bootstrap + dashboard + bot

## 5. Archive Transition

- Current repo README is updated to describe the Rust export path and archive intent.
- Fresh Rust repo gets its own clean README and minimal Node manifest.
- Once the fresh repo is published, current repo becomes read-only JS/archive reference.
