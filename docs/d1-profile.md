# D1 Audit profile

The `lenso.audit-log.d1` Plugin supplies the existing `lenso.audit-log@1` Role.
Its configuration contains exact writer/reader Instance keys and optional exact
reader scope ceilings. The required private `store` facility is selected by the
Host for one Plugin Instance. Its configuration is `{ "profile": "workers-d1" }`;
the Host passes one D1 binding and one event scope, never the complete environment.

The private JavaScript adapter starts a fresh `withSession("first-primary")`
for every finite call. Its append batch uses one atomic session; a later
read starts from the current primary rather than an earlier event bookmark. The Owner `setup(database)` action is explicit.
Runtime preparation only checks the owned schema version. Event validation,
metadata redaction, caller-derived source attribution, bounded filters and
projection are shared with the PostgreSQL implementation in `lenso-audit-log-core`.
The finite store operations are readiness, append, get and list. SQL stays private.

Append inserts once and reads the stored result in one D1 batch. A replay with
changed content is rejected by shared Rust comparison. An exception, decode
failure or closed event after submission is Runtime unavailable: the append may
have committed. The consumer retains its stable delivery key and reconciles via
the original event; the store does not replay a mutation or invent a receipt.
Event IDs without a stable key use Host-owned cryptographic UUID generation.

The Node SQLite tests qualify finite SQL and persistence semantics only. They do
not establish actual Workers execution or the complete Management App. The new
profile needs an actual workerd/D1 and ordinary Source App qualification receipt
before a Workers support claim. Native source checkpoints remain separate.
