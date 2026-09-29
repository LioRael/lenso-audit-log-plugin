# Management audit evidence

The `lenso.audit-log@1` 1.1 contract adds an optional `idempotency_key` to the
existing append operation. Exact caller Instance plus key selects a stable
SHA-256 event ID. Replaying identical sanitized evidence returns the original
row; changing intent returns `idempotency_conflict`. Another caller using the
same key receives an independent event. The existing append-only schema and
legacy adoption bytes remain unchanged. Events without a key keep their
previous fresh-ID behavior. Event time comparisons use PostgreSQL microsecond
precision, including concurrent replay.

A producer keeps one stable key per evidence event. An attempt, an ambiguous
result and later reconciliation are separate immutable evidence events, rather
than an update of the attempt row. Only the actual domain owner may claim a
business commit. Management records its admission and dispatch observations.
Input digests stay in the controlled operation journal; metadata keeps only
allowlisted fields. Existing recursive redaction and size limits remain active.

The optional `reader_scopes` configuration maps an exact allowed reader
Instance to opaque `{kind,id}` ceilings. Scoped readers must supply an admitted
scope to `list_events`; the provider rejects unscoped/cross-scope reads before
querying and checks `get_event` before projecting a row. A denied guessed event
ID returns `not_found`. Trusted legacy local readers can explicitly retain the
existing unscoped behavior. Management separately checks each authenticated
person or machine's current scope permission on every call. A provider ceiling
is not a substitute for that check. Remote/model profiles must use the scoped
projection and must not expose append.

Business outboxes, durable attempt admission, backpressure and dispatch failure
policy remain with their owning Plugin. This package does not combine writes in
another database into a transaction or silently discard an audit failure.
The native PostgreSQL implementation is tested; Workers PG/Hyperdrive and any
remote audit transport remain unqualified.

## Verification

```sh
cargo fmt --all -- --check
lenso-contract-codegen workspace check --manifest-path Cargo.toml
LENSO_POSTGRES_TEST_URL=postgresql://.../lenso_audit_log_test_security \
  cargo test --locked --workspace --features postgres-acceptance
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

The PostgreSQL acceptance database is disposable and owns the audit/platform
schemas. Candidate CI runs once for the immutable `candidate/**` SHA; landing
and package publication remain separate actions.
