# Briefcase migration decisions

The Carbon asked for the move from IAM and Honeycomb to Silicon Accounts and Silicon Apps and said not to ask
questions. These are the judgement calls made along the way, for review. The migration brief's decisions
D1–D9 and `apps/briefcase.md` are the starting point; entries here refine or apply them. Each stage appends.

## Service part 1 (identity), 2026-10-10

### S1. One credential per route
Every API route takes the caller's own Silicon Accounts access token (`Authorization: Bearer <JWT>`,
`aud = briefcase`, `iss = ACCOUNTS_URL`). The ten delegated `/api/v1/obo/*` routes take only
`Authorization: Proof sap_…`. A route given the other kind answers 401 with `proof_not_accepted_here` or
`proof_required`, so an app can never fall back to a user token or the reverse. The capability-only transfer
route keeps its upload capability and refuses any `Authorization` header.

### S2. Local verification, introspection only where revocation must be immediate
Tokens are verified locally with the cached JWKS (refetch on an unknown `kid` at most every 30 s, refresh after
1 h, keep serving stale keys if Silicon Accounts is unreachable). Introspection (answer reused ≤ 30 s) runs for:
grants, invitations, expiring-share changes and link access (`Action::GrantPermission`), revocations, delete /
trash, self-destruct keep (new `Action::KeepEntry`, previously `UpdateEntry`), version restore and storage
configuration — the decision file's sensitive list. Reads, lists, content and uploads do not introspect; they
still see webhook-fed revocations on every request (S6).

### S3. Configuration names and validation
`ACCOUNTS_URL` is the public origin and the exact issuer (trailing `/` ignored); `ACCOUNTS_API_URL` (optional)
is the server-to-server origin. This follows the stage spec rather than the survey's
`BRIEFCASE_ACCOUNTS_ISSUER`. Also `BRIEFCASE_APP_ID` (default `briefcase`, must be a bare app id),
`BRIEFCASE_APP_SECRET` (required, must start `sa_app_`), `BRIEFCASE_ACCOUNTS_WEBHOOK_SECRET` (required, must
start `whsec_`) with optional `BRIEFCASE_ACCOUNTS_WEBHOOK_PREVIOUS_SECRET` (both are tried, for rotations),
`BRIEFCASE_WEBHOOK_MAX_BODY_BYTES` (262144), `BRIEFCASE_ACCOUNTS_REQUEST_TIMEOUT_MS` (5000),
`BRIEFCASE_ACCOUNT_CACHE_TTL_SECONDS` (300, 1–86400). Plain http is accepted only for loopback hosts and
production requires https. The webhook secret is required at boot: a Briefcase that cannot verify deliveries
would silently miss sign-outs.

### S4. `BRIEFCASE_PROOF_ISSUERS`: per scope, deny by default
Format: entries separated by commas, semicolons or whitespace, each `scope=app` or `scope=app1|app2`; unknown
scopes and non-bare app ids fail at boot with the list of valid scopes. Unset = no app may act for anyone (the
brief's later "default empty = deny" wins over `_cross-app.md`'s "default: the issuers in this table"); the API
logs a warning at start when it is empty. Production value to configure at cutover (from the survey's cross-app
inventory; Browser and Interface are outside this batch but already call these routes):

```
briefcase.uploads.reserve=waveform|extend|browser|interface
briefcase.uploads.commit=waveform|extend|browser|interface
briefcase.uploads.status=waveform|extend|browser|interface
briefcase.uploads.cancel=waveform|extend|browser|interface
briefcase.entries.list=waveform|browser|interface
briefcase.files.read=waveform|extend|browser|interface
briefcase.folders.create=interface
briefcase.entries.trash=extend
briefcase.invitations.create=extend|interface
briefcase.link_access.update=interface
```

The scope strings are the old OBO endpoint ids (`domain::delegation::DELEGATED_SCOPES`); the legacy
`briefcase.files.create` (retired `/obo/files`, 410) is not a scope. Interface's wider access (everything the
user can reach, not only `apps/interface/`) is a part 3 decision; today every issuer is confined to
`apps/{issuer}/`.

