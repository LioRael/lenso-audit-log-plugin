# Plan 001: Validate bounded audit events before persistence

> Drift check: `git diff --stat f1a81c9..HEAD -- crates/lenso-audit-log-postgres-plugin crates/lenso-capability-audit-log`.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED
- **Depends on**: none
- **Category**: security
- **Planned at**: commit `f1a81c9`, 2026-08-30

## Why this matters

Audit input must be rejected before persistence when its strings or recursive metadata
exceed the public Contract. Size admission should not allocate a second metadata-sized
buffer, because rejected inputs are exactly the path where allocation must stay bounded.

## Current state

The Plugin-first migration on `main` introduced Contract-aligned limits for envelope
strings, metadata depth, container size, node count, scalar length, encoded bytes, and
portable JSON numbers. It also revalidates legacy rows before projecting them.

## Scope

In scope: preserve those limits on the Plugin write path, count exact encoded metadata
bytes without materializing a second buffer, and test the byte boundary. Out of scope:
retroactively rewriting legacy rows or imposing producer-specific schemas without a
public schema-registration contract.

## Steps

1. Keep recursive structural and portable-JSON validation before `NewAuditEvent` is
   constructed and before the repository can insert it.
2. Replace temporary metadata serialization with a bounded writer that stops at the
   Contract's exact 64 KiB encoded limit.
3. Cover the exact accepted byte boundary and the first rejected byte alongside the
   existing depth, collection, portability, and redaction cases.

## Verification

- `lenso-cargo test --workspace --all-targets --all-features` -> all pass.
- `lenso-cargo check --workspace --all-targets --all-features` -> exit 0.
- `lenso-cargo clippy --workspace --all-targets --all-features -- -D warnings` -> exit 0.
- `git diff --check` -> no output.

## STOP conditions

Stop if a stricter producer-specific allowlist would reject generic Plugin callers;
that policy requires an explicit public schema-registration contract first.
