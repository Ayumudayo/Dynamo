# JS Pattern Audit For Rust Mainline Cutover

This document captures the legacy JS patterns that are still worth preserving while the current repository becomes the Rust mainline and the JS runtime is split into the `Dynamo-JS` archive repository.

## Runtime and Boot Flow

| Area | Legacy JS reference | What is worth preserving | Rust status |
|---|---|---|---|
| Boot order | [`Dynamo-JS/bot.js`](https://github.com/Ayumudayo/Dynamo-JS/blob/main/bot.js) | Configuration validation before startup, dashboard-before-bot launch ordering, DB-first initialization | Mostly preserved |
| Dynamic loading | [`Dynamo-JS/bot.js`](https://github.com/Ayumudayo/Dynamo-JS/blob/main/bot.js) | Clear grouping of commands, contexts, and events as separate runtime concerns | Preserved with static Rust registry |
| Runtime helpers | legacy extenders and handlers | Practical runtime ergonomics for replies, guild lookups, and workflow helpers | Replaced by typed helpers; preserve UX parity, not JS monkey patches |

## Dashboard and Settings UX

| Area | Legacy JS reference | What is worth preserving | Rust status |
|---|---|---|---|
| Guild access rules | [`Dynamo-JS/dashboard/routes/guild-manager.js`](https://github.com/Ayumudayo/Dynamo-JS/blob/main/dashboard/routes/guild-manager.js) | Only manageable guilds are shown and editable | Preserved via OAuth + guild permissions |
| Settings grouping | [`Dynamo-JS/dashboard/routes/guild-manager.js`](https://github.com/Ayumudayo/Dynamo-JS/blob/main/dashboard/routes/guild-manager.js) | Group settings by operator task: basic, greeting, moderation, stats, ticket | Partially preserved; keep refining grouping language |
| Save flows | [`Dynamo-JS/dashboard/routes/guild-manager.js`](https://github.com/Ayumudayo/Dynamo-JS/blob/main/dashboard/routes/guild-manager.js) | Fast save-and-refresh behavior with obvious success path | Preserved via modal save flow and audit logs |
| Selector UX | JS dashboard guild manager and listing pages | Fast server selection before entering detailed controls | Preserved in Rust OAuth dashboard |

## Feature Patterns Worth Retaining

| Area | Legacy JS reference | What is worth preserving | Rust status |
|---|---|---|---|
| Stock UX | [`Dynamo-JS/src/helpers/StockService.js`](https://github.com/Ayumudayo/Dynamo-JS/blob/main/src/helpers/StockService.js) | Compact market embeds, refresh sessions, ETF preset flow | Preserved and improved |
| Currency UX | [`Dynamo-JS/src/commands/currency/exchange.js`](https://github.com/Ayumudayo/Dynamo-JS/blob/main/src/commands/currency/exchange.js), [`Dynamo-JS/src/commands/currency/rate.js`](https://github.com/Ayumudayo/Dynamo-JS/blob/main/src/commands/currency/rate.js) | Sensible defaults, compact result boards, currency choice set | Preserved and improved |
| Ticket workflow | [`Dynamo-JS/src/commands/ticket/ticket.js`](https://github.com/Ayumudayo/Dynamo-JS/blob/main/src/commands/ticket/ticket.js) | Interactive setup, add/remove participants, close/closeall/log/limit workflows | Preserved |
| Giveaway manager split | [`Dynamo-JS/src/handlers/giveaway.js`](https://github.com/Ayumudayo/Dynamo-JS/blob/main/src/handlers/giveaway.js) | Separate persistence-backed giveaway runtime and command surface | Preserved; now promoted to core |
| Dashboard-driven settings | JS dashboard routes | Operators should configure most guild behavior without terminal edits | Preserved |

## Gaps To Keep Watching Before JS Removal

- Deployment and guild pages should stay denser than the legacy JS dashboard without losing scannability.
- Rust command descriptions should remain explicit and human-written.
- Startup reports should stay compact while still surfacing service wiring and command counts.
- Any JS-only operational convenience discovered later should be documented here before the archive split is finalized.
