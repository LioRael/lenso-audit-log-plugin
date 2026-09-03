# PostgreSQL Audit Log Plugin card

## Job and first observable result

A business Plugin records a security- or workflow-relevant fact and receives
the durable event, including its generated ID and storage timestamps. An
authorized product surface can then list or retrieve the same evidence through
the same Capability Contract.

## Package identity

- Package: `lenso-audit-log-postgres-plugin`
- Plugin ID: `lenso.audit-log.postgres`
- Plugin Root slot: `audit`
- Provides: `lenso.audit-log@1` version `1.0.0`
- Requires: `lenso.secrets@1`, cardinality one
- Execution: native implementation of a portable, cross-lane Contract

## Capability role

The cohesive Audit Log role owns three request operations:

| Operation | Terminal success | Domain errors |
| --- | --- | --- |
| `append_event` | The durable, redacted Audit Event | `invalid_event`, `unauthorized` |
| `list_events` | A filtered page plus optional next cursor | `invalid_query`, `unauthorized` |
| `get_event` | One Audit Event | `invalid_id`, `not_found`, `unauthorized` |

Storage, dependency, protocol, and unavailable-generation failures remain
Runtime failures. Generated bindings preserve unknown future Domain codes and
their payload/extra fields.

Append deliberately accepts no caller-supplied source identity. The provider
derives `source_instance` from the invocation caller and writes it to the
historical PostgreSQL `module_name` column. A writer therefore cannot spoof a
different Plugin Instance while the original migration bytes remain unchanged.

## Immutable Instance configuration

```json
{
  "database_url_secret": "audit/database-url",
  "writer_instances": ["support"],
  "reader_instances": ["audit-viewer"]
}
```

Every list is non-empty, duplicate-free, and contains exact Instance IDs. A
Capability binding does not grant all operations: append checks
`writer_instances`; list and get check `reader_instances`. Authorization runs
before request validation and storage, preventing a caller from probing either.

## Data boundary

This Plugin exclusively owns the fixed `audit_log` PostgreSQL schema and its
append-only `events` table. It preserves the original migration bytes (SHA-256
`f6f6b84e1d184eaaac746e68c9cfa65ff864d5439c8e50668a381a23cb984199`).
Generic actor, scope, resource, outcome, severity, reason, correlation, and
story fields remain intact; the Contract exposes stored provenance as
`source_instance`.

Before storage, metadata:

- must encode to at most 64 KiB;
- may contain at most 1,024 entries per object/array, 16,384 total nodes, and 32
  levels of nesting; property names are limited to 256 characters;
- must satisfy the portable JSON-number boundary;
- recursively replaces password, token, secret, private-key, API-key,
  authorization, and cookie-like values with `[redacted]`.

The append Contract keeps its historical object-shaped metadata boundary.
Portable legacy objects without a reserved envelope key project unchanged.
Legacy array, scalar, null, and reserved-key metadata project losslessly under
the fixed `_lenso_legacy_value` key. Metadata containing an integer outside the
portable JSON safe range is preserved as exact JSON text under
`_lenso_legacy_portable_json`; it is never rounded. The Capability package's
`recover_legacy_metadata` helper recognizes and recovers both forms. Adoption
never rewrites those rows.
All legacy text fields and the final metadata envelope are revalidated at the
`StoredEvent`-to-Capability projection boundary. Oversized owner-written rows
fail closed as a Runtime Failure and cannot produce an unbounded response.

`AuditLogOperator::setup`, `AuditLogOperator::upgrade`, and the one-time
`AuditLogOperator::adopt_legacy` workflow are the only schema mutation paths.
Adoption exists solely for exact legacy 0.1.5 deployments whose migration is
recorded in `platform.schema_migrations` but whose Plugin ledger is absent. It
requires a database maintenance window in which direct owner DDL is stopped and
every participating DDL actor first takes the database-wide Lenso maintenance
advisory lock (`hashtextextended(current_database() || ':lenso-maintenance',
0)`). PostgreSQL exposes no SQL-level schema lock, so uncoordinated owner DDL is
explicitly outside the protocol rather than falsely claimed to be excluded. In
one transaction adoption takes that maintenance lock and the PostgreSQL kit's
schema lock, locks both persistent tables, verifies
schema/table/column/generated-type owners and ACLs plus default privileges,
verifies the exact legacy ledger row and complete catalog fingerprint
(including collations, comments, security labels, and index state), then
creates the Plugin ledger with the current migration checksum. Missing,
partial, tampered, over-granted, extra-object, and already-managed states are
rejected without a ledger or data change.

Plugin `prepare` resolves the database URL via Secrets and only verifies the
managed schema; it never sets up, upgrades, or adopts storage. Managed prepare
and legacy adoption reject database-wide and `audit_log` schema-wide PostgreSQL
publications; adoption also rejects publication of the legacy `platform`
provenance schema. Implicit publication exposure therefore cannot bypass the
relation membership fingerprint. `deactivate` closes the owned pool after
removing the prepared generation from live state.

## Product and admin boundary

The Plugin manifest has no admin or Console contribution. Audit browsing is a
business Capability use case owned by the target UI or Agent-facing Plugin. It
binds `lenso.audit-log@1` and uses the generated Client like every other
consumer.

The private `lenso.audit-log.agent-tools` adapter exposes only `list_events`
and `get_event` as parallel-safe Console Agent Tools. It forwards the invocation
context unchanged, so the PostgreSQL provider's exact `reader_instances`
admission remains authoritative. It does not expose `append_event`, persist
state, inspect private tables, or create a second audit policy.

## Deletion boundary

To remove Audit Log behavior from an App:

1. Remove the `lenso.audit-log.postgres` Instance selection.
2. Remove bindings and requirements for `lenso.audit-log@1`.
3. Remove the operator-owned `audit_log` schema only under the application's
   explicit data-retention policy.

No Kernel, Runtime Driver, or Host source branch is changed. Keeping the package
linked but unselected is inert; a composition test starts successfully without
the Audit Log Instance or any Audit capability binding.

## vNext break

The historical direct Rust writer/read/admin surface is intentionally absent.
Legacy-lane applications must migrate their composition and consumers to the
Capability Contract; this repository does not ship a compatibility package or
package-name shim.
