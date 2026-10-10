# Briefcase cutover to Silicon Accounts: runbook notes

Notes for the production switch, collected by each stage. Nothing here has been run against production.

## From service part 1 (identity), 2026-10-10

Before deploying the new service:

1. **Silicon Accounts app `briefcase`** (with the app's own credentials, never from the Briefcase CLI):
   sign-in setup allows `https://briefcase.teamofsilicons.com/auth/callback`, `device_flow: true`,
   `public_client: true`, optional field `email`. Set the webhook:
   `PUT /v1/apps/briefcase/webhook {"url": "https://backend.briefcase.teamofsilicons.com/webhook/"}` and keep
   the returned `whsec_…` secret (a PUT keeps an existing secret and returns `null`).
2. **Secrets and settings** on the base-tier host (`/etc/silicon-briefcase/*.env` / Secrets Manager
   `silicon-briefcase/production`): add `BRIEFCASE_APP_SECRET`, `BRIEFCASE_ACCOUNTS_WEBHOOK_SECRET`,
   `BRIEFCASE_PROOF_ISSUERS` (production value in decisions.md S4); set `ACCOUNTS_URL=https://accounts.teamofsilicons.com`
   and `BRIEFCASE_APP_ID=briefcase`; remove every `BRIEFCASE_IAM_*`, `BRIEFCASE_HONEYCOMB_*`,
   `BRIEFCASE_TEST_ENVIRONMENT_ENCRYPTION_KEY`, `BRIEFCASE_TEST_DATABASE_URL` and
   `BRIEFCASE_TEST_WORKER_DATABASE_URL`. The API refuses to start without the app secret or webhook secret.
3. **Migrate** with `briefcase-migrate` (adds migration 0048; no existing data changes).
4. **Drain delegated uploads** made under IAM (`delegated_uploads` rows in `reserved`/`receiving`/`staged`):
   their identity keys will not match proof-authenticated calls after the switch. Same for
   `idempotency_records` still `in_progress`.

Switch together (they break otherwise):

- **Interface** (outside this migration) must switch its Files feature to User verification proofs at the same
  time: it uses the removed `/api/v1/auth/slt` + `/auth/refresh` broker and `X-Org-ID` bearer calls today. With
  `BRIEFCASE_PROOF_ISSUERS` it may use `briefcase.entries.list`, `briefcase.files.read`,
  `briefcase.folders.create`, `briefcase.uploads.*`, `briefcase.invitations.create`,
  `briefcase.link_access.update` (confined to `apps/interface/` until part 3 widens it). Otherwise disable
  Interface Files at the cutover.
- **Waveform, Extend, Silicon Browser** call `/api/v1/obo/*` with IAM OBO headers through the vendored
  `briefcase-client` 3.x: they must send `Authorization: Proof sap_…` (User verification for receiving app
  `briefcase` with the scopes above) and, for the byte transfer, `X-Org-ID: <the proof user's uuid>`.
- **Stemcell** checks `briefcase iam --json` and installs through Honeycomb (CLI stage handles the alias).

After the switch: IAM webhook deliveries to `/webhook/` answer 401 (expected); remove the IAM webhook
registration. The testing-plane database and the Honeycomb/testing tables are no longer used; back them up and
decommission with the Carbon's approval.

Open for parts 2 and 3: legacy org tenants (`tos`) are not reachable until the identity re-keying and drive
restructure run; do not cut over production before those parts land.

## From service part 2 (drives and permissions), 2026-10-10

1. **Migration 0049** (with 0048) changes no existing content. It adds the drive constraints and indexes, the
   Silicon allow-list table and the cross-drive SECURITY DEFINER functions, and backfills
   `organization_storage_configs.external_id` with the exact derivation in use today (so customers' role trust
   policies keep working; check with `SELECT org_id, external_id FROM briefcase.organization_storage_configs`).
   It must run as the usual migrator role: the definer functions' cross-drive reads rely on policies created for
   the role that ran the migration.
2. **No new settings.** Quotas stay at 100 GiB/day and 1 PiB, now per drive; an organization's overrides are
   carried to its owner's drive by part 3's re-keying.
3. **Legacy organization data stays unreachable** until part 3 moves it into drives (`{org}-public/`, tag
   folders, `apps/{app}/{org}-public/`, `unclaimed/…`) with snapshot shares. Do not cut over before part 3.
4. **Silicon allow-lists start empty.** After the switch a share to a Silicon from outside its custodian's
   Silicons needs the Silicon or its custodian to allow the sharer first. Part 3's snapshot shares record access
   members already had, so the re-keying command must write them directly rather than through the API's
   allow-list check. Tell Carbons that new shares to someone else's Silicon need an allowance.
5. **Clients switch to drives together with the service** (the web and CLI stages, and Interface): there is no
   `root_type` public/tag any more, paths no longer start with `private/{id}/`, the top of a drive is a real
   folder (`GET /drives` gives its id), `drive` names another account's drive, and `GET /shared` replaces
   browsing other members' private folders. Apps acting through proofs reach only the user's own drive.
6. **Account deletion is destructive by design:** `account.deleted` erases the account's own drive (purged at
   once by the worker, bring-your-own objects included). Before the switch, confirm the worker's cleanup can
   assume each bring-your-own role with the stored External ID.

