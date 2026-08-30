# Release process

The legacy Audit Log release line remains available through its historical
crate versions and tags. The default branch now owns two vNext packages with
distinct identities:

1. `lenso-capability-audit-log`; and
2. `lenso-audit-log-postgres-plugin`.

Publication is manual and runs only from a clean `main` checkout through
`.github/workflows/release-plz.yml`. Pushes to `main` may create or update a
release pull request, but never publish. A live dispatch requires `live=true`,
`confirm=publish`, and the `main` ref. Run the dry-run dispatch first.

Before first publication of each new crate name:

1. pass generated-contract, Rust, PostgreSQL, boundary, and independent package
   verification;
2. allocate the crate name once using crates.io's authenticated initial-publish
   flow, because OIDC Trusted Publishing cannot create a new crate name;
3. configure the crate's Trusted Publisher with owner `LioRael`, repository
   `lenso-audit-log-plugin`, workflow `release-plz.yml`, and no environment;
4. confirm the Capability crate is public before the implementation crate; and
5. run the live workflow only after both crates have their matching publisher.

The workflow has no registry-token fallback. The live job obtains a short-lived
crates.io credential through GitHub OIDC and has only the required
`id-token: write` publication authority. Never use `--no-verify`, a long-lived
registry token, or a Git dependency as a publication shortcut.