### S5. Proof checks
A proof must be `user_verification` (an App verification proof names no account: 403
`proof_kind_not_accepted`), have `receiving_app = briefcase` (else 401 `proof_invalid`), carry the route's
scope (403 `proof_scope_missing`) and come from an allowed issuer (403 `proof_issuer_not_allowed`). The request
runs as `user.uuid` with `AuthenticationMode::OnBehalfOf{issuing app}`, so audit rows record the app. Proofs are
verified with Silicon Accounts on every call (no caching), as its docs advise. Proofs bind scopes, not request
bodies: IAM's exact-request binding is replaced by one scope per route, the issuer allow-list and the
`apps/{issuer}/` confinement.

### S6. Revocation semantics
- `membership.signed_out` with reason `app_revoked` is ignored (one Briefcase session ended itself, e.g. a CLI
  logout or the web's sign-out). Any other reason sets `revoked_before = occurred_at`.
- `membership.access_removed` sets `revoked_before` and `status = access_removed`; a later verified token
  (`iat` after the removal) reactivates the account, matching "signing into the app again restores".
- `account.deleted` sets `status = deleted` and anonymises the cache (display name, photo, email cleared);
  every token is then refused with `account_deleted`.
- A token with `iat <= revoked_before` is refused with 401 `token_revoked`, checked on every request.
- Proofs from other apps are refused (403) for accounts that are deleted or removed Briefcase's access (an app
  must not keep storing files for an account that turned Briefcase off), but not after a sign-out (the account
  still uses Briefcase; the proof's own validity is Silicon Accounts' call).
- A revocation event for an account Briefcase never saw creates a placeholder row (kind `carbon` until the first
  verified token corrects it), so the revocation still holds.
- Briefcase holds no proofs or sessions of its own (it issues no proofs and the service is stateless), so
  "end the app's stored proofs" has nothing to end; cached introspection answers are dropped.

### S7. Ordering of webhook events
Deliveries can arrive out of order. The public id keeps the newest `occurred_at` (`public_id_observed_at`), the
profile keeps the highest `account.version`, the custodian keeps the newest `occurred_at`
(`custodian_observed_at`). A token's `id` claim updates the public id only if the token was issued after the
last observation. Lookups count as observations at lookup time. Old ids go to `account_id_history` (for the part
3 URL redirects).

### S8. Webhook path and receipts
`POST /webhook/` is kept (the contract names it); IAM deliveries now get 401. Receipts go to the new
`accounts_webhook_receipts` (event id, type, account, occurred_at, outcome applied|stale|ignored), not to the
org-scoped IAM `webhook_receipts`. The `X-Accounts-Event-Id` header, when present, must equal the body's
`event_id` (400 otherwise). Status codes: 204 handled (including duplicates, unknown types and `ping`), 401
missing/bad signature or stale timestamp, 400 signed body that is not an event.

### S9. `account.deleted` and the account's files
D2/decision file: "delete the account's own space (respecting its BYO bucket) and remove its shares". The drive
model arrives in part 2, so part 1 records the deletion (status, anonymised cache, tokens refused) and leaves the
purge to part 2. Nothing is deleted yet.

### S10. Interim tenancy until drives (part 2)
A verified account acts in its own tenant: tenant key = account uuid (case-sensitive text), represented actor =
(kind, uuid), role `owner` (drive owners have owner-equivalent rights, per the decision file), no tags. The
existing repository bootstraps that tenant like a first IAM login did, with the legacy root layout. Pre-migration
org tenants are unreachable through the API until part 3's re-keying. `/org/{tenant}/{path}` resolves only the
caller's own tenant; `/api/v1/public/{tenant}/{path}` accepts account uuids or legacy org handles.

### S10a. An account never acts in a tenant it does not own
Account uuids are short base62 strings, so a lowercase uuid can spell a pre-migration organization handle (`tos`
is a valid uuid). Migration 0048 adds `organizations.owner_account_uuid`; a tenant created for an account records
it, and an account whose tenant key names a tenant recorded for anyone else (legacy organizations have NULL) is
refused with 404 and an `account_tenant_conflict` error log — it never becomes that organization's owner. Part 2's
drive model can rely on this column as the drive owner.

