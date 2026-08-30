#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

forbidden_pattern='HostLinkedModule|ModuleManifest|lenso-platform-|platform_(core|module|testing)|lenso-module-audit-log|crates/audit-log'
scan_paths=(Cargo.toml README.md crates docs/plugin-card.md .github)

if rg --line-number --hidden --glob '!generated.rs' "$forbidden_pattern" "${scan_paths[@]}"; then
  echo "legacy production or current-documentation boundary detected" >&2
  exit 1
fi

if rg --files crates/audit-log >/dev/null 2>&1; then
  echo "legacy audit-log crate still contains tracked source" >&2
  exit 1
fi

rg --quiet 'plugin-id = "lenso.audit-log.postgres"' crates/lenso-audit-log-postgres-plugin/Cargo.toml
rg --quiet '"id": "lenso.audit-log@1"' crates/lenso-capability-audit-log/capability.json
rg --quiet '"lenso.audit-log@1"' crates/lenso-capability-audit-log/src/generated.rs
rg --quiet 'AuditLogOperator::adopt_legacy' README.md docs/plugin-card.md
rg --quiet '_lenso_legacy_value' README.md docs/plugin-card.md
for portable_boundary in README.md docs/plugin-card.md crates/lenso-capability-audit-log/src/lib.rs; do
  rg --quiet '_lenso_legacy_portable_json' "$portable_boundary"
  rg --quiet 'recover_legacy_metadata' "$portable_boundary"
done
for maintenance_boundary in README.md docs/plugin-card.md crates/lenso-audit-log-postgres-plugin/src/operator.rs; do
  rg --quiet "':lenso-maintenance'" "$maintenance_boundary"
done
rg --quiet '"maxItems": 200' crates/lenso-capability-audit-log/schemas/list-events-response.schema.json
rg --quiet '97ff73638d1fca098034c443539d44d95062eea4' Cargo.toml
rg --quiet 'Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6' .github/workflows/ci.yml

expected_migration_hash="f6f6b84e1d184eaaac746e68c9cfa65ff864d5439c8e50668a381a23cb984199"
actual_migration_hash="$(shasum -a 256 crates/lenso-audit-log-postgres-plugin/migrations/001_create_audit_log_schema.sql | awk '{print $1}')"
if [[ "$actual_migration_hash" != "$expected_migration_hash" ]]; then
  echo "Audit Log migration bytes changed: expected $expected_migration_hash, got $actual_migration_hash" >&2
  exit 1
fi
