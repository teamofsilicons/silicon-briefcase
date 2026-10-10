# Briefcase migration to Silicon Accounts and Silicon Apps: progress log

Branch `migrate/accounts-apps-20261010` (from `origin/main` 5b05155). Every stage appends a dated section:
what it did, commits, test commands and results, what is left, gotchas. Decisions are in
[decisions.md](decisions.md); the cutover runbook is [cutover.md](cutover.md).

## 2026-10-10 — Service part 1 of 3: identity (Silicon Accounts authentication, account cache, webhook)

Scope: service stage steps 1–5 and 8 (baseline, dependencies, configuration, authentication incl. the proof
extractor, the accounts cache and custodian helper, the Accounts webhook). Tenancy (drives), delegated-route
redesign, URLs, identity re-keying migrations and the operator command are parts 2 and 3.

### Baseline (before any change, origin/main 5b05155)

Environment: Rust 1.98.0, Postgres 16.11 on 127.0.0.1:5460 (trust), no Docker, no MinIO.
Setup: `CREATE DATABASE briefcase_mig; CREATE DATABASE briefcase_mig_test;` and
`deploy/postgres/001_runtime_roles.sql` on `briefcase_mig` (creates the cluster roles `briefcase_api`
NOBYPASSRLS and `briefcase_worker` BYPASSRLS, already app-prefixed).

| command | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | clean |
| `BRIEFCASE_TEST_DATABASE_URL=…/briefcase_mig BRIEFCASE_TEST_CONTROL_DATABASE_URL=…/briefcase_mig BRIEFCASE_TEST_DATA_DATABASE_URL=…/briefcase_mig_test cargo test --locked --all-targets --all-features -- --test-threads=1` | 267 passed, 0 failed, 1 ignored (`telemetry_live_delivery`, needs the private collector). `s3_object_store` and `postgres_canonical_iam` pass by skipping (no bucket / no canonical DB URL). |
| `cd clients/rust && cargo test --locked --workspace --all-targets --all-features` (separate target dir) | 177 passed, 0 failed, 2 ignored (`live.rs`, needs `BRIEFCASE_LIVE_*`) |

### What changed

Commits: `eca8ed0` Authenticate with Silicon Accounts and retire IAM and the testing plane; `e9d76cc` Record the
Silicon Accounts identity stage of the migration; `c966514` Resolve recipient ids at Silicon Accounts and word
token refusals for callers; `aee2e80` Refuse an account in a tenant recorded for another owner; `8e0891f` Log the
post-guard end-to-end run and migration gotchas; `636c554` Version the service 4.0.0 with contract 4.0.0 (and the
commit adding this line).

- **Dependencies.** `silicon-accounts-client = "0.4.0"` (crates.io) replaces `silicon-iam-client` and the whole
  `vendor/silicon-iam-client` tree (deleted). `ed25519-dalek` is a dev-dependency (test token signing).
  `deny.toml` had no IAM-specific exceptions. Docker build cache id renamed.
- **Configuration** (`src/config/accounts.rs`): `ACCOUNTS_URL` (public origin = JWT `iss`, default
  `https://accounts.teamofsilicons.com`), optional `ACCOUNTS_API_URL`, `BRIEFCASE_APP_ID` (default `briefcase`),
  `BRIEFCASE_APP_SECRET` (`sa_app_…`, required), `BRIEFCASE_ACCOUNTS_WEBHOOK_SECRET` (`whsec_…`, required) and
  `…_PREVIOUS_SECRET`, `BRIEFCASE_WEBHOOK_MAX_BODY_BYTES`, `BRIEFCASE_ACCOUNTS_REQUEST_TIMEOUT_MS`,
  `BRIEFCASE_ACCOUNT_CACHE_TTL_SECONDS`, `BRIEFCASE_PROOF_ISSUERS`. Exact boot messages; plain http only for
  loopback; https required in production. All `BRIEFCASE_IAM_*`, `BRIEFCASE_HONEYCOMB_*`,
  `BRIEFCASE_TEST_ENVIRONMENT_ENCRYPTION_KEY`, runtime `BRIEFCASE_TEST_DATABASE_URL` and
  `BRIEFCASE_TEST_WORKER_DATABASE_URL` are gone. `.env.example` and `docs/deployment.md` (secrets and Silicon
  Accounts setup steps) updated.
