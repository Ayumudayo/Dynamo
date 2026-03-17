# Workspace Architecture

This note defines the intended ownership boundaries for the current Rust workspace. It is the source of truth for stabilization and future review, and it should stay aligned with the actual crate graph.

## Ownership

| Area | Owning crate |
| --- | --- |
| Module trait, manifests, command/settings catalog descriptors | `dynamo-module-kit` |
| Deployment and guild configuration state | `dynamo-settings` |
| Repository traits for persisted state | `dynamo-repositories` |
| Stock quote service contract | `dynamo-service-stock` |
| Exchange-rate service contract | `dynamo-service-exchange` |
| Persistence registry and persistence-side helper methods | `dynamo-persistence-api` |
| Runtime service registry | `dynamo-services-api` |
| Shared runtime state, poise context alias, and top-level error type | `dynamo-runtime-api` |
| Environment-driven runtime configuration | `dynamo-config` |
| Pure enablement resolution | `dynamo-enablement` |
| Context-aware access checks | `dynamo-access` |
| Module registry, command catalog construction, intent aggregation | `dynamo-registry` |
| Dashboard audit log and command sync state | `dynamo-ops` |
| Startup reporting and rendering | `dynamo-observability` |
| Currency, stock, giveaway, invite, moderation, stats, suggestion data models | `dynamo-domain-*` |
| MongoDB repository implementations | `dynamo-persistence-mongo` |
| External data providers | `crates/providers/*` |
| First-party runtime modules | `crates/modules/*` |

## Dependency Rules

- Modules depend on `dynamo-module-kit`, `dynamo-runtime-api`, and only the domain/settings/service crates they actually use.
- Modules should depend on `dynamo-access` only when they need context-aware module or command guard helpers.
- Modules should not depend on `dynamo-persistence-api`, `dynamo-services-api`, or `dynamo-ops` directly unless a concrete feature requires it.
- Providers depend on their specific domain crate plus the repository or service contract they implement.
- `dynamo-persistence-mongo` depends on `dynamo-repositories`, `dynamo-settings`, `dynamo-ops`, and the domain crates it persists.
- `dynamo-enablement` owns pure effective-state calculation only. It must stay free of runtime state and persistence access.
- `dynamo-access` is the only crate that combines `dynamo-enablement` with `dynamo-runtime-api` to produce guard helpers.
- `dynamo-runtime-api` is intentionally limited to `AppState`, `Context`, and `Error`. It should not absorb persistence or service implementation details again.
- New cross-cutting helper code should be placed in an existing focused crate whenever possible. Avoid introducing new omnibus shared crates.

## Process-Level Usage

- `dynamo-app` assembles the runtime pieces and wires modules, providers, persistence, and catalogs together.
- `dynamo-bot` depends on `config`, `access`, `registry`, `persistence-api`, `services-api`, `runtime-api`, `ops`, and `observability`.
- `dynamo-dashboard` depends on `enablement`, `persistence-api`, `runtime-api`, `settings`, `ops`, and `observability`.
- `dynamo-bootstrap` depends only on Mongo bootstrap and observability.

## Merge Stack

Current merge order for the refactor stack:

1. `chore/post-core-split-followup`
2. `chore/runtime-api-fanout-reduction`
3. `chore/persistence-services-api-split`

Each branch should pass the same baseline validation before merge:

- `cargo fmt --all --check`
- `cargo check`
- `cargo test --workspace`
- `bash scripts/check-workspace-structure.sh`