## From service part 3 (addresses, other apps, moving the data), 2026-10-10

This completes the service side: production data can now be moved. Order of the switch:

1. **Stop** the API and the worker; drain in-flight work (the command refuses otherwise: idempotency records in
   progress, delegated uploads reserved/receiving/staged/cleanup pending, multipart sessions open, outbox pending).
2. **Back up** the database (`pg_dump --format=custom`; the 2.1.0 dump was 4.4 MB) and keep it until the
   cutover is accepted. It is the way back once the new service has been used.
3. **Migrate**: `briefcase-migrate` applies 0048–0050 (no data changes).
4. **Write the mapping** (`docs/migration/decisions.md` S44 has the format). For each identity stored in the
   organization (`SELECT DISTINCT actor_type, actor_id FROM briefcase.organization_members` as the migrator, plus
   `iam_identity_bindings` for principal UUIDs), find the Silicon Accounts account — normally the same handle:
   `lookup_by_id` with Briefcase's app credentials, cross-checked with the IAM export — and write
   `iam_principal_id,accounts_uuid,kind,accounts_id`, e.g. `c:saket,zQo,carbon,c:saket`. Leave out members who have
   no account (their files go to `unclaimed/` in the owner's drive). If `tos` has no single active owner in the
   projection, add `org:tos,<uuid>`.
5. **Dry run**: `briefcase-migrate link-identities --file mapping.csv --dry-run` (same environment as the
   migrator). Read the report: lines that matched nothing, members without an account, where each space went and
   with how many accounts it is shared, renames. Fix the mapping and repeat until it is right.
6. **Apply**: the same without `--dry-run` (`--json` for the full report; it is also stored in
   `briefcase.identity_link_runs.report`). Expect, for production as surveyed: ~190 entries moved, the 79
   link-shared entries still link-shared, 30-ish identities linked. A corrected mapping can be applied by running
   again — it undoes the previous run first — until the service is opened; afterwards restore the backup instead.
7. **Settings**: add `BRIEFCASE_PROOF_FIRST_PARTY_ISSUERS=interface` next to `BRIEFCASE_PROOF_ISSUERS` (S4's
   value). Start the API and worker.
8. **Check**: as the owner, `GET /api/v1/drives` (own drive plus each Silicon looked after), `GET /api/v1/entries`
   (moved files at the top, `tos-public`, tag spaces, `unclaimed/`), an old link anonymously
   (`/org/tos/public/<a link-shared file>` → 200), a member's `GET /api/v1/shared`.

Switch together with Briefcase (in addition to part 1's list):

- **Interface**: Files works through proofs on `/api/v1/obo/*` with `interface` first-party (lists and reads in any
  drive the account reaches; `drive` names one). Proofs are accepted only on those routes — resolving an address
  at `/api/v1/paths/…` takes a bearer token — so Interface lists and reads through the obo routes. Its
  permanent-URL validator must accept `https://briefcase.teamofsilicons.com/{c:|si:id}/…` and keep accepting
  `/org/…`.
- **DM, Commit**: stored `/org/{org}/…` links keep working (aliases); their validators must also accept the new
  `/{owner id}/{path}` form for new attachments.
- **Waveform, Extend, Browser**: upload with reserve/transfer/commit (no `X-Org-ID` needed any more); they stay in
  `apps/{app}/` of the account's drive. Extend's invite step stays possible (`briefcase.invitations.create`) but
  the custodian already sees its Silicons' files.
- **The web** (later stage) serves `/{owner}/{path}` (ids contain `:`, so they never collide with its own routes;
  a uuid owner segment can appear while an owner's id is unknown) by calling `GET /api/v1/paths/…` through the BFF,
  follows a `308` by redirecting the browser to the body's `permanent_url`, and serves `/org/{org}/{path}` by
  calling the API's `/org/…` and redirecting to the entry's `permanent_url`.

Notes:

- After the switch, Silicons' custodians are learned lazily (S47); a Carbon may need to open its drive list once
  more for a Silicon that has not signed in since the cutover.
- `legacy_path_aliases` lives on; nothing expires it. Old ids redirect for as long as `account_id_history` keeps
  them (an id taken by someone else belongs to its new holder).
- The legacy organization rows (now empty structural folders, members, tags, contacts, idempotency records) stay;
  propose a cleanup migration only after the cutover is accepted.