- **Authentication** (`src/api/auth.rs`, `src/api/extract.rs`, `src/infrastructure/accounts*.rs`): every bearer
  route takes `Authorization: Bearer <JWT>` verified locally (`VerifyOptions::for_app(app_id).with_issuer(…)`)
  against a cached JWKS (refetch on unknown `kid` at most every 30 s; refresh after 1 h; stale keys kept if a
  refresh fails). Sensitive actions (grant/invite/link access/expiring-share change, revoke, delete/trash,
  self-destruct keep (new `Action::KeepEntry`), version restore, storage configuration) also introspect (answer
  reused ≤ 30 s). The verified `Actor { uuid, kind, id, credential: AccessToken{fid, mid, scope, iat} | Proof{issuing_app, proof_id, scope} }`
  is in `domain::account`. Delegated `/api/v1/obo/*` routes take only `Authorization: Proof sap_…`: verified
  with `verify_proof`, must be `user_verification`, `receiving_app == briefcase`, carry the route's scope (the
  old endpoint id, catalogue in `domain::delegation`) from an issuer allowed for that scope in
  `BRIEFCASE_PROOF_ISSUERS` (deny by default); the request acts as `user.uuid` with the issuing app recorded
  (`AuthenticationMode::OnBehalfOf`, which keeps the existing `apps/{app}/` confinement). Every refusal is a
  401/403 with an exact `code` and message.
- **Removed routes:** `GET /api/v1/iam`, `GET /api/v1/auth/status`, `POST /api/v1/auth/slt`,
  `POST /api/v1/auth/refresh`, the 11 testing-environment operations, `PUT|GET /internal/honeycomb/…`.
  **Added:** `GET /api/v1/accounts` (public discovery: app id, Accounts URL, delegated scopes) and
  `GET /api/v1/me` (verified account, profile refreshed from `userinfo` when stale, custodian).
  `X-Org-ID` is no longer read except as the untrusted tenant selector of the capability-only transfer route.
- **Tenant owner guard:** `organizations.owner_account_uuid` (migration 0048) records the account a tenant was
  created for; an account whose uuid spells a legacy org handle is refused there (decisions S10a).
- **Interim tenancy bridge (until part 2):** `RequestAuthContext::for_account` makes the verified account act in
  its own tenant: tenant key = account uuid, represented actor = (kind, uuid), role `owner`, no tags. Existing
  repository code then bootstraps that tenant (organizations/members rows + legacy root layout
  `public/`, `private/`, `private/{uuid}/`, `apps/`) exactly as it did for a first IAM login. Legacy org tenants
  (`tos`) are not reachable through the API in this interim state.
- **Accounts cache and custodian circle** (`migrations/0048_silicon_accounts.sql`,
  `src/infrastructure/postgres/accounts.rs`, `src/infrastructure/accounts/directory.rs`, `src/domain/account.rs`):
  `briefcase.accounts` (uuid text COLLATE "C" PK, kind, public id + observed-at, display name, pfp, email,
  custodian + observed-at, profile version, status active|access_removed|deleted, revoked_before, deleted_at,
  refreshed_at), `briefcase.account_id_history`, `briefcase.accounts_webhook_receipts`. Filled from verified
  tokens (written only when something changed), lookups (`lookup`/`lookup_by_id`, local 600/min token bucket,
  cache TTL) and webhooks. `AccountDirectory::{resolve, custody, same_circle, is_custodian_of,
  custodial_silicons}` implement id resolution (exact 422 `invalid_account_id`, `unknown_account`,
  `account_inactive`, `principal_kind_mismatch`) and the custodian rule (`domain::account::same_circle`).
  Share recipients (`invite`, `grant`, folder invitees, delegated invite) resolve through it and are stored by
  uuid; `tag:` and `email:` recipients answer 422 `tag_recipients_retired` / `email_recipients_retired`.
- **Accounts webhook** (`POST /webhook/`, `src/api/handlers/accounts.rs`): raw-body `verify_and_parse_webhook`
  (5 min tolerance, current + previous secret), event-id header must match, dedupe on `event_id`
  (receipt outcome applied|stale|ignored), then `account.id_changed` (id + history, ordered by occurred_at),
  `account.updated` (profile by `version`, email only when shared), `silicon.custodian_changed`
  (ordered by occurred_at), `membership.signed_out` (`app_revoked` ignored; other reasons set `revoked_before`),
  `membership.access_removed` (`revoked_before` + status until a newer sign-in), `account.deleted` (status
  deleted, cache anonymised, every token refused). Unknown types and `ping`: 204, logged. 401 bad/missing
  signature or stale timestamp, 400 signed non-event body. Revocations also drop cached introspection answers.
- **Testing plane retired:** `infrastructure/testing*.rs`, `application/testing.rs`, the testing and Honeycomb
  handlers, worker test pool, `TestingEnvironmentContext`, sandbox storage limit, telemetry test ids. Migrations
  and tables untouched (no destructive migration). Legacy tag grants can only be seeded by tests
  (`invite_tag` is `#[cfg(test)]`); reads and revocations of existing tag grants still work.
- **Delegated uploads:** the NOT NULL `iam_organization_id/iam_principal_id/iam_membership_id` columns now hold
  `domain::account::storage_key` UUIDv8s derived from tenant/account/membership (no schema change).
