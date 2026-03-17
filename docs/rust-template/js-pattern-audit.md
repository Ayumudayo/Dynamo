# JS Pattern Audit For Rust Cutover

This document captures the parts of the legacy JS bot and dashboard that are still worth preserving while the Rust-only public template is prepared.

## Runtime and Boot Flow

| Area | Legacy JS reference | What is worth preserving | Rust status |
|---|---|---|---|
| Boot order | [bot.js](/E:/Repos/MyRepos/DiscordBots/Dynamo/bot.js) | Configuration validation before startup, dashboard-before-bot launch ordering, DB-first initialization | Mostly preserved |
| Dynamic loading | [bot.js](/E:/Repos/MyRepos/DiscordBots/Dynamo/bot.js) | Clear grouping of commands, contexts, and events as separate runtime concerns | Preserved with static Rust registry |
| Runtime helpers | legacy extenders and handlers | Practical runtime ergonomics for replies, guild lookups, and workflow helpers | Replaced by typed helpers; keep UX parity, not JS monkey patches |

## Dashboard and Settings UX

| Area | Legacy JS reference | What is worth preserving | Rust status |
|---|---|---|---|
| Guild access rules | [dashboard/routes/guild-manager.js](/E:/Repos/MyRepos/DiscordBots/Dynamo/dashboard/routes/guild-manager.js) | Only manageable guilds are shown and editable | Preserved via OAuth + guild permissions |
| Settings grouping | [dashboard/routes/guild-manager.js](/E:/Repos/MyRepos/DiscordBots/Dynamo/dashboard/routes/guild-manager.js) | Group settings by operator task: basic, greeting, moderation, stats, ticket | Partially preserved; continue improving grouping language |
| Save flows | [dashboard/routes/guild-manager.js](/E:/Repos/MyRepos/DiscordBots/Dynamo/dashboard/routes/guild-manager.js) | Fast save-and-refresh behavior with obvious success path | Preserved via modal save flow and audit logs |
| Selector UX | JS dashboard guild manager and listing pages | Fast server selection before entering detailed controls | Preserved in Rust OAuth dashboard |

## Feature Patterns Worth Retaining

| Area | Legacy JS reference | What is worth preserving | Rust status |
|---|---|---|---|
| Stock UX | [src/helpers/StockService.js](/E:/Repos/MyRepos/DiscordBots/Dynamo/src/helpers/StockService.js) | Compact market embeds, refresh sessions, ETF preset flow | Preserved and improved |
| Currency UX | [src/commands/currency/exchange.js](/E:/Repos/MyRepos/DiscordBots/Dynamo/src/commands/currency/exchange.js), [src/commands/currency/rate.js](/E:/Repos/MyRepos/DiscordBots/Dynamo/src/commands/currency/rate.js) | Sensible defaults, compact result boards, currency choice set | Preserved and improved |
| Ticket workflow | [src/commands/ticket/ticket.js](/E:/Repos/MyRepos/DiscordBots/Dynamo/src/commands/ticket/ticket.js) | Interactive setup, add/remove participants, close/closeall/log/limit workflows | Preserved |
| Giveaway manager split | [src/handlers/giveaway.js](/E:/Repos/MyRepos/DiscordBots/Dynamo/src/handlers/giveaway.js) | Separate persistence-backed giveaway runtime and command surface | Preserved; promote to core |
| Dashboard-driven settings | JS dashboard routes | Operators should configure most guild behavior without terminal edits | Preserved |

## Gaps To Keep Watching Before New Repo Export

- Deployment and guild pages should stay denser than the current JS dashboard, but not lose scannability.
- Rust command descriptions should continue to be explicit and human-written.
- Startup reports should remain compact while still surfacing service wiring and command counts.
- Any JS-only operational convenience discovered later should be documented here before the fresh repo is cut.
