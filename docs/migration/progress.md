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
