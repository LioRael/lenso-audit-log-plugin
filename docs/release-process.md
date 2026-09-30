# Release process

The legacy Audit Log release line remains available through its historical
crate versions and tags. The current workspace has four public-package
identities:

1. `lenso-capability-audit-log`;
2. `lenso-audit-log-core`;
3. `lenso-audit-log-d1-plugin`; and
4. `lenso-audit-log-postgres-plugin`.

`lenso-audit-log-agent-tools-plugin` remains private. This package policy is
unchanged by source delivery or the release workflow.

Publication is manual and runs only from a clean `main` checkout through
`.github/workflows/release-plz.yml`. Pushes to `main` neither run this workflow
nor create a release PR. Candidate CI still requires independent normalized
package verification; no archive check is removed or weakened. A manual dry-run
has read-only repository permission and explicitly selects `command: release`
with `dry_run: true`, so it does not invoke the default release-PR command.
See the [Release-plz input contract](https://release-plz.dev/docs/github/input).

A live dispatch requires separate explicit publication approval, `live=true`,
`confirm=publish`, and the `main` ref. Run the dry-run dispatch first and verify
its exact checked source SHA and artifacts before authorizing live publication.
Landing source or a successful CI/dry-run does not authorize publication.

Before first publication of each new crate name:

1. pass generated-contract, Rust, PostgreSQL, boundary, and independent package
   verification;
2. allocate the crate name once using crates.io's authenticated initial-publish
   flow, because OIDC Trusted Publishing cannot create a new crate name;
3. configure the crate's Trusted Publisher with owner `LioRael`, repository
   `lenso-audit-log-plugin`, workflow `release-plz.yml`, and no environment;
4. confirm dependency packages are public before their dependent implementations;
   and
5. run the live workflow only after every selected public crate has its matching
   publisher and the exact dependency/archive gates pass.

The workflow has no registry-token fallback. The live job obtains a short-lived
crates.io credential through GitHub OIDC and has only the required
`id-token: write` publication authority. Never use `--no-verify`, a long-lived
registry token, or a Git dependency as a publication shortcut.
