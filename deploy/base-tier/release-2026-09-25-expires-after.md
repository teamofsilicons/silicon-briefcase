# Briefcase 2.1.0 release: Expires after and Self Destruct (25 September 2026)

Briefcase 2.1.0 adds **Expires after** shares and **Self Destruct** files
(UNDERSTANDING.md §Expires after, §Self Destruct). API, worker, browser gateway,
documentation, CLI and client crates are live. The API contract is 2.1.0 with
62 operations. The new operations are `changeExpiringShare` and `makeEntryPermanent`.
Existing operation revisions are unchanged, so 2.0.0 clients keep passing the startup check.

## Released source and artifacts

- Source: `2e6ffefec98daca6e10f199faff0ad3f294dcd50`, tag `v2.1.0`.
  Image candidates were built by run 36055064751 and the native package by run 36059028186 (tag).
- Images in `234951665042.dkr.ecr.us-east-1.amazonaws.com/silicon-briefcase-production`.
  CI artifact checksums and `SOURCE_REVISION` were verified before pushing:

| Component | Tag | Digest |
| --- | --- | --- |
| API and worker | `2.1.0-backend-2e6ffefec98d` | `sha256:ff6c5395b6ac8c8dbf46bb90eba3484c0cad0a57bc042fdede92936b4300a17f` |
| Browser gateway | `2.1.0-web-2e6ffefec98d` | `sha256:e36573451d34baade59f200c78fd32013d2693190bb4f2223afa163b98cac478` |
| Documentation | `2.1.0-docs-2e6ffefec98d` | `sha256:bfa48379cfff334af3c015a11e438bd34a3aca229c5103929148321df37d4d2a` |

- Honeycomb `briefcase` 2.1.0 on the `prod` channel: accepted/public release
  `1aaf92c3-539c-4ad0-9321-c2cfc2adfb8d`. Archive `briefcase-2.1.0.tar.gz`, 20,498,210 bytes,
  SHA-256 `1f7b5ca3ec4b0185f20699025c6aa89d7bcce2fda7ae2d7cf4b912edbf44566f`.
  A fresh isolated `honeycomb install briefcase` selected 2.1.0 and reported `briefcase 2.1.0`.
- [GitHub release v2.1.0](https://github.com/teamofsilicons/silicon-briefcase/releases/tag/v2.1.0).
- crates.io: `briefcase-client` 2.1.0 and `briefcase-cli` 2.1.0.

## Backups

Both databases were dumped online with `postgres:17-bookworm` tools before migrating. The host's
`pg_dump` is 15 and cannot dump the PostgreSQL 17.9 servers. Both dumps passed `pg_restore --list`
(453 TOC lines each):

| Plane | Migration | Entries | Versions | Grants | Links | Bytes | SHA-256 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| production | 46 | 190 | 131 | 25 | 79 | 4,387,199 | `d889351bb34a47cc67692f9f30ff3c09648e91b2cb7d96f6e6bbaec6cb94b13c` |
| testing | 46 | 0 | 0 | 0 | 0 | 278,351 | `5d738313d5748dc8a496fa003a7622742d2652f34a976a19b0b58539c66aa468` |

Off-host copies are in
`s3://silicon-browser-production-234951665042-us-east-1/backups/expires-after-20260925/briefcase/`
(AES256). They were re-downloaded and checksum-matched. The instance role cannot write to that bucket,
so the host uploaded them through operator-presigned PUT URLs. The on-host copies remain under
`/var/lib/silicon-briefcase/releases/expires-after-2.1.0/backup`.

## Rollout

SSM command `d04678e9-7b7c-4257-9735-daf158e33b8c`:

1. Backed up the unit files and `current-release.json` to `.../expires-after-2.1.0/units-before`.
2. Ran `briefcase-migrate` from the new image with each plane's existing migrator env file, taken
   from the 2.0.0 release directory. Migration 47 applied to both databases.
3. Pointed the API, worker, browser unit and upload drop-in at the new digests and restarted all three services.

Readiness returned 200 on the first probe, and all three units were active with zero restarts.
`/api/version` reported contract and build 2.1.0 with 62 operations. After migration the row counts
matched the backups, and every new column was present on both planes. Worker and API logs showed no
errors. The docs image was extracted to `/var/www/briefcase-docs/releases/expires-after-2.1.0`, and
`current` was switched atomically; the public docs and `openapi.yaml` (2.1.0) returned 200.

## Live acceptance

A throwaway CLI home was signed in as `c:saket` with a freshly minted SLT. In `private/c:saket` it
checked the following:

- **Upload:** it uploaded `briefcase-2.1.0-smoke.txt` with `--self-destruct 2`.
- **Expiring link:** it gave the file a 5-minute expiring link and read it anonymously.
- **Filters:** it confirmed that `is:self-destruct` and `is:expiring` both found the file.
- **Self-destruct:** the worker deleted the file at its deadline (not found at 21:17:17 UTC). The
  public link then returned 404, and the file never appeared in the bin.

The session was then logged out. The live browser bundle contains the new Expires after and
Self-destructing upload UI.

## Rollback

Restore the unit files from `units-before`, run `systemctl daemon-reload`, and restart the three
services. Switch the docs `current` link back to `/var/www/briefcase-docs/public-identifiers-2.0.0`.
Migration 47 is additive, apart from narrowing the active-grant unique indexes. A 2.0.0 binary's
`ON CONFLICT ... WHERE revoked_at IS NULL` grant upsert no longer matches those indexes, so grants
fail under a rolled-back API. Plan any rollback with that in mind; do not roll the migration back.
