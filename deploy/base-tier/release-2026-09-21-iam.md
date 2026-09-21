# Canonical IAM compatibility backend release — 2026-09-21

Briefcase backend 1.1.1 authenticates canonical IAM public IDs while preserving its existing UUID ownership and membership keys. Its API contract and published client remain 1.1.0. Both legacy IAM and canonical IAM authorization snapshots are supported through official IAM client 3.0.0.

The backend source is 5247cc1cf2190465a1c8bd917ef2233cfd6f5343. Commit d64f2f4 records the backend Honeycomb integration already present in the preceding deployment; importer commit 97a2ab4 removes the database TEMP privilege requirement. Unrelated CLI, browser, and documentation drafts were excluded from the release checkout.

The ARM64 image is:

```
234951665042.dkr.ecr.us-east-1.amazonaws.com/silicon-briefcase-production@sha256:0e007accc1270ed2626ecca3d76a101b5e46c9879eb77ac190f108c06e877ef3
```

Its OCI source revision matches the backend commit. API and worker switched at 09:51:45UTC on i-00c2b0c4968b57186 in us-east-1. The browser image, environment files, Nginx configuration, database credentials, and IAM application identity were preserved. Both backend containers remained stable with zero restarts during the verification interval.

Before migration, API and worker writers were stopped and complete native RDS snapshots were confirmed available:

- `briefcase-iam3-5247cc1-production-20260921094608`
- `briefcase-iam3-5247cc1-testing-20260921094608`

The production and separate shared testing databases both passed exact migration/checksum checks through 0045. The trusted private IAM export verified 28 production bindings and 50 bindings across 8 retained test worlds. All 13 production member records, 18 test member records, and 33 delegated upload ownership references retained their keys and fingerprints. No runtime privileges were broadened for import.

The first snapshot attempt was aborted when the testing database was in its scheduled backup window. No migration or image change occurred in that attempt, and both original services were restored and checked successfully. Its additional production snapshot was retained. The successful retry checked both database instances were available before stopping writers. Rollback restores the previous service units independently; it never restores an older database snapshot automatically after workers resume.

Verification before the canonical IAM switch:

- 241 backend unit tests passed; 1 existing test remained ignored. All-target Clippy passed.
- Disposable PostgreSQL 17 tests covered the restricted migration owner, preserved keys, runtime environment isolation, canonical and legacy membership webhooks, importer rollback/apply/recheck, conflicting exports, and a migration owner without database TEMP permission. Temporary containers were removed.
- Public readiness returned 200 and version reported build 1.1.1. A request with an organization and no bearer returned 401.
- Maharaj's installed Briefcase 1.1.0 saved session read the same 3 existing entries as before deployment. Authentication status returned 200 with the same local owner UUID.
- A controlled existing IAM test world passed Briefcase discovery, test login, entry listing, and authentication status. The local owner UUID remained stable and its test token was rejected by production with 401. This new Briefcase session was retained privately for post-cutover verification.

Eight older retained testing credentials were rejected directly by the preceding IAM service at discovery. They were not reset or rotated. Their stored ownership bindings were still imported and verified. An actual previously pending delegated upload was not completed during rollout; its retained bindings were verified, and the upload authority paths are covered by the regression suite.

Deployment command: 0693de62-3cb7-42d5-9f1d-52a8601083af. Protected rollout state and backup manifests are retained on the host under `/var/lib/silicon-briefcase/releases/iam3-5247cc1-20260921-r2`. No credentials or identity export are committed to the repository.
