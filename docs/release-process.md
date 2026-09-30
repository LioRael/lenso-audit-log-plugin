# Release process

The changed public Audit Log cohort is limited to:

- `lenso-capability-audit-log` 0.1.1
- `lenso-audit-log-core` 0.1.0
- `lenso-audit-log-postgres-plugin` 0.1.1
- `lenso-audit-log-d1-plugin` 0.1.0

The historical Capability and PostgreSQL 0.1.0 archives remain immutable.
Core and D1 are new crate names. Agent Tools remains `publish = false` and is
outside every release set. Source delivery and this workflow do not alter
package publication flags.

## Manual qualification and publication

`.github/workflows/release-plz.yml` has only `workflow_dispatch`; Main pushes
cannot publish packages or open release PRs. The default `dry-run` job has read
permissions and cannot obtain an OIDC publishing token. It explicitly runs
`command: release` with `dry_run: true`; it never invokes `release-pr`.

Dispatch from Main with the full `source_sha`, exact `candidate_run_id` and
`candidate_attempt`, and a JSON array of `package_name`/`version` objects. The
set must equal the registry-derived pending subset of the four versions above.
This permits a separately authorized new-name bootstrap followed by a reviewed
remainder, while rejecting extra packages, wrong versions, duplicates and
unknown registry responses.

The gate verifies the clean checkout against fresh remote Main and requires
the exact `candidate/**` push CI workflow, SHA and attempt, with one successful
`quality` job. Normalized Cargo verification packages only the pending changed
versions. Archive inspection checks identity, clean VCS SHA and both manifests,
and records SHA256 digests. Candidate CI's existing archive, generated-contract,
Native PostgreSQL and Wasm gates remain required without weakening.

Live mode requires separate human authorization and `confirmation=publish`.
The live job repeats the source, candidate, pending-set and archive checks
immediately before pinned release-plz. `.github/release-owner.toml` processes
only this four-package allowlist. The postcondition reconciles exact action
records, Primary versions, source-bound tags and GitHub releases, even after
partial failure. Inspect an unknown or partial publication before any further
dispatch. Landing source or passing a dry-run does not authorize publication.

## New names and Trusted Publishers

Current [crates.io documentation](https://crates.io/docs/trusted-publishing)
requires a crate's initial publication before configuring its Trusted Publisher.
The current registry implementation also rejects new-name creation by a
Trusted Publishing token. This workflow deliberately has no initial-token
fallback and fails live mode when a selected crate name does not exist.

A separately authorized first-name publication must use the reviewed exact
source and normalized archive after its dependencies are visible. Then
configure and verify each crate's Trusted Publisher with owner `LioRael`,
repository `lenso-audit-log-plugin`, workflow `release-plz.yml`, and no environment.
Do not enable a token fallback or change publisher settings as part of source
landing. The four-package allowlist remains fixed after bootstrap; recompute
the exact registry-derived pending subset before any later authorized dispatch.

Publish the changed Capability before Core, then PostgreSQL and D1. A local
Git-patched source proof is separate from normalized registry verification,
registry visibility and actual profile qualification. Never use `--no-verify`
or republish modified bytes under an already registered package version.

See the [Release-plz input contract](https://release-plz.dev/docs/github/input).
