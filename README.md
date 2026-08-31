# Lenso Audit Log Plugin

This repository owns removable, PostgreSQL-backed Audit Log behavior for Lenso
applications. It is business evidence written by Plugins; it is not Runtime
Story, telemetry, transport history, or a Kernel execution log.

The workspace publishes two packages:

- `lenso-capability-audit-log`: the portable `lenso.audit-log@1` Contract and
  generated Rust Client/Provider binding.
- `lenso-audit-log-postgres-plugin`: the native Plugin implementation with
  Plugin ID `lenso.audit-log.postgres`.

## Capability

`lenso.audit-log@1` is a request Capability with three terminal operations:

- `append_event` durably appends and returns the stored event.
- `list_events` reads a filtered, cursor-paginated event page.
- `get_event` reads one event by ID.

Append is a request instead of an Event endpoint because callers need durable
success or a classified Domain/Runtime failure. The Contract is portable and
cross-lane transferable. Generated bindings preserve unknown future Domain
codes and redact metadata in `Debug` output.

The append request has no source-identity field. The provider derives the
stored and returned `source_instance` from `Ctx::caller_instance`, so an
authorized writer cannot attribute an event to another Plugin Instance.

Binding the Capability is necessary but not sufficient authorization. App
Composition also supplies exact `writer_instances` and `reader_instances`.
Writer-only callers receive Domain `unauthorized` from both read operations;
reader-only callers receive Domain `unauthorized` from append.

## Storage and setup

The Plugin owns the fixed `audit_log` schema and the append-only
`audit_log.events` table. Its migration SQL is byte-for-byte unchanged from the
pre-vNext package.

Setup and upgrade are explicit operator actions:

```rust
use lenso_audit_log_postgres_plugin::AuditLogOperator;

AuditLogOperator::setup(&database_url).await?;
AuditLogOperator::upgrade(&database_url).await?;
```

Deployments created by the legacy 0.1.5 package use the same table bytes but
only have `platform.schema_migrations` evidence. They require a separate,
one-time operator action:

```rust
AuditLogOperator::adopt_legacy(&database_url).await?;
```

Adoption is never attempted by Plugin startup. Run it only in a database
maintenance window after stopping direct DDL by database-owner sessions. The
workflow takes the database-wide Lenso maintenance advisory lock, then the same
schema advisory lock as `lenso-postgres-kit`, and locks both persistent tables.
Every DDL actor participating in the window must first take the same
transaction-level maintenance lock:

```sql
select pg_advisory_xact_lock(
  hashtextextended(current_database() || ':lenso-maintenance', 0)
);
```

PostgreSQL has no SQL-level `LOCK SCHEMA` command. An uncoordinated owner can
therefore race any schema inspection; that actor is outside this operator
protocol and must be quiesced operationally. Within the protocol, adoption
serializes DDL, then verifies schema, table, column, and generated-type owners
and ACLs; default privileges; exact migration evidence; columns and collations;
defaults; constraints; every material index flag; comments; security labels;
and absence of extra objects. Only an exact unmanaged legacy schema receives
the Plugin ledger and current migration checksum; event rows are not updated.
Partial, modified, over-granted, extra-object, and already-managed states fail
closed.

Plugin `prepare` resolves `database_url_secret` through `lenso.secrets@1` and
only verifies the operator-managed schema; it never creates or upgrades it. It
fails closed when any PostgreSQL publication includes all database tables or
all tables in `audit_log`, including implicit exposure not represented in
`pg_publication_rel`. Legacy adoption applies the same publication check before
accepting the catalog fingerprint and also rejects schema-wide publication of
the `platform` migration provenance ledger.
Metadata is limited to 64 KiB, must satisfy portable JSON-number rules, and has
sensitive keys recursively replaced with `[redacted]` before persistence. The
wire policy also bounds each object/array to 1,024 items, property names to 256
characters, total nodes to 16,384, and nesting to 32 levels. Every stored text
field and projected metadata value is revalidated against the Capability output
bounds, so an oversized owner-written or legacy row fails closed before any
response is encoded.
The v1 append Contract continues to accept metadata objects. Historical rows
may contain any JSON value. Portable objects without reserved envelope keys are
returned unchanged. An array, scalar, null, or an object containing an envelope
key is returned losslessly as `{"_lenso_legacy_value": <original-value>}`. If
the value contains an integer outside the portable JSON safe range, the exact
JSON is preserved as text under
`{"_lenso_legacy_portable_json": "<original-json>"}` rather than rounded.
Consumers can call `lenso_capability_audit_log::recover_legacy_metadata` to
recognize and recover either envelope. Thus native and cross-lane calls expose
the same value without rewriting historical rows.

See [the Plugin card](docs/plugin-card.md) for configuration, ownership, and
the deletion boundary.

Publication and crates.io Trusted Publisher setup are documented in
[the release process](docs/release-process.md).

## vNext compatibility boundary

This is a deliberate breaking migration. The old linked package and Rust call
surface are not retained as a compatibility shim. Applications on the v0.3
legacy lane must keep using their historical dependency until they migrate to
Plugin selection, Capability requirements, and generated Clients.

There is no Plugin manifest admin/Console hook. A product that needs an audit
viewer binds `lenso.audit-log@1` from its own UI or Agent-facing Plugin and calls
the generated read operations.

## Development

From this repository inside the Lenso workspace:

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

PostgreSQL acceptance additionally requires `LENSO_POSTGRES_TEST_URL` naming a
database whose name contains `test`, then runs with `--features
postgres-acceptance`. The test owns and recreates the `audit_log` and `platform`
schemas in that guarded test database.
