# Changelog

The entries below record release candidates created by the retired shadow
coordinator. Versions `0.1.1` through `0.1.3` were not published to crates.io;
the latest public version remains `0.1.0` until the independent Release-plz
workflow publishes `0.1.3`.

## Historical shadow candidates (not published)

### lenso-module-audit-log@0.1.3

### Fixes

Validate the complete Shadow release chain with ancestry-aware preflight.

## lenso-module-audit-log@0.1.2

### Fixes

Validate the final Shadow release chain against the current publisher runtime.

## lenso-module-audit-log@0.1.1

### Fixes

Validate the reviewed Shadow release control plane end to end.
Exercise the corrected shadow release handoff.
Regenerate the digest-bound candidate after the coordinator token scope correction.