### S11. Recipients and ids
Callers name accounts by `c:`/`si:` id (trimmed, lowercased) or by uuid (3+ alphanumerics; a bare `bob` is
therefore treated as a uuid). Public ids move between accounts, so an id is always resolved with `lookup_by_id`
(local 600/min token bucket, 429 upstream mapped to `rate_limited`); the cache answers only when Silicon Accounts
is rate-limited or unreachable and it holds exactly one fresh account for that id (otherwise 503, never a guess). Errors are exact 422s: `invalid_account_id`,
`unknown_account`, `account_inactive` (deleted, unclaimed or pending custodian), `principal_kind_mismatch`.
Grants store the uuid. The `tag:` and `email:` recipient types are retired with exact 422s
(`tag_recipients_retired`, `email_recipients_retired`), per D2 and the decision file. "Silicons are not open to
the world" is enforced in part 2 together with the drive model (the circle helpers are ready).

### S12. Custodians
A Silicon's first verified token triggers a lookup to learn its custodian (best effort; failures are logged and
retried on the next lookup or webhook). Carbons never need a lookup for custody. `same_circle` follows D2 exactly
(Silicon S: S, its custodian and the custodian's other Silicons; Carbon C: C and its Silicons; an unknown
custodian fails closed).

### S13. `/api/v1/accounts` and `/api/v1/me`
`/api/v1/accounts` (public) returns `app_id`, `accounts_url` and the delegated scope catalogue; it replaces
`/api/v1/iam`. `/api/v1/me` returns `{uuid, id, kind, display_name, pfp_url, custodian {uuid, id} | null}` and
refreshes the cached profile from `userinfo` with the caller's token when stale; the email is stored only when
the token's scope includes `email`. The `/api/v1/auth/slt` and `/refresh` broker is removed: CLIs exchange SLTs
and refresh as public clients, the web BFF does the code exchange (D5, D7).

### S14. Delegated-upload identity keys
The NOT NULL `iam_organization_id/iam_principal_id/iam_membership_id` columns of `delegated_uploads` now hold
deterministic UUIDv8 keys (SHA-256 over tenant, account and membership), so reserve/commit/status/cancel only
match the same account and issuing app. No schema change in part 1; part 3 may add text columns. Reservations
made under IAM will not match after the cutover: drain them first (cutover.md).