- **Versions:** the service package is 4.0.0 (the repo's policy pairs backend and contract versions);
  `docs/version-policy.md` has the 4.0.0 row (`silicon-accounts-client` 0.4.0). The client and CLI versions are
  the client stage's.
- **OpenAPI** `info.version` and `CONTRACT_VERSION` 4.0.0, 49 operations, `bearerAuth` (JWT) and `proofAuth`
  (`Proof`), `x-proof-scope` on delegated operations, unused schemas dropped. Transformation script kept at
  `<scratch>/briefcase-openapi-part1.py` (text-level, idempotent enough to re-run on 3.0.0).

### Tests (after the change)

| command | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | clean |
| `BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5460/briefcase_mig cargo test --locked --all-targets --all-features -- --test-threads=1` (fresh `briefcase_mig`) | 204 passed, 0 failed, 2 ignored |
| `cd clients/rust && cargo test --locked --workspace --all-targets --all-features` | 177 passed, 0 failed, 2 ignored (unchanged; the client workspace is untouched) |
| `cargo test --lib api::accounts_tests` (DB + wiremock Accounts) | 10 passed |
| `cargo test --lib infrastructure::accounts` (wiremock Accounts) | 6 passed |
| end-to-end against the shared local Silicon Accounts stack: `python3 <scratch>/briefcase-e2e/e2e.py r3` with the API on 127.0.0.1:4101 (`<scratch>/briefcase-e2e/run-api.py`) | 19/19 steps passed (below); rerun as `r4` after the tenant owner guard on a fresh `briefcase_e2e`: 19/19, tenant `VHP` recorded with owner `VHP`, no warnings or errors in the log |
| `briefcase-migrate` on a fresh database `briefcase_e2e` | 0001–0048 applied |
| upgrade path: `<scratch>/briefcase-e2e/apply-0047.py briefcase_upgrade migrations` (origin/main's 0001–0047, recorded in `_sqlx_migrations` with sqlx checksums), a legacy fixture (org `tos`, 2 members), then the new `briefcase-migrate` | 0048 applied; members intact (2); `tos.owner_account_uuid` NULL; no accounts rows |
| `cargo deny --offline check licenses sources bans` | same 7 errors as origin/main (checked on a `git archive` of origin/main): webpki CDLA-Permissive-2.0 licenses ×3, the space-station git source ×2, the crate's own "Proprietary" license, path/wildcard deps. No new finding; cargo-deny is not run in CI. |
| boot validation: `.mig/boot-check.sh` (runs `briefcase-api` with 11 bad configurations) | each refused with its exact message, e.g. `required environment variable BRIEFCASE_APP_SECRET is missing`, `invalid environment variable ACCOUNTS_URL: uses plain http for a host that is not this machine; use https, or http only for localhost, 127.0.0.1 or ::1`, `… BRIEFCASE_PROOF_ISSUERS: \`briefcase.files.delete\` is not a Briefcase delegated scope; use one of …`, `… ACCOUNTS_URL: must use https in production` |

98 baseline tests no longer exist and 36 are new. The removed ones are the deleted IAM, Honeycomb and
testing-plane suites (`api::testing_auth_tests`, `infrastructure::iam::*`, `api::webhook::*` (IAM HMAC),
`application::webhook::*`, `infrastructure::postgres::webhook::*`, `infrastructure::testing::*`,
`tests/postgres_testing_environments.rs`, `postgres_honeycomb*.rs`, `postgres_database_identity.rs`,
`postgres_canonical_iam.rs`, the session-broker and IAM-secret validators, the sandbox share-URL and
testing-contract checks) plus tests rewritten under new names for the new surface (`api::auth`, `api::tests`
security/discovery/retired-route checks, the tenant-projection check, the contract-version check). Every other
baseline test passes unchanged. New: `domain::account` (6), `domain::delegation` (1), `config::accounts` (4),
`api::auth` (4), `infrastructure::accounts` (4) and `budget` (2), `api::accounts_tests` (10), `api::tests` (3),
contract and tenant checks (2).

Marked in progress: `tests/openapi_contract.rs::official_client_consumes_the_exact_published_operations` is
`#[ignore]`: it compares the published contract with `briefcase_client::OPERATIONS`, which still describes 3.0.0.
The client stage regenerates the client inventory for 4.0.0 and removes the ignore.

End-to-end (real local stack, real webhook worker, `interface` as proof issuer): Carbon sign-in token → `/me`,
folder created and listed in its own drive; Silicon via SLT → `/me` shows its custodian (lookup); Carbon shares
with its Silicon by `si:` id (live introspection) and the grant names the Silicon's uuid; unknown id → 422
`unknown_account`; webhook URL pointed at the service (secret kept) → `ping` received and recorded; renaming
the Carbon at Silicon Accounts → `account.updated` applied; `interface` issued a User verification proof →
`/obo/folders/create` made `apps/interface/private/{uuid}/from-interface`, `/obo/entries/list` listed it,
`/obo/files/read` refused with `proof_scope_missing`; `DELETE /v1/me/apps/briefcase` →
`membership.access_removed` applied → the old access token refused with `token_revoked` and the proof with
`account_access_removed`; signing in again → active. Briefcase's webhook URL was restored to
`http://127.0.0.1:9593/briefcase/webhooks` (its secret was never changed). No warnings or errors in the API log.

### Left for part 2 (tenancy) and part 3 (delegated routes, URLs, migrations, operator)

- Part 2: drives as tenants (drive key = owner uuid, Shared with me, custodial drives in listings, custodian
  rule in `domain::permission`, "Silicons are not open to the world" on shares and invitations — the helpers
  `AccountDirectory::same_circle/is_custodian_of` are ready), drive root layout, notifications into recipients'
  drives, per-drive quotas, BYO S3 per drive with a persisted `external_id` (the worker's
  `storage_external_id` still derives only from legacy org handles, so BYO cleanup in a uuid tenant would fail),
  `account.deleted` purge of the account's own drive (today: cache anonymised and every token refused).
- Part 3: `apps/{issuing_app}/` confinement inside the user's drive and the first-party issuer exception
  (`interface`), the transfer route's tenant selection (today `X-Org-ID` = account uuid), `/{owner-id}/{path}`
  URLs with `account_id_history` redirects and legacy `/org/{org}/…` aliases (today `/org/{tenant}/…` only for
  the caller's own tenant), identity re-keying migrations + `identity_links` + `link-identities` operator,
  upgrade-path fixture test, OpenAPI per-operation revisions and docs.
- Client stage: `briefcase-client` contract 4.0.0, un-ignore the cross-check above. Docs stage: README and
  `docs/` still describe IAM in many places (only `docs/deployment.md` configuration steps were rewritten).

### Gotchas

- `.mig/` is excluded through the repository's shared `info/exclude` (worktrees share it with the Carbon's
  checkout); nothing in the checkout's working tree was touched.
- sqlx 0.9 refuses `format!`-built SQL (`AssertSqlSafe`); use `concat!` with a column macro.
- The shared stack rate-limits verification codes (429); the e2e driver backs off 30 s and retries.
- Accounts uuids are 3+ alphanumerics, so a bare word like `bob` is a valid uuid shape: resolution treats it as
  a uuid (and answers `unknown_account`), not as a malformed id.
- Test fixture databases on 5460: `briefcase_mig` (unit/integration, recreated after the 0048 amendment),
  `briefcase_e2e` (end-to-end), `briefcase_upgrade` (0047 + legacy fixture, then 0048).
- Migration 0048 was amended in `aee2e80` (adds `organizations.owner_account_uuid`) before anything outside these
  local databases applied it. A database migrated at `eca8ed0`/`c966514` fails sqlx's checksum check: recreate it.
- The migrator embeds migrations at compile time (`sqlx::migrate!`): rebuild `briefcase-migrate` after editing
  a migration, or the old SQL is applied.
- zsh does not word-split `$VAR`; run multi-word env or command strings through `bash`.
- `<scratch>/briefcase-e2e/` holds `run-api.py` (starts the API with the stack's briefcase credentials from
  `test-stack.json`, never printing them), `e2e.py` (the 19-step check, restores the webhook URL at the end) and
  `apply-0047.py` (brings a database to origin/main's schema the way sqlx would, for upgrade tests).
  `.mig/boot-check.sh` runs the boot-validation cases.

## 2026-10-10 — Service part 2 of 3: drives and permissions

Scope: service stage step 6 and the authorisation parts of step 10 — drive as tenant, definer helpers, My files /
Shared with me / custodial drives, rights (drive owner, creator, custodian), sharing by id, link rules, quotas,
bring-your-own storage, notifications, search and filter scope, bins. Decisions S20–S37 in
[decisions.md](decisions.md); cutover notes and the UNDERSTANDING proposal extended.

### Hand-over

An earlier run of this stage stopped at 05:43 after committing `dbcd2c2` (migration 0049, persisted External
IDs) and leaving the domain/persistence/API rewrite uncommitted (49 files, green tests, two clippy errors). It
was backed up to `<scratch>/briefcase-p2-wip-backup/`, reviewed file by file, fixed (clippy:
`ordinary_routes` too long → `drive_routes()`; `DriveState` bools → `recorded_owner`), the D9 wording in
`ActorKind`/`ActorTypeDto` docs corrected, and committed as `8032824`. Its scratch databases
(`briefcase_p2_owner`, `briefcase_p2_sql`) and role (`briefcase_p2_migrator`) were dropped.

### What changed

Commits: `dbcd2c2` Add personal drives to the schema and persist bring-your-own External IDs (earlier run);
`8032824` Make every account's drive the storage tenant and authorize by drive; `df27c9f` Test the cross-drive
functions under production's database roles; `995e87a` Refuse moving an entry into another drive with an exact
reason; `3689e16` Cover every drive route family for owner, share, custodian and outsider; `971a226` Document
personal drives in the API contract and registry; `c652e03` Resolve each drive once per Shared with me page;
`2b9dd98` List the drive operations in the documented-status contract test; and the commit adding this section.

- **Migration 0049** (`migrations/0049_personal_drives.sql`): drive root shape + unique index, `entry_id`
  index, path derivation for children of a drive root, `apps` reserved at the top (trigger), drive key = owner
  (CHECK), persisted `external_id` (backfilled with the org-handle derivation, immutable), `shared-with` index,
  `silicon_share_allowances`, `drive_definer_lookup` policies for the migrating role, and the definer functions
  `entry_drive`, `shared_with_principal`, `principal_drives`, `deliver_notification`,
  `revoke_deleted_account_shares`, `erase_deleted_account_drive`, with grants.
- **Domain** (`domain/actor.rs`, `domain/permission.rs`): `DriveAccess {Owner, Custodian, Visitor}`,
  `RequestAuthContext::{for_drive, for_account, in_drive, with_custodial_silicons}`, creator set = caller + its
  Silicons; the four-source policy (S22); `OrganizationRole`, tags and Public/Tag boundaries removed from policy;
  `SystemEntryKind::{DriveRoot, ApplicationContainer (apps/), ApplicationFolder (apps/{app}/)}` plus legacy kinds.
- **Drive resolution** (`api/drives.rs`): `for_entry` (entry_drive), `named` (`drive` selector), `own`,
  `enter` (relation from the cached custody facts; proofs confined to the user's own drive); every handler moves
  its request into the right drive before calling a use case.
- **Persistence** (`infrastructure/postgres/drives.rs` replaces `roots.rs`): drives created on the owner's or
  custodian's first request with a root folder, visitors become participants, `apps/{app}/` on an app's first
  use; queries use the creator set and drive owner rights instead of roles/tags; restore falls back to the drive
  root; notifications delivered into the recipient's drive; worker email reads `accounts.email`.
- **Routes**: `GET /api/v1/drives`, `GET /api/v1/shared`,
  `GET /api/v1/silicons/{silicon}/allowed-accounts`, `PUT|DELETE …/allowed-accounts/{account}`; `drive` on
  listings, folder creation, uploads, bin, usage, storage configuration and permission inspection; uploads
  without a destination go to the top of the drive; 422 `cross_drive_move`, `reserved_name`, `tags_retired`,
  `public_space_retired`, `protected_folder`; 403 `silicon_not_open`, `not_silicons_custodian`; 422
  `not_a_silicon`, `allowance_self`.
- **OpenAPI**: 54 operations, new `Drives` tag, `Drive`/`Silicon` parameters, `AccountSummary`, `Drive(List)`,
  `Shared(Entry|Page)`, `AllowedAccounts`; revisions bumped per S36 and mirrored in `api/versioning.rs`;
  `docs/api/operations.md` regenerated from the document; `docs/version-policy.md` notes the drive changes.
  `openapi-spec-validator openapi.yaml`: OK (the session scratchpad's `venv-oas`).

### Tests

| command | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | clean (2 errors in the inherited WIP fixed first) |
| `.mig/test.sh` = `BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5460/briefcase_mig cargo test --locked --all-targets --all-features -- --test-threads=1` (final) | **206 passed, 0 failed, 2 ignored** (lib 194, openapi_contract 3 + 1 ignored, postgres_drive_roles 1, postgres_metadata 3, postgres_versioning 4, s3_object_store 1; ignored: `telemetry_live_delivery`, the client cross-check) |
| inherited WIP before any change (same command) | 197 passed, 2 ignored; `api::drives_tests` really writes (entries 1069 → 1095) |
| `cargo test --test postgres_drive_roles` | 1 passed: a throwaway database migrated by a non-superuser, non-BYPASSRLS owner; every definer function called as `briefcase_api`; RLS still confines each session; the worker's self-destruct statement passes the drive-root trigger |
| `cargo test --lib api::drive_access_tests` | 8 passed (uploads charged to the drive they land in, versions/content/download 404 for outsiders, self-destruct keeps, storage settings, search scope, permanent/public URLs, proofs confined, filters by id, cross-drive moves) |
| `cargo test --lib api::drives_tests` | 4 passed (drives and custodial drives, shares by id + Shared with me + bins + inbox + inspection, Silicon allow-list, account deletion) |
| `cargo test --test openapi_contract` | 3 passed, 1 ignored; the registry test now compares id, method, path and revision for all 54 operations |
| all 49 migrations applied with psql one file per transaction as `briefcase_test_migrator` (NOSUPERUSER NOBYPASSRLS) | all applied |
| upgrade path: `briefcase_upgrade` (origin/main 0047 + legacy fixture, then 0048 in part 1) + legacy `public_root`, a link-public folder and an active BYO config, then `briefcase-migrate` | 0049 applied; legacy rows and paths unchanged; `tos` stays ownerless (not a drive, `entry_drive` NULL); `external_id` = `161cc38e…` = SHA-256 of `silicon-briefcase:byo-storage:v1\0tos` computed independently |
| fresh database: `briefcase-migrate` on a recreated `briefcase_e2e` | 0001–0049 applied |
| end to end: `python3 <scratch>/briefcase-e2e/e2e-drives.py p2a` against the shared local Silicon Accounts stack, API on 127.0.0.1:4101 (`run-api.py "briefcase.entries.list=interface"`) | **36/36 passed**, no WARN/ERROR/5xx in the API log; webhook URL restored to `http://127.0.0.1:9593/briefcase/webhooks` |

End to end, with real sign-ins (three Carbons, one Silicon via SLT): ada's drives list her own and Scout's;
top-level paths start fresh; a share by `c:` id is stored by uuid and appears in bob's Shared with me; in ada's
drive bob sees only the shared folder; bob's share to Scout is refused (`silicon_not_open`) until ada allows him,
then reaches Scout; ada opens and adds to Scout's drive as herself; usage is the owner's and custodian's only;
a cross-drive move is refused; an `interface` User verification proof for bob cannot reach ada's drive but works
in bob's; ada hands Scout to bob at Silicon Accounts → `silicon.custodian_changed` → ada loses the custodial
drive (keeping only what she created there), bob gains it; cat deletes her account → `account.deleted` → her
drive is erased (2 entries due for purge), the share to her ends, her files leave bob's Shared with me, her token
is refused, ada's files are untouched.

### Left for part 3 (and later stages)

- Part 3: identity re-keying migrations, `identity_links` and the `link-identities` operator command (moving
  legacy org data into drives: `{org}-public/`, `{org}-{tag}/`, `apps/{app}/{org}-public/`, `unclaimed/…`,
  snapshot shares written directly, tag grants expanded, quota overrides and `stored_bytes` recomputed);
  `/{owner-id}/{path}` permanent URLs (`/api/v1/paths/…`) with `account_id_history` redirects and the legacy
  `/org/{org}/…` alias table; `apps/{issuer}/` confinement details and the first-party `interface` exception
  (today proofs reach only `apps/{issuer}/` in the user's own drive); the transfer route's tenant selector; the
  per-operation revisions for the URL operations.
- Client stage: `briefcase-client` 4.0.0 with the 54 operations and their revisions (un-ignore
  `official_client_consumes_the_exact_published_operations`). CLI: `@<id>/path` targets map to `drive`.
- Web stage: My files / Shared with me / one entry per custodial Silicon from `/drives` and `/shared`, the
  "You might not be seeing all the contents of this folder" note on any folder in another drive, the Silicon
  allow-list in a Silicon's settings, storage settings per drive.
- Docs stage: README and `docs/` beyond the API inventory still describe organisations.

### Gotchas

- `openapi.yaml` is edited text-level by `<scratch>/briefcase-openapi-part2.py` (strict: every edit asserts its
  original). Plain YAML scalars cannot contain `: ` (write `` `c:`/`si:` ``), and inside single-quoted scalars
  apostrophes are doubled.
- sqlx 0.9 needs `sqlx::AssertSqlSafe(...)` for DDL built at run time (test role/database names).
- The worker never updates `entries.name/parent_id/path`; 0046's alias trigger (SELECT on
  `identifier_path_aliases`, not granted to the worker) would refuse it. 0049's drive-root trigger fires on
  `deleted_at` too, which the worker does update; its EXECUTE grant covers that.
- The test role `briefcase_test_migrator` (LOGIN, local password) stays on 5460: `postgres_drive_roles` creates it
  when missing and drops only its own throwaway database.
- After the OpenAPI commit `971a226` one lib test (`every_openapi_operation_has_the_expected_response_status`)
  failed until `2b9dd98` added the new operations to its table; run the whole suite, not just the contract tests,
  after a contract change.
- MinIO binaries are gone from dl.min.io (HTTP 410), so S3 paths are still not exercised locally.

## 2026-10-10 — Service part 3 of 3: delegated routes, addresses, the move

Scope: service stage steps 7, 9, 10 and 11 — delegated (proof) routes and first-party apps, permanent addresses by
owner id with redirects for ids given up and the legacy `/org/{org}/` aliases, the archive listing for the web,
migration 0050 (empty and upgrade path), the `link-identities` operator command, OpenAPI, the whole suite.
Decisions S38–S48 in [decisions.md](decisions.md); runbook additions in [cutover.md](cutover.md); UNDERSTANDING
proposals extended.

### Hand-over check

At 585b7ae (end of part 2) the suite was green as recorded: 206 passed, 0 failed, 2 ignored
(`.mig/logs/p3-baseline.log`).

### What changed

Commits: `1fb52fd` Name drives by their owner's id in permanent addresses and widen first-party apps; `66da1c3` Add
the link-identities operator command for the cutover to personal drives; `bc37d8d` List archive contents for the
web and publish the part 3 contract changes; `5060817` Fix what the live cutover run found: v7 share ids and
Silicons recorded without a custodian; `be02193` Record the delegated routes, addresses and cutover stage of the
migration; `3dde92e` Bound archive listings and keep the address docs in Briefcase's words (zip directory read
≤ 8 MiB, at most four tar/7z listings at once); `1387563` Record that Briefcase needs no Ting adapter and issues no
proofs (S49); and the commit adding this line.

- **Delegated routes** (S38–S41): one scope per route as before (the old catalogue is complete); new
  `BRIEFCASE_PROOF_FIRST_PARTY_ISSUERS` (`AuthenticationMode::FirstPartyOnBehalfOf`, `Actor::confined_app`): listed
  apps reach everything the account can (other drives through shares, custodial drives, `drive` in
  `/obo/entries/list`), everyone else stays in `apps/{app}/` of the account's own drive; `/obo/entries/trash`
  refuses entries another app created (403 `not_created_by_app`); the byte transfer finds its drive from the
  reservation (`briefcase.delegated_upload_drive`), `X-Org-ID` optional.
- **Addresses** (S42, S43): `ResponseMapper` names drives by owner id (labels from the account cache per request;
  `AppState::mapper_for`), content URLs by uuid; new `api/handlers/locations.rs` serves
  `GET /api/v1/paths/{owner}[/{path}]`, `/api/v1/public/{owner}/{path}` and `/org/{org}/{path}`;
  `AccountDirectory::drive_owner` resolves ids cache-first and sends ids given up to the current one (308);
  `legacy_path_aliases` + `briefcase.resolve_legacy_location` (longest prefix). Invitation emails link the new
  address.
- **Migration 0050** (`identity_link_runs`, `identity_links`, `identity_link_ledger`, `legacy_path_aliases`, the
  two definer functions and their grants, a definer policy on `delegated_uploads`). Additive only.
- **link-identities** (S44, S45): `briefcase-migrate link-identities --file … [--dry-run] [--json]`; Rust side in
  `infrastructure/postgres/link_identities/mod.rs` (mapping parser, runner on a detached connection, operator
  summary), the program in ten SQL files there (setup and maintenance mode, resolution, undo, move primitive,
  planning, after-move work, verification and report, main).
- **Archive listing** (S46): `GET /api/v1/entries/{id}/archive`; parsers in `application/archive_listing.rs`,
  handler in `api/handlers/archive_listing.rs`.
- **Custodians after the cutover** (S47): `/drives` learns up to 10 unknown custodians per call.
- **Diagnostics**: internal repository failures log their cause (`metadata_repository_failed`).
- **OpenAPI** (S48): 57 operations; `openapi-spec-validator openapi.yaml`: OK; `docs/api/operations.md`
  regenerated (`<scratch>/briefcase-operations-doc.py`); edits scripted in `<scratch>/briefcase-openapi-part3.py`.
- **Docs**: `.env.example`, `docs/deployment.md` (first-party apps, the cutover command).

### Tests

| command | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | clean |
| `.mig/test.sh` (= `BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5460/briefcase_mig cargo test --locked --all-targets --all-features -- --test-threads=1`) | **229 passed, 0 failed, 2 ignored** (lib 212, openapi_contract 3 + 1 ignored, postgres_drive_roles 1, postgres_link_identities 5, postgres_metadata 3, postgres_versioning 4, s3_object_store 1; ignored: `telemetry_live_delivery`, the client cross-check) |
| `cargo test --test postgres_link_identities` | 5 passed: upgrade path (origin/main's 0001–0047 + the legacy organization fixture, then 0048–0050, as a non-superuser NOBYPASSRLS migrator) and an empty database (50 migrations); the move (every destination, owners, snapshot and expanded shares, notification, bucket copies, usage, multipart, aliases, accounts, UUIDv7 ids, legacy resolution and drive visibility as `briefcase_api`); dry run and idempotent re-run; corrected mapping ≡ fresh run over the whole snapshot; refusals (unlinked owner, many-to-one, uuid = organization key, kind mismatch, work in flight, used since) |
| `cargo test --lib api::locations_tests` | 3 passed (owner id and uuid addresses, top of a drive, link reads, 308 after `account.id_changed`, legacy aliases incl. prefix and anonymous links) |
| `cargo test --lib api::delegation_tests` | 2 passed (first-party reach incl. custodial drive and `drive`, confinement, origin recorded, trash only app-created, one scope per route; transfer without `X-Org-ID`) |
| `cargo test --lib api::archive_tests application::archive_listing` | 7 passed (zip incl. zip64 extra and zip64 end record, tar/tar.gz/tar.zst incl. long names, symlinks, caps, 7z, format detection; through the router with an in-memory object store: listings, limit, refusals, access) |
| `cargo test --lib api::drives_tests` | 5 passed (incl. custodians finding Silicons the cutover recorded) |
| `cargo deny --offline check licenses sources bans` | same 7 errors as origin/main; nothing new from `flate2`/`sevenz-rust2` |
| `cd clients/rust && cargo test --locked --workspace --all-targets --all-features` (separate target dir; the client workspace is untouched) | 177 passed, 0 failed, 2 ignored (unchanged) |
| end to end: `<scratch>/briefcase-e2e/e2e-cutover.py prepare|check RUN` with the real `briefcase-migrate` and `briefcase-api` (127.0.0.1:4101, `run-api-p3.py`) on `briefcase_p3cut` against the shared local Silicon Accounts stack | **25/25 passed** (run `d093745`): three real accounts; 0001–0047 + fixture, then 0048–0050; dry run (summary printed), apply, re-run (already in place); then saket's drives incl. Scout's, his moved files at the top, `/{id}/notes`, bob's Shared with me, bob reads his guide in saket's `tos-public`, `/org/tos/public/…` and `/api/v1/public/tos/…` anonymously, `/org/tos/private/c:saket/notes` for saket and bob, c:gone's file closed, custodian opens Scout's drive, Scout its own, Interface (first-party proof) lists bob's view of saket's drive, a real id change (`POST /v1/me/id`) → `account.id_changed` → `308` with the query kept → the new id resolves; webhook URL restored; no WARN/ERROR or 5xx in the API log |

The first live run (`c093407`) failed 4 steps and found two defects the database tests could not: shares created
with UUIDv4 ids (the domain requires v7) and Silicons recorded without a custodian missing from their custodian's
drive list. Both fixed in `5060817`, with a test each.

### Left for later stages

- Client stage: `briefcase-client` 4.0.0 with the 57 operations and revisions (un-ignore
  `official_client_consumes_the_exact_published_operations`); `@<id>/path` targets; the transfer without
  `X-Org-ID`; `GET /paths/…` for permanent URLs; the archive listing.
- Web stage: serve `/{owner}/{path}` (follow `308` to the body's `permanent_url`) and `/org/{org}/{path}`
  (redirect to the entry's `permanent_url`); use `listArchiveContents` for the archive renderer.
- Docs stage: `README.md`, `docs/api/delegated-uploads.md` (`X-Org-ID` is optional now), `docs/obo.md`,
  `docs/sharing.md` and the IAM-era guides still describe organisations and IAM.
- Cutover: Interface, Waveform, Extend, Browser, DM and Commit changes as listed in cutover.md.

### Gotchas

- The link program runs with user triggers off: it must keep every invariant the triggers keep (paths, closure,
  participants, UUIDv7 ids). `link_verify` recomputes paths and closures; the UUIDv7 rule is asserted in the test.
- Removing a row cascades immediately even when foreign keys are deferred (`ON DELETE CASCADE`): the undo removes
  created rows only after every move is undone.
- plpgsql: a record variable named like a column (`item` vs `identity_link_ledger.item`, `report`) is an error
  ("column reference is ambiguous"); qualify columns or rename variables.
- wiremock matches proof bodies by substring: never give one test proof token a prefix of another.
- Throwaway databases on 5460 from these tests (`briefcase_link_*`) are dropped by the tests; `briefcase_p3cut`
  (the e2e database) is kept for inspection. Test identities on the stack: `briefcase-p3-*-c093407`,
  `briefcase-p3-*-d093745`.

## 2026-10-10 — Resumed CLI and client stage

Reviewed and completed the inherited client/CLI rewrite. The client and CLI are 4.0.0, use the
57-operation Accounts-era contract, expose personal/custodial drives and explicit sharing, and replace
broker sign-ins with public-client device/SLT flows and file-locked rotating refresh. Saved credentials
stay bound to the API deployment. Offline discovery and status, hidden runtime aliases, retired-option
errors, token redaction, telemetry controls and daemon-uninstall compatibility are covered.

Rewrote the shipped CLI/client guides to match the implementation, removed the retired testing guide
from the sync inventory, and restored the formerly ignored service/client operation contract test.
The previous process security suite was tied to the retired broker and credential format: its eight
file-operation tests were retained and migrated (pagination, cursor-cycle refusal, durable mutation
replay, sharing/expiry, links, self-destruct and reports). The 21 Accounts process tests replace broker
login/refresh/testing-plane assertions. Nothing was relaxed to accept incorrect file behavior.

Verification against the current code:
- Client workspace `cargo test --locked --workspace --all-targets --all-features`: **142 passed,
  2 ignored** (external live credentials required). Log: `.mig/logs/client-final.log`.
- Client workspace `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`: passed.
- Service `cargo test --offline --test openapi_contract`: **4 passed**, including exact identity, method,
  path and revision equality for all 57 operations. Root lockfile updated for the path dependency 4.0.0.
- Public-client and CLI tests exercise denial, expiry, slow-down, single-flight refresh and revoke.

Next: real CLI/API/Accounts storage journeys, native Apps archives, frontend and active docs. Production
cutover remains separate. The recovered local Accounts stack was recreated; old cached test sessions
are invalid and all new checks must mint fresh identities.
