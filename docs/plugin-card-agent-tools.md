# Audit Log Agent Tools Plugin card

## Owner and deletion boundary

`lenso-audit-log-agent-tools-plugin` is a private, stateless adapter. Removing
it removes only the Console Agent's Audit Log read Tools. Durable Audit Events,
reader and writer admission, PostgreSQL lifecycle, and retention remain owned
by the Audit Log provider and App composition.

## Roles

- Provides `lenso.agent.tool-provider@2` in the `tool-providers` root slot.
- Requires exactly one `lenso.audit-log@1` provider.
- Exposes `audit_log_list_events` and `audit_log_get_event` as parallel-safe
  reads using the existing portable request schemas.

## Authority boundary

The adapter forwards the invocation context unchanged and maps existing Domain
Errors into Agent Tool errors. The bound provider retains exact reader
admission, query validation, redaction, pagination, storage, and all durable
state.

The adapter does not expose `append_event`. A Console Agent cannot manufacture
business evidence or choose a source identity; evidence remains attributable
to the business Plugin Instance that performed the audited work.