### S15. Transfer route tenant selector
`PUT /api/v1/obo/uploads/{upload_id}/content` still reads `X-Org-ID`, now the uuid of the account the reservation
belongs to (the proof's user), validated as a tenant key; the capability, checked inside that tenant, authorizes
the bytes. Documented as `UploadTenant` in OpenAPI. Part 3 may replace it.

### S16. The Honeycomb testing plane is removed from code
Without IAM-owned test discovery it cannot select a sandbox, and D5 retires Honeycomb testing worlds. Routes,
store, worker test pool, `TestingEnvironmentContext`, the 2 GiB sandbox limit and telemetry test ids are gone.
All testing and Honeycomb tables and SQL functions stay (no destructive migration); `install_transaction_context`
clears `briefcase.testing_environment_id` explicitly. Legacy tag grants: no route creates them; `invite_tag` is
test-only to keep the legacy read/revoke paths covered until the cutover converts tag grants.

### S17. Contract 4.0.0
`info.version` and `CONTRACT_VERSION` are 4.0.0 (breaking identity change, per the brief's version rule; the
repo serves one `/api/v1` and negotiates per-operation revisions, so no `/api/v2`). The two new operations start
at 1.0.0. Other per-operation revisions are left for part 3, which changes paths and request shapes anyway.
`tests/openapi_contract.rs::official_client_consumes_the_exact_published_operations` is ignored until the client
stage updates `briefcase-client`.

### S17a. Service version 4.0.0
`docs/version-policy.md` pairs the backend version with the contract (3.0.0/3.0.0, 2.1.0/2.1.0), so the
`silicon-briefcase` package moves to 4.0.0 with contract 4.0.0 (the brief: 1.x/2.x/3.x → next major). The
compatibility matrix gains the 4.0.0 row; the client stage ships the matching 4.0.0 client and CLI.

### S18. Errors carry exact reasons
New `AppError::Explained { status, code, message }` (constructors `unauthorized`, `refused`, `unprocessable`)
returns the precise reason and what to do, e.g. `token_expired`, `token_wrong_audience`, `token_revoked`,
`account_deleted`, `proof_scope_missing`, `unknown_account`. Token rejections are reworded for the caller (the
client library's hints address the verifying app). The error envelope shape is unchanged.

### S19. Uuids are text
`AccountUuid` accepts 3–32 ASCII letters and digits (Silicon Accounts issues ≤ 12 today), is never lowercased or
parsed as an RFC 4122 UUID, and every new column is `text COLLATE "C"`.

## Service part 2 (drives and permissions), 2026-10-10

### S20. Every account's drive is the storage tenant (supersedes S10)
Each Carbon and Silicon owns exactly one drive, keyed by its Silicon Accounts uuid. The physical `org_id`
column keeps its name and holds that uuid, so every composite key, foreign key, row-security policy, usage row,
storage configuration and bin partition carries over. A drive is an `organizations` row whose
`owner_account_uuid` equals its key (CHECK `organizations_drive_key_is_owner`); pre-migration organizations keep
NULL and are never entered as drives (S10a). A drive comes into being on its owner's — or a custodian's — first
request, with one root folder (`system_kind = 'drive_root'`, stored path `.`); the paths of what sits at the top
start fresh (`notes/a.md`). Anyone else reaches only a drive that already exists and becomes a *participant* of it
(an `organization_members` row) on first use, because the actor columns of every row reference a participant.
Participation and `org_role` convey nothing any more; rights come only from S22.

### S21. Drive keys compare exactly without re-collating `org_id`
The decision file asks for the drive key as `text COLLATE "C"`. `org_id` keeps its declared `text` type with the
database's deterministic collation instead of an `ALTER … COLLATE "C"` across ~30 tables: under deterministic
collations equality and uniqueness are byte-exact (`'zQo' = 'zqo'` is false, checked on 5460), comparisons with
the `COLLATE "C"` account columns resolve to `"C"` (checked), and nothing orders or pattern-matches drive keys.
The re-collation would rebuild every composite key, foreign key and policy expression for no behavioural gain and
risks collation-mismatch errors in the policies. Every new identity column is `COLLATE "C"`.

### S22. Where rights come from
Exactly four sources, evaluated by `domain::permission::evaluate_authorization`:
1. **The drive.** Its owner, and the custodian of a Silicon that owns it, hold every operation on everything in
   it, including what grantees created there (their space, their quota). This replaces the org owner/admin god
   view; former admins get nothing implicit.
2. **Creating the entry.** The creator keeps every operation on it in any drive, and so does the custodian of a
   Silicon that created it (the custodian rule). After a custodian change the former custodian keeps only what it
   created itself (proved end to end).
3. **An explicit share** on the entry or an inheriting ancestor: read, write (add children), update (change what
   exists). Delete and permission management are never shareable.
4. **Creating a folder above it**, which conveys reading what others put inside (kept from before).
Nothing is open to "everyone": legacy Public and Tag containers convey nothing. System folders (a drive's root,
`apps/`, `apps/{app}/`) are structure — nobody renames, moves, deletes or shares them; only the drive's owner
rights add at the top; only Briefcase creates folders directly in `apps/`. A custodian acts as itself: what it
creates in a Silicon's drive is recorded as the custodian (proved), and an app acting through a proof never
carries the account's custodial powers.

### S23. How a request picks its drive
Routes that address an entry act in the drive holding it (`briefcase.entry_drive`). Listing, folder creation at a
path or the top, upload, the bin, usage, storage settings and path-based permission inspection take `drive`
(the owner's `c:`/`si:` id or uuid, resolved at Silicon Accounts; default: the caller's own). `drive` together
with `parent_id` is refused (`drive` / `drive_with_parent_id`): a folder names its drive. An unknown owner, or a
visitor naming a drive that was never used, answers 404. An upload without a destination goes to the top of the
drive (the old API required one; the top is now a real folder).

### S24. Apps act only in the proof user's own drive
A request authenticated by a proof never enters another drive, even for an entry shared with the user (404).
Waveform and Extend store in the account's own `apps/{app}/`. Whether the first-party Interface may act on
everything the user can reach is part 3's decision (S4).

### S25. My files, Shared with me, and the drives of a custodian's Silicons
`GET /api/v1/drives` lists the drives the caller opens with the owner's rights: its own (`own`) and, for a
Carbon, each Silicon it looks after (`custodial`), with the owner, root folder and permanent URL; listing creates
a custodial Silicon's drive if it was never used (the custodian may manage it). `GET /api/v1/shared` lists live,
unexpired shares to the caller on entries it can still see, newest first, one item per entry with the owner so
clients group by owner. Clients build **My files** from the own drive, **Shared with me** from `/shared`, and one
entry per custodial Silicon.

### S26. Silicons are not open to the world
New table `silicon_share_allowances(silicon_uuid, allowed_uuid, allowed_by_uuid)`. A share (grant, invitation,
folder invitee, delegated invitation) reaches a Silicon only from its custodian, the other Silicons its custodian
looks after (the circle, D2), or an account the Silicon or its custodian allowed with
`PUT /api/v1/silicons/{silicon}/allowed-accounts/{account}`; otherwise 403 `silicon_not_open` with what to do.
Only the Silicon itself or its custodian reads or changes the list (403 `not_silicons_custodian`); changing it
introspects like any sharing change. Removing an allowance keeps shares already made (they are revoked one by
one). Anyone signed in can share with a Carbon. User-facing copy says "the Silicons you look after" /
"its custodian and the custodian's other Silicons", never "circle".

### S27. Sharing
Immediate grants, no acceptance step. Only the entry's creator, the drive's owner and the custodian of either
may share. The recipient's notification goes into the recipient's own inbox, in the recipient's drive, through
`briefcase.deliver_notification` (which creates that drive's bookkeeping on first use and skips — logging — a
uuid that names a legacy organization). The invitation email goes only to an active account that shared its
`email` with Briefcase at sign-in. Notification subjects carry the entry's drive so links point at the right
drive.

### S28. Link access
Allowed on any file or folder that is not a system folder, managed by the creator, the drive's owner or the
custodian of either. The top of a drive, `apps/` and `apps/{app}/` never get a link (422 `protected_folder`) —
the decision file allows links "below drive roots and the apps/ container"; `apps/{app}/` is itself a system
folder, so links go on what is inside it.

### S29. Quotas are per drive
One `organization_usage` row per drive; defaults 100 GiB per UTC day and 1 PiB, overridable per drive in the
database (`UPDATE briefcase.organization_usage SET daily_upload_limit_bytes = …, storage_limit_bytes = … WHERE
org_id = '<uuid>'`). Uploads are charged to the drive they land in, whoever uploads them (tested: Bob's upload
into Ada's shared folder spends Ada's allowance, not his). `GET /api/v1/usage` needs the drive's owner rights
(404 otherwise). Recomputing `stored_bytes` for re-keyed legacy data belongs to part 3.

### S30. Bring-your-own storage is per drive
Configured by the drive's owner or the custodian of a Silicon that owns it (`drive` in the body; 403 otherwise).
Each configuration persists its STS External ID (immutable with the rest of the descriptor). Legacy rows are
backfilled with the exact organization-handle derivation, so customer trust policies keep working (upgrade path:
`tos` → `161cc38e…`, equal to an independent SHA-256); new configurations derive it from the drive key. The API and
the worker read the stored value (the worker's cleanup in a uuid drive no longer fails).

### S31. Search and filter scope
Search covers the caller's drive, its Silicons' drives and the 50 drives with the most recent live shares to it
(a bound on fan-out); each drive is searched under its own authorization, results are merged by filename match,
content hits and score (≤ 20), and each drive records its own access audit. Listing filters run within one drive;
`from:`/`to:`/`for:@id` name accounts by the uuid their id resolves to now (unknown id: 422 `unknown_account`).
There is no cross-drive "Recent" view; clients use `/shared` for what others shared.

### S32. The bin is per drive
The drive's owner and a custodian see everything in its bin; anyone else sees what they created there and what was
shared with them. A restored entry goes back to its folder, or to the top of the drive (owner rights only) when
the folder is gone or closed to the caller.

### S33. Entries move only within their drive
Bytes, versions and shares belong to a drive, so a move to a folder in another drive that the caller can see is
422 `cross_drive_move` (download and upload instead); a destination it cannot see is 404, so a move never confirms
that a hidden folder exists. The check reads the destination without recording an access.

### S34. `account.deleted` erases the account's own drive (refines S9)
In the webhook's transaction: every entry in the account's drive — including what grantees created there, which
the owner always could delete — goes into one deletion batch with `purge_after = now` (the worker purges the
objects, bring-your-own bucket included through the stored External ID), links end, shares in the drive end and
the inbox is cleared; every live share *to* the account ends in every drive (audited); the account leaves every
Silicon allow-list. What it created in other drives stays: it belongs to those drives. A Silicon deleted by its
custodian loses its drive the same way. Both SQL functions refuse unless the cache records the deletion.

### S35. Cross-drive questions go through narrow SECURITY DEFINER functions
`entry_drive`, `shared_with_principal`, `principal_drives`, `deliver_notification`,
`revoke_deleted_account_shares`, `erase_deleted_account_drive`; every request otherwise stays inside one drive
under forced row security. Production migrates as a role without BYPASSRLS, so the functions' owner reads
across drives through `drive_definer_lookup` SELECT policies created for the migrating role.
`tests/postgres_drive_roles.rs` proves the layout with a non-superuser, non-BYPASSRLS migrator and calls every
function as `briefcase_api`.

### S36. Operation revisions
Bumped where a request, response or refusal changed: `listEntries`, `createFolder`, `createInvitation`,
`readOrganizationUsage`, `configureOrganizationBucket` → 2.0.0; `updateEntry`, `uploadFile`,
`grantPermission`, `inspectEffectivePermissions`, `listNotifications`, `listBin` → 1.1.0; `setLinkAccess`,
`resolvePermanentUrl` → 1.2.0. New operations start at 1.0.0. Rights changes that keep an operation's shape
(who may delete, keep a self-destruct, restore) are covered by the contract major (4.0.0). The registry test now
compares each operation's method, path and revision with `openapi.yaml`.

### S37. Test harness
The API tests' AWS configuration has a behavior version and a clock, as a loaded production configuration does,
so the bring-your-own validation path runs (and fails without credentials) instead of panicking.
`aws-smithy-async` is a dev-dependency only (already in the lock graph). MinIO no longer publishes binaries
(HTTP 410 from dl.min.io), so uploads are tested through the content repository with a stored-object receipt, as
the existing versioning tests do.

## Service part 3 (delegated routes, addresses, the move), 2026-10-10

### S38. One scope per delegated route, unchanged paths and bodies
The ten `/api/v1/obo/*` routes keep their paths and JSON shapes, and each requires exactly one scope, its old
endpoint id (`domain::delegation::DELEGATED_SCOPES`): `briefcase.folders.create`, `briefcase.entries.list`,
`briefcase.files.read`, `briefcase.entries.trash`, `briefcase.uploads.reserve|commit|status|cancel`,
`briefcase.invitations.create`, `briefcase.link_access.update`. That is the whole old catalogue: the retired
`briefcase.files.create` (`POST /obo/files`, 410) is not a scope, so Waveform and Extend upload with
reserve/transfer/commit. The byte transfer has no scope (its capability authorizes it). `operation_id`
idempotency and the capability transfer are kept.

### S39. First-party apps reach what the account can (resolves S4/S24)
`BRIEFCASE_PROOF_FIRST_PARTY_ISSUERS` (default empty; production `interface`) names apps whose allowed scopes reach
everything the account can: its own drive outside `apps/{app}/`, entries shared with it in other drives, and the
drives of the Silicons it looks after (a first-party app acting for a Carbon carries the Carbon's custodial
powers, unlike every other app). It still uses only the delegated routes its scopes allow, needs an entry in
`BRIEFCASE_PROOF_ISSUERS` for each scope, and what it creates records the app. Other issuers stay confined to
`apps/{issuer}/` in the account's own drive, without custodial powers. `drive` in `POST /obo/entries/list` is
accepted only from first-party apps (others: 422 `drive_not_delegated`). Default empty keeps "deny by default".

### S40. An app trashes only what it created
`briefcase.entries.trash` is "move an app-created entry to the bin": the route answers 403 `not_created_by_app`
for an entry the issuing app did not create, whatever the account itself may delete (the subtree check already
refused mixed subtrees with 409). First-party apps included.

### S41. The byte transfer finds its drive
`PUT /obo/uploads/{upload_id}/content` no longer needs `X-Org-ID`: the drive comes from the reservation through
the definer function `briefcase.delegated_upload_drive` (migration 0050). A sent `X-Org-ID` is still honoured
(it must name that drive) so 3.x-era clients keep working; OpenAPI marks it optional and deprecated.

### S42. Permanent addresses name the drive by its owner's id
- Site: `https://briefcase.teamofsilicons.com/{owner id}/{path}` (`/c:ada/notes/plan.md`; the top is `/c:ada`).
  `permanent_url` in every response and the invitation email use it.
- API: `GET /api/v1/paths/{owner}/{path}` and `GET /api/v1/paths/{owner}` (the top), owner as `c:`/`si:` id or
  uuid. `content_url`/`download_url` name the drive by uuid (`/api/v1/paths/{uuid}/…?disposition=…`) so stored
  content addresses survive id changes. While Briefcase has not learned an owner's id the site address uses the
  uuid (rare: every drive owner has signed in or been looked up).
- Without `Authorization` an address serves only link-shared entries; the top of a drive is never public (401).
- Ids resolve cache-first: webhooks keep ids current, and a fresh single cached holder answers without a lookup
  (permanent addresses are opened far more often than recipients are named; S11's lookup-first rule stays for
  naming recipients). Otherwise Silicon Accounts is asked.
- An id nobody holds any more answers `308` to the same address under the owner's current id (from
  `account_id_history`), query kept, with `{moved_to, permanent_url, owner}` in the body. An id someone else took
  since belongs to its new holder (old links then resolve in that drive, normally 404) — the readable scheme the
  decision file chose accepts this.

### S43. Addresses from before personal drives keep resolving
`legacy_path_aliases(legacy_org, legacy_path) → (drive, entry_id)` holds every entry the cutover moved, every
2.0.0 identifier alias of those entries, and each structural folder whose contents moved (`private/c:saket` →
Saket's drive top, `public` → `tos-public`, …). `briefcase.resolve_legacy_location` takes the longest alias equal
to or above the requested path, so links to files created later inside a moved folder resolve too.
`GET /org/{org}/{path}` (deprecated `resolveLegacyPermanentUrl`) and `GET /api/v1/public/{org}/{path}` answer
directly (no redirect, so bearer clients and anonymous links both work) for the entry's current place, whose new
`permanent_url` is in the body; access is decided in the drive exactly as for the new address. A uuid-shaped
segment that is a drive is read as that drive first (S10a guarantees no drive key equals an organization key).

### S44. link-identities: what the command does
`briefcase-migrate link-identities --file mapping.csv [--dry-run] [--json]`, in one transaction with every
Briefcase table locked, row security and user triggers off and foreign keys deferred (restored exactly before
commit):
- Mapping: header `iam_principal_id,accounts_uuid[,kind][,accounts_id]`; the key is a legacy public id, an IAM
  principal UUID (matched through `iam_identity_bindings`, `organization_members.principal_id` and
  `delegated_uploads.iam_principal_id`, as the survey asked) or `org:{org}` (who receives that organization's
  shared spaces). One legacy identity ↔ one account (refused otherwise); rows matching nothing are reported, not
  guessed; an account uuid equal to an organization key is refused (S10a).
- Re-key: every account column of pre-migration organizations (an explicit list; the command refuses to run while
  the schema has an unlisted `X_type`/`X_id` pair). JSON snapshots in audit metadata and notification details keep
  the original text.
- Moves: `private/{m}/…` → m's drive top; `apps/{app}/private/{m}/…` → m's `apps/{app}/`; `public/…` →
  `{org}-public/`, `{tag}/…` → `{org}-{tag}/`, `apps/{app}/public/…` → `apps/{app}/{org}-public/` in the owner's
  drive; members without an account → `unclaimed/{legacy id}/` (apps content under `apps/{app}/` inside it), other
  stray content → `unclaimed/{org}/`. Creators keep ownership. Names that collide get ` (from {org})` (and
  `apps` at a drive's top is kept for apps). Entries keep their ids, versions, shares, logs, search documents,
  link sharing and bin state; structural folders stay in the organization (empty), so nothing is deleted. A
  container that was link-shared passes that to its new folder.
- The owner: `org:` if given, else the single active `owner` member, who must be linked (refused otherwise).
- Snapshot shares: each space is shared read+write, inheriting, with exactly the active members (with the tag, for
  tag spaces) who are linked; written directly (they record access members had, so the "Silicons are not open to
  the world" check does not apply). Tag shares become per-member shares with the same rights, inheritance and
  expiry, merged into an existing permanent share (one per account and entry, the schema's rule), and the tag share
  is kept, revoked. Removed members and members without an account get nothing (reported).
- Also: notifications move to the recipient's drive (inbox) with their subject's new drive and path; a drive
  holding versions stored in an organization's bucket gets a copy of that bucket's settings with the same External
  ID (the owner's drive takes over the active settings unless it has its own; other copies are `superseded`, read
  only); stored bytes are recomputed for every touched drive and organization and limit overrides move to the
  owner's drive; linked accounts Briefcase never saw are recorded (kind, id) as stale so the service looks them up;
  `identity_links` records every link.
- Before commit, every touched drive's paths and closure rows are recomputed from the parent links and compared;
  any difference aborts. The report lists links, unmatched lines, unlinked members, spaces with their share counts,
  unclaimed areas, renames and counts; the run row stores it.

### S45. Re-running: idempotent, and undone in place until the cutover
Each run records a ledger (`identity_link_ledger`) of what it created and the values it changed. The same mapping
again (same SHA-256) is a no-op ("already in place"). A different mapping first undoes the run in place — changes
and moves newest first, then the rows it created (a second pass, because removing a copied tag or participant
cascades) — and then applies the new mapping from the original state (tested: "A then B" leaves exactly the state
"B" leaves on a fresh copy). Undoing is refused once Briefcase has been used in the drives that run filled
(audit, entries, versions, shares, notifications newer than the run); then the backup taken before the run is the
way back. A dry run does everything and rolls back.

### S46. Archive listing
`GET /api/v1/entries/{id}/archive` (`listArchiveContents`) for anyone who may read the file (same rule and audit as
reading content). Formats by name, then media type: zip (central directory via ranged reads, zip64 aware, ≤ 64 MiB
directory), tar / tar.gz / tar.zst (streamed, ≤ 1 GiB of archive read), 7z (index read from a private temporary
copy, archives ≤ 512 MiB). At most 10,000 entries (`limit`), with `truncated` and `truncated_because`. Paths are
decoded leniently and only displayed; encrypted entries are flagged; an encrypted 7z index answers 422
`archive_encrypted`. The route has the upload deadline (it reads bytes before answering). New dependencies:
`flate2` (rust backend) and `sevenz-rust2` (no default features); `zip` and the 7z encoder are test-only.

### S47. Custodians find Silicons the cutover recorded
The cutover records Silicons without their custodian. `GET /api/v1/drives` looks up up to 10 such Silicons per call
(each at most once per cache lifetime), so a custodian sees its Silicons' drives before the Silicons sign in again;
their own first request, a lookup or `silicon.custodian_changed` teach it too. Found by the live cutover run.

### S48. Operation revisions (part 3)
`resolvePermanentUrl` 2.0.0 (now `/paths/{owner}/{path}`), `readPublicEntry` 2.0.0 (owner ids, 308),
`transferDelegatedUpload` 2.1.0, `listEntriesOnBehalfOfMember` 3.1.0, `trashEntryOnBehalfOfMember` 3.1.0; new
`resolveDriveTop`, `resolveLegacyPermanentUrl` (deprecated) and `listArchiveContents` at 1.0.0. The new address
format of `permanent_url`/`content_url` in every entry is covered by the contract major (4.0.0), like S36's
rights changes. 57 operations.
