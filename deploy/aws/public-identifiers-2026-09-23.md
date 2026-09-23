# Briefcase 2.0.0 identifier cutover

Briefcase API, worker, browser gateway, and documentation were activated on 2026-09-23 after IAM 4.0 and Honeycomb 0.4. The public API remains `/api/v1`; its identity contract is 2.0.0. The configured application is `briefcase`, owned by `tos`.

Release source is `53504049c8bba2a1ba7bbf8646dd22e6cfc8b5f7`. Compiled production artifacts came from `c6d74c3badbf91f6a1018ba96268e0887fdeba2f`; subsequent changes affect tests, documentation, and the offline migration operator, not compiled production code or dependency versions. Hosted CI and all six native targets passed again at the release source (runs 35879772164 and 35879765717).

The active immutable images in `234951665042.dkr.ecr.us-east-1.amazonaws.com/silicon-briefcase-production` are:

| Component | Digest |
| --- | --- |
| API and worker | `sha256:f584b8a4fcba2d7aa9506dc7995069e887719fb00d1bafadcf20e44ccf25143e` |
| Browser gateway | `sha256:f453ed1fbbf5fc1008e72a86f69b835f484cec84ee138cae02772e12f60de88b` |
| Documentation | `sha256:16acc39c04fb0581e104f8d7e0bbbb84674c9959fe081cf7be10eb4b1f08997e` |

The three systemd units and the browser upload drop-in retain those image references. `/etc/silicon-briefcase/current-release.json` records the active release. The persistent application secret and API environment changed only `BRIEFCASE_IAM_APP_ID`; all other credential values were preserved. Documentation is served from `/var/www/briefcase-docs/public-identifiers-2.0.0` through the existing `current` symlink.

Both PostgreSQL 17 databases were backed up while all three services were stopped. Custom-format dumps passed `pg_restore --list`; protected configuration, session state, CA files, and systemd definitions were archived and verified against their source files. Seven backup objects were uploaded with AES256 encryption and independently verified by size and SHA-256 under:

`s3://silicon-browser-production-234951665042-us-east-1/backups/public-identifiers-20260923/briefcase/frozen-1790175517250261613/`

Production dump SHA-256: `4aea5ec1996fe5db3a14cb74bd071ab1c2bb50017a38f105b1b5347ccaeb22b7`. Testing dump SHA-256: `77c276d6d90530a016a0440495ca994c2cff4f2642e8f6a1d4d6dae59baeed86`.

The exact image first migrated isolated restored copies with no external network access. The IAM final quiesced inventory supplied every retained production actor and application mapping. All 30 production identity bindings retained their UUIDs. Schema migrations 1–45 retained their existing checksums; migration 46 and the reviewed offline mapping were then applied to both live databases.

The frozen production baseline of 173 entries and 114 versions was preserved. Verification compared version storage descriptors and checksums, immutable identity keys, idempotency request hashes and response bodies, RLS flags, foreign-key properties, and grants. The mapper created 152 aliases for old permanent paths. Historical webhook aggregate UUIDs and signed receipt evidence were preserved exactly; a dedicated regression covers this case.

The user separately authorized deletion of all testing worlds and their data. Eleven obsolete Briefcase control records referred to IAM worlds already deleted. The existing `reset_current_iam_testing_environment` function removed their test filesystem and identity projections while preserving 21 provider-cleanup descriptors. Control records were retired with their ordinary two-day recovery period; 19 existing Honeycomb disabled-state tombstones were preserved. The normal worker subsequently completed all 21 provider cleanup jobs. Final verification found zero test entries, versions, identity bindings, active environments, or cleanup jobs. Production data was excluded from this reset.

Live acceptance verified all three units active; public readiness, IAM discovery, browser, and docs returned HTTP 200. Authenticated reads returned `c:saket`. A retained private file and its history were readable through both its original and canonical paths; both downloads matched the recorded SHA-256. A fresh Honeycomb installation reported `briefcase 2.0.0`.

The six-platform Honeycomb archive is `briefcase-2.0.0.tar.gz`, SHA-256 `d5abc154e4cb4fec5938af384d584e30d3a1a086eb6c4dba0cfa8b089bff6337`, published as production release `e03af56a-4ab8-4609-a6f7-d4e40ba9277f`. GitHub release: <https://github.com/teamofsilicons/silicon-briefcase/releases/tag/v2.0.0>.

Operator receipts: backup `b9ffcb40-b515-4004-bbd6-8a8c2bbca241`; successful restored-copy rehearsal `da580043-60f9-4856-ac94-174e821f476f`; live mapping `a24e597f-9859-4281-96d4-347a4fa95737`; activation `936486f0-ac51-4477-9cbf-b671b00197f0`; final state verification `4d977474-893f-4a16-8bec-689023d1ba8e`. Protected operator inputs and receipts remain under `/var/lib/silicon-briefcase/releases/public-identifiers-2.0.0` on the host.
