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

### S18. Errors carry exact reasons
New `AppError::Explained { status, code, message }` (constructors `unauthorized`, `refused`, `unprocessable`)
returns the precise reason and what to do, e.g. `token_expired`, `token_wrong_audience`, `token_revoked`,
`account_deleted`, `proof_scope_missing`, `unknown_account`. Token rejections are reworded for the caller (the
client library's hints address the verifying app). The error envelope shape is unchanged.

### S19. Uuids are text
`AccountUuid` accepts 3–32 ASCII letters and digits (Silicon Accounts issues ≤ 12 today), is never lowercased or
parsed as an RFC 4122 UUID, and every new column is `text COLLATE "C"`.
