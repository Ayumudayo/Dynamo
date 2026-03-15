# Dev Smoke Checklist

Run this checklist after major module, persistence, or dashboard changes.

## Bootstrap

1. Run `cargo run -p dynamo-bootstrap`.
2. Confirm the `dynamo-rs` database exists under `dynamo-cluster`.
3. Confirm collections exist for:
   - `deployment_settings`
   - `guild_settings`
   - `provider_state`
   - `suggestions`
   - `members`
   - `member-stats`
   - `mod-logs`

## Startup

1. Start `cargo run -p dynamo-dashboard`.
2. Start `cargo run -p dynamo-bot`.
3. Confirm the dashboard root and deployment page load.
4. Confirm commands register in the development guild when `DISCORD_REGISTER_GLOBALLY=false`.

## Dashboard

1. Open `/deployment`.
2. Toggle one module off and on again.
3. Open `/guild/<guild_id>`.
4. Change one structured field and confirm it persists after reload.
5. Save one module configuration through the advanced JSON editor.
6. Disable one leaf command in deployment settings and confirm it disappears or becomes unavailable after the next sync cycle.
7. Disable one leaf command in guild settings and confirm the guild-specific command set updates after the next sync cycle.

## Core Commands

1. Run `/ping`.
2. Run `/wtinv`.
3. Run `/maint`.
4. Run `/pll`.
5. Run `/stock NVDA`.
6. Run `/etf`.

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
