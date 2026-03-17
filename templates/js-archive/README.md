# Dynamo-JS

`Dynamo-JS` is the read-only archive of the legacy JavaScript implementation of Dynamo, including the old Discord.js runtime and EJS/Express dashboard.

It is preserved for reference, migration history, and behavior comparison only. New development is expected to continue in the active Rust mainline repository.

## Archive Scope

- `src/` legacy bot runtime
- `dashboard/` legacy web dashboard
- `bot.js` and `config.js`
- original `package.json` and `package-lock.json`
- legacy command docs and migration helper scripts

## Status

- No new feature work is expected here.
- No support promise is implied for modern Discord changes or third-party API drift.
- Use this archive when you need to compare behavior or inspect the original JS implementation during Rust migration work.
