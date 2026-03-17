# Dev Smoke Checklist

Run this checklist after major module, persistence, or dashboard changes. This checklist applies to the current repository as the Rust mainline product; JS archive validation is tracked separately in the cutover docs.

## Bootstrap

1. Run `cargo run -p dynamo-bootstrap`.
2. Confirm the `dynamo-rs` database exists under `dynamo-cluster`.
3. Confirm collections exist for:
   - `deployment_settings`
   - `guild_settings`
   - `provider_state`
   - `suggestions`
   - `giveaways`
   - `members`
   - `member-stats`
   - `mod-logs`
   - `dashboard-audit-logs`

## Startup

1. Run `./scripts/dev-up.ps1` or `./scripts/dev-up.sh`.
2. Confirm the dashboard root and deployment page load.
3. Confirm the bot startup log shows:
   - persistence database name
   - command scope
   - loaded module count
   - loaded leaf command count
4. Confirm the dashboard startup log shows:
   - persistence database name
   - host and port
   - loaded module count
   - loaded leaf command count
5. Confirm commands register in the development guild when `DISCORD_REGISTER_GLOBALLY=false`.

## Dashboard

1. Open `/` and confirm the landing page shows a Discord sign-in CTA.
2. Complete Discord OAuth login and confirm `/selector` lists only guilds where the signed-in user has `Manage Server` or `Administrator`.
3. Open `/deployment` with an admin account and confirm it loads.
4. Open `/guild/<guild_id>`.
5. Change one structured field and confirm it persists after reload.
6. Confirm the selector page does not show guild-only section links before a server is selected.
7. Confirm guild and deployment pages show `Overview`, `Modules`, `Commands`, and `Logs` tabs.
8. Confirm module and command cards remain compact at `1440`, `1024`, and `768`.
9. Confirm command category tabs filter the visible command cards.
10. Open the `Logs` tab after a dashboard mutation and confirm a new audit row appears with the correct actor and target.
11. Disable one leaf command in deployment settings and confirm it disappears or becomes unavailable after the next sync cycle.
12. Disable one leaf command in guild settings and confirm the guild-specific command set updates after the next sync cycle.
13. Optional automated smoke:
   - `npm run dashboard:smoke:install`
   - `npm run dashboard:smoke:auth`
   - `PLAYWRIGHT_GUILD_ID=<guild_id> PLAYWRIGHT_STORAGE_STATE=output/playwright/dashboard-auth.json npm run dashboard:smoke`

## Core Commands

1. Run `/ping`.
2. Run `/wtinv`.
3. Run `/maint`.
4. Run `/pll`.
5. Run `/stock NVDA`.
6. Run `/etf`.

## Currency

1. Run `/exchange` with no arguments and confirm dashboard defaults are applied.
2. Run `/exchange USD KRW 1` and confirm the embed shows a Google Finance backed value plus a normal Discord timestamp.
3. Run `/rate` and confirm each row shows a rate, with cached fallback only appearing when a live lookup fails.
4. Force or simulate a fetch failure and confirm cached fallback values are shown instead of a hard failure.
5. Confirm a `provider_state` document exists for the Google Finance exchange cache.
6. Confirm the bot startup log shows the exchange-rate cache service and 30-minute refresh loop status.

## Greeting And Invite

1. Enable `greeting` and `invite` in deployment and guild settings.
2. Configure welcome and farewell content.
3. Run `/greeting preview`.
4. Join the guild through a tracked invite and verify:
   - invite counters update
   - reward roles apply when thresholds are met
   - welcome message renders expected placeholders
5. Leave the guild and verify farewell dispatch plus invite decrement behavior.

## Suggestion

1. Configure suggestion channel settings.
2. Run `/suggest`.
3. Approve, reject, and delete a suggestion with moderator controls.
4. Verify status updates persist in MongoDB.

## Giveaway

1. Configure the giveaway default channel or pass a channel explicitly.
2. Run `/giveaway start`.
3. Use the entry button from multiple users.
4. Run `/giveaway pause`, `/giveaway resume`, `/giveaway edit`, and `/giveaway list`.
5. Run `/giveaway end` or wait for the poller to finish it automatically.
6. Run `/giveaway reroll` after completion.

## Ticket

1. Configure ticket categories, log channel, and panel content.
2. Run `/ticket setup`.
3. Open a ticket from the panel.
4. Add and remove a participant.
5. Close the ticket and confirm the transcript attachment is sent to the log channel.

## Stats

1. Enable `stats`.
2. Send messages and confirm XP accumulates.
3. Trigger a level-up and verify the configured channel or fallback channel receives the message.
4. Join and leave a voice channel and verify voice connection count and time accumulation.

## Moderation

1. Run `/warn` and `/warnings list`.
2. Run `/warnings clear`.
3. Run `/timeout` and `/untimeout`.
4. Run `/nick`.
5. Run `/kick`, `/ban`, `/unban`, and `/softban` in a safe test guild.
6. Confirm modlog output is sent when `modlog_channel_id` is configured.

## Yahoo Provider

1. Run the ignored smoke tests when stock provider changes:
   - `cargo test -p dynamo-provider-yahoo live_quote_summary_enrichment_returns_rich_nvda_quote -- --ignored --nocapture`
   - `cargo test -p dynamo-provider-yahoo live_quote_summary_persists_yahoo_session_to_mongodb -- --ignored --nocapture`
2. Confirm a `provider_state` document exists for Yahoo session persistence.

## Cutover Prep

1. Run `./scripts/export-js-archive.ps1` or `./scripts/export-js-archive.sh`.
2. Confirm `output/Dynamo-JS` contains:
   - `src/`
   - `dashboard/`
   - `bot.js`
   - `config.js`
   - `package.json`
   - `package-lock.json`
   - `docs/commands`
3. Confirm the staged archive root README clearly says the archive is read-only and that active development remains in the Rust mainline repository.
