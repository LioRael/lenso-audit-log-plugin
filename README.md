# Lenso Audit Log Module

First-party Lenso audit-log module.

This repository is intentionally separate from `lenso` core. The module is a
reusable, installable business infrastructure module for recording module-owned
audit events. It is not a replacement for Runtime Story, telemetry, remote call
history, or platform execution logs.

## What It Provides

- Append-only audit event storage in `audit_log.events`.
- Generic actor, scope, resource, outcome, severity, reason, metadata, and
  Runtime Story correlation fields.
- Linked Rust writer APIs for first-party modules.
- Schema-admin read surface through `audit_log.events.read`.

## Install In A Lenso Host

```rust
use audit_log::module as audit_log_module;
use lenso::host::prelude::*;

pub fn host_composition() -> HostComposition {
    HostBuilder::new()
        .linked_module(audit_log_module::linked_module())
        .build()
}
```

`audit-log` is independent from `organization`. Organization events use generic
`scope_type` and `scope_id` fields, so other modules can record workspace,
project, account, tenant, or custom scoped activity without depending on
organization tables.

## Design

The implementation follows the repository design spec at
`docs/superpowers/specs/2026-07-04-lenso-audit-log-module-design.md`.

## Development

```sh
cargo fmt --all --check
cargo test --locked -p lenso-module-audit-log
cargo clippy --locked -p lenso-module-audit-log --all-targets -- -D warnings
```

DB-backed tests use `DATABASE_URL` when it is set. Without it, the repository
still runs the pure Rust coverage and skips the Postgres integration cases.

## Release

Release-plz creates a release pull request from changes merged to `main`. After
that pull request is merged, the same workflow publishes the crate through
crates.io Trusted Publishing and creates the GitHub Release. There is no manual
publisher workflow or repository registry token in the normal release path.
