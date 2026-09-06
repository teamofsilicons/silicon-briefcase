# Calling Briefcase on behalf of a member

This guide is for an Application that wants to read or write Silicon Briefcase
files **for a Carbon or Silicon it already represents**, without ever holding a
Briefcase credential of its own. It is self-contained: everything you need to
register, mint a proof, and make the call is here.

On-behalf-of (OBO) means IAM mints a proof for **one exact request**. Briefcase
verifies that proof with IAM, consumes it, and then acts as the represented
member — with that member's own permissions, never more. There is no long-lived
delegated key to store, leak, or revoke.

- Audience Application: `tos>briefcase`
- Briefcase API base: `https://backend.briefcase.teamofsilicons.com/api/v1`
- IAM base: `https://backend.iam.teamofsilicons.com/`

`X-Org-ID`, `org_id` and the `{org_id}` URL segment carry the **Team** ID; they
are wire names from the IAM contract, kept verbatim throughout this guide.

## Before you start

You need all five of these. Briefcase fails closed on any of them.

1. **Your Application is registered in IAM, in the same Team as Briefcase.**
   OBO never crosses a Team. IAM derives the Team from the two Applications and
   refuses to accept one from you.
2. **A subject token: the member's IAM access token, issued to *your*
   Application** (`oat_…`). A token issued to some other Application is refused.
3. **`obo.issue` on that subject token.** Without it IAM answers
   `403 obo_subject_token_forbidden` at exchange time.
4. **`roles.read` and `memberships.read` disclosure**, in both the subject token
   and Briefcase's currently approved scopes. Briefcase requires the delegated
   authorization snapshot and answers `403` when role or membership disclosure
   is missing. Never infer authority from an undisclosed (`null`) field.
5. **Your Application's own secret**, used only for HTTP Basic and the HMAC on
   the IAM exchange. It is never sent to Briefcase.

The member must be an active member of your Application's Team at verification
time. Ending a session does not by itself extend or revoke IAM authority.

## The shape of every call

Four steps, in this order, every time:

```text
1. Discover   GET  {iam}/api/v1/obo-access/applications/tos>briefcase/endpoints
2. Hash       body_sha256 = lowercase hex SHA-256 of the EXACT bytes you will send
3. Exchange   POST {iam}/api/v1/obo-access/exchanges   -> access_proof (obo_…)
4. Call       POST {briefcase}/obo/…  with those exact bytes + the proof
```

Briefcase then calls IAM's `verify` itself, which consumes the proof, and only
then executes. URL-encode the `>` in an Application ID inside a path.

### Exchange request

```json
{
  "subject_token": "oat_…",
  "audience": "tos>briefcase",
  "endpoint_id": "briefcase.folders.create",
  "metadata": {},
  "request": {
    "method": "POST",
    "body_sha256": "…64 lowercase hex chars…"
  }
}
```

Sent with HTTP Basic (your Application credential), an `Idempotency-Key`, and:

```http
X-OBO-Timestamp: <unix seconds>
X-OBO-Signature: <lowercase hex HMAC-SHA256 with your app secret over>
                 {timestamp}.{UPPERCASE_METHOD}.{registered_path}.{body_sha256}.{idempotency_key}
```

`registered_path` is the catalog path **including `/api/v1`**, exactly as
registered. The body itself never goes to IAM — only its digest. The bytes
travel straight to Briefcase, and the proof commits to what they will be.

### Calling Briefcase

```http
POST /api/v1/obo/folders/create
X-App-ID: tos>your-app
X-IAM-OBO-Access-Proof: obo_…
Content-Type: application/json
```

| Header | Rule |
| --- | --- |
| `X-App-ID` | Your canonical `{org_id}>{handle}`. Must equal IAM's `issuer_app_id`. |
| `X-IAM-OBO-Access-Proof` | The `access_proof` from the exchange; always starts `obo_`. |
| `X-Org-ID` | Optional. If sent, it must agree with the Team IAM reports. |
| `X-Testing-Environment-Key` | Sandbox only; see [Sandboxes](#sandboxes). |
| `Authorization` | **Never.** A bearer alongside a proof is `400 ambiguous_authentication`. |

## Two rules that break integrations

**Exact bytes.** Hash the serialized body once and send that same buffer. Do not
re-serialize between hashing and sending — a reordered key or a changed space
changes the digest, and Briefcase recomputes the digest from what it actually
received. The SDK sends `manifest.body_bytes()` unchanged for this reason.

**One proof, one request, no retry.** A proof is valid for one verification or
60 seconds, whichever comes first. IAM consumes it exactly once; a retry is
indistinguishable from a replay and is refused as one. Exempt these calls from
any automatic HTTP retry layer. To try again, mint a fresh proof.

## What Briefcase exposes

Register nothing yourself — this catalog is Briefcase's, and you discover it.
Metadata schemas are fixed: only the one-shot file endpoint takes metadata.

| Endpoint ID | Registered path (all `POST`) | Metadata | Body | Success |
| --- | --- | --- | --- | --- |
| `briefcase.files.create` | `/api/v1/obo/files` | `path`, `name`, `content_type` | raw bytes | `201` entry |
| `briefcase.folders.create` | `/api/v1/obo/folders/create` | `{}` | JSON | `201` entry |
| `briefcase.entries.list` | `/api/v1/obo/entries/list` | `{}` | JSON | `200` page |
| `briefcase.files.read` | `/api/v1/obo/files/read` | `{}` | JSON | `200`/`206` bytes |
| `briefcase.entries.trash` | `/api/v1/obo/entries/trash` | `{}` | JSON | `204` |
| `briefcase.uploads.reserve` | `/api/v1/obo/uploads/reserve` | `{}` | JSON | `200` reservation |
| `briefcase.uploads.commit` | `/api/v1/obo/uploads/commit` | `{}` | JSON | `200` status |
| `briefcase.uploads.status` | `/api/v1/obo/uploads/status` | `{}` | JSON | `200` status |
| `briefcase.uploads.cancel` | `/api/v1/obo/uploads/cancel` | `{}` | JSON | `200` status |

One route is deliberately **not** an OBO endpoint:
`PUT /api/v1/obo/uploads/{upload_id}/content` moves private bytes using a narrow
capability instead of a proof. See [Large or recoverable uploads](#large-or-recoverable-uploads).

An endpoint ID is never repointed at a different path, so a proof you obtained
for one operation can never be spent on another.

## Where your files go

An empty destination path selects the represented member's private Application
folder, `private/{actor}/apps/{app_id}`. It is created on first use and reserved
from then on, so each Application keeps its own space inside the member's own
storage. Any other path must name an existing folder the member may add content
to; their permissions still decide, and the Team's storage and daily upload
allowances apply exactly as they do to the member's own uploads.

Every created folder and file records the Application that acted, and appears in
the entry's history alongside the member who was represented.

## Operations

### Create a file in one shot

`POST /api/v1/obo/files`, `Content-Type: application/octet-stream`, body = the
raw file bytes. The destination travels as proof-bound **metadata**, not as a
header or query parameter, so a proof you legitimately obtained cannot be
redirected somewhere else.

```json
{ "path": "", "name": "report.pdf", "content_type": "application/pdf" }
```

Hash the file bytes, exchange with that metadata, then send the bytes. Any
supported size works (up to 5 TiB). A name an active file already carries
publishes that file's next version. The proof identifier doubles as the
idempotency key. Returns `201` with the created entry.

This is not a recoverable operation: after an uncertain response, a fresh proof
is a *new* attempt, not an idempotent retry. When you need durable recovery, use
the staged protocol below.

### Create a folder

`POST /api/v1/obo/folders/create`

```json
{
  "operation_id": "66a263a8-2dd7-42ea-aa88-3177f48ca6be",
  "parent_path": "",
  "name": "recordings"
}
```

One child name per call. `operation_id` is a non-nil UUID you generate and keep.
Returns `201` with the entry.

### List entries

`POST /api/v1/obo/entries/list`

```json
{ "path": "public/recordings", "filter": "type:file", "cursor": null, "limit": 100 }
```

All fields optional; `parent_id` and `path` are mutually exclusive. Without a
parent it lists roots, or searches the visible tree when a filter is present.
`limit` is 1–100, default 100. Filters and pagination are the ordinary member
ones, and results are permission-filtered. **Every page needs its own proof** —
the cursor is part of the signed body.

### Read a file

`POST /api/v1/obo/files/read`

```json
{ "entry_id": "…UUID…", "range": "bytes=0-1023", "download": true }
```

Range and disposition live in the signed body only; an HTTP `Range` header or
query parameter cannot override it. Malformed or multi-range syntax falls back
to the complete file, as on ordinary reads. Returns `200`/`206` with the bytes
under the normal sandboxed CSP, `nosniff`, and private no-store headers. A
different range is a different request: prepare and mint again.

### Move an entry to the bin

`POST /api/v1/obo/entries/trash`

```json
{ "operation_id": "…UUID…", "entry_id": "…UUID…" }
```

Returns `204`. An Application may trash only content **it** created, and the
represented member must have delete authority across the whole subtree. This is
the bin, not permanent deletion; normal retention and restore rules apply.
Missing and unreadable targets are deliberately indistinguishable.

## Large or recoverable uploads

Three phases: reserve a destination with a proof, transfer private bytes with a
capability, publish with a fresh proof. The transfer does not have to finish
inside the reservation proof's 60 seconds — only inside the reservation deadline.

**1. Reserve** — `POST /api/v1/obo/uploads/reserve`. Hash the whole file first.

```json
{
  "operation_id": "66a263a8-2dd7-42ea-aa88-3177f48ca6be",
  "parent_path": "",
  "name": "recording.webm",
  "content_type": "video/webm",
  "size": 0,
  "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
}
```

Returns `operation_id`, `upload_id`, `state`, `expires_at`, `published_entry_id`
and `capability`. Only an idle `reserved` operation returns a capability. The
resolved parent, destination, storage target and IAM identity are frozen, so a
later occupant of that name cannot inherit an old reservation's authority.

Reservations live at most 24 hours. A Team may hold at most 32 pending
reservations and 5 TiB of reserved bytes, further constrained by its current
allowances; a sandbox's aggregate limit is 2 GiB. Reserved capacity is charged
as an upload only when it is published.

**2. Transfer** — no proof, no `Authorization`, no `X-App-ID`:

```http
PUT /api/v1/obo/uploads/{upload_id}/content
X-Org-ID: <team>
X-Briefcase-Upload-Capability: <capability>
Content-Type: application/octet-stream
Content-Length: <exact reserved size>
```

Send no content encoding and no chunked transfer encoding. Briefcase verifies
the complete length and SHA-256 before writing; verified bytes reach `staged`
with no visible file and no upload charge yet. The capability is a secret that
can only stage bytes for this one reservation — it cannot read files, publish
anything, or act as a general credential. An almost-expired reservation is
rejected before any bytes are accepted.

**3. Commit** — `POST /api/v1/obo/uploads/commit` with a fresh proof over
`{"operation_id": …, "upload_id": …}`. Commit rechecks identity, destination
rights and quota, then publishes and charges quota in one transaction, returning
`committed` and the real `published_entry_id`. A repeated successful logical
commit does not create a second version.

`briefcase.uploads.status` and `briefcase.uploads.cancel` take
`{"operation_id": …}` with their own fresh proofs. They stay available to the
same member/Application/plane even after destination access is lost, and never
return a capability or file bytes. Cancellation never deletes a committed file —
use trash for that.

| State | What to do next |
| --- | --- |
| `reserved` | A fresh reserve can issue a capability; then transfer. |
| `receiving` | Reconcile later; never start a second writer. |
| `staged` | Get a fresh commit proof and commit. |
| `committed` | Keep `published_entry_id`; do not re-upload. |
| `cleanup_pending` | Keep the operation record while cleanup reconciles. |
| `cancelled`, `expired` | This operation cannot publish; start a new logical operation. |

## Retrying safely

`operation_id` is your logical mutation identity for folder creation, trash, and
the upload lifecycle. After an uncertain response:

1. Keep `operation_id` and the body **unchanged**.
2. Mint a **fresh** proof — never replay the consumed one.
3. Send the same bytes again.

Briefcase scopes mutation results by Team, represented member, originating
Application and operation, and rechecks current authorization before any logical
replay. An old successful trash cannot delete an entry that has been restored
since. For uploads, reconcile with `status` before retrying rather than inferring
publication from a completed transfer.

Never persist a proof or an authorization snapshot as a durable grant. An outbox
may keep manifests, operation and upload IDs, and state — nothing else.

## Errors

Every error is `{"error": {"code": …, "message": …, "request_id": …}}`.

| Status | Typical code | Meaning and what to do |
| --- | --- | --- |
| `400` | `ambiguous_authentication` | You sent `Authorization` with a proof. Send only the proof. |
| `400` | `invalid_app_id`, `invalid_org_id` | Use the canonical `{org_id}>{handle}` and a valid Team ID. |
| `401` | `unauthenticated` | Proof missing, malformed, expired, already consumed, or refused by IAM. **Do not retry** — mint a new proof. |
| `403` | `forbidden` | A binding failed (issuer ≠ `X-App-ID`, wrong audience, path or Team mismatch, wrong endpoint), authorization was undisclosed, or the member lacks the permission. |
| `404` | `not_found` | Target missing or invisible to the member — deliberately indistinguishable. Do not retry unchanged. |
| `409` | varies | A conflicting state; read the code before acting. |
| `413` | — | Body above the accepted size. |
| `416` | — | Range not satisfiable. |
| `422` | `invalid_obo_metadata`, `invalid_name`, `invalid_path` | The proof-bound metadata or the body failed validation. Re-read the catalog and compare what you actually sent. |
| `429` / `507` | — | Rate limited, or the Team's storage allowance is exhausted. |
| `503` | — | IAM or a dependency is unavailable. This is *not* a refused proof; that is `401`. |

At the IAM exchange, expect `403` (Team mismatch, inactive membership, missing
`obo.issue`), `409` (proof consumed, or an idempotency key reused with different
input), `410 proof_expired` (more than 60 seconds elapsed), and `422` (metadata
does not satisfy the declared schema).

## Sandboxes

A Briefcase testing environment is a full replica paired with one IAM test
plane. To use OBO there, send the Briefcase root key for the plane paired with
the proof's IAM environment:

```http
X-Testing-Environment-Key: <32-character Briefcase root key>
```

Use the same key throughout a flow, including the raw transfer. The root key
does not replace the proof, and a proof from the wrong plane cannot fall back to
production. Register the endpoint catalog before testing OBO. Full setup is in
the [testing-environment guide](testing-environments.md).

## Rust

Use the official `briefcase-client` package. It never sends your bearer on these
calls, never retries them, and never stores a session.

```rust
use briefcase_client::{ApplicationId, DelegatedCreateFolder, OboProof};

let manifest = DelegatedCreateFolder {
    operation_id,                // keep this with the unchanged logical request
    parent_path: String::new(),  // the member's private Application folder
    name: "recordings".into(),
}.prepare()?;

// Bind exactly these when you exchange: manifest.endpoint_id(), manifest.method(),
// manifest.path(), manifest.body_sha256(), and empty metadata {}.
let folder = client.create_folder_on_behalf_of(
    &ApplicationId::new("tos>your-app")?,
    OboProof::new(fresh_proof)?,
    &manifest,
).await?;
```

`DelegatedListEntries`, `DelegatedReadFile` and `DelegatedTrashEntry` share the
same `prepare()` interface; reads return a `ContentStream`. For staged uploads:
`DelegatedReserveUpload::file(...)` (hashes with bounded memory) →
`reserve_delegated_upload` → `transfer_delegated_upload` →
`DelegatedCommitUpload` + `commit_delegated_upload`, with
`delegated_upload_status` and `cancel_delegated_upload` for recovery. The
one-shot path is `create_file_on_behalf_of(&OnBehalfOfUpload::file(...))`.

`OboProof` and `UploadCapability` are redacted, non-cloneable, non-serializable,
and consumed by the call. Keep the source file unchanged between hashing and
transfer, and use a real file — not a symlink, pipe or device.

## CLI

Handy for trying an operation before you write code. Prepare and describe the
request locally, take those binding values to IAM, then send with the proof:

```bash
briefcase app request folder-create --body folder.json --describe
briefcase app request folder-create --body folder.json --app-id 'tos>your-app'

briefcase app prepare-upload ./recording.webm --operation-id "$OPERATION_ID" > reserve.json
briefcase app request upload-reserve --body reserve.json --app-id 'tos>your-app' \
  --capability-file ./upload.cap
briefcase app transfer "$UPLOAD_ID" ./recording.webm --capability-file ./upload.cap
```

`--describe` is entirely local: it prints the canonical `body`, `method`, `path`,
`endpoint_id`, `body_sha256` and metadata without contacting anything. Operations
are `folder-create`, `entries-list`, `file-read`, `entry-trash`, `upload-reserve`,
`upload-commit`, `upload-status`, `upload-cancel`, plus `app upload` for the
one-shot path. Pass proofs through the hidden prompt or `--proof-stdin`, never
`--proof` in a shared shell.

## Checklist

- [ ] Same Team as Briefcase; subject token issued to your Application.
- [ ] `obo.issue`, `roles.read`, `memberships.read` present.
- [ ] Body serialized **once**; digest taken over those exact bytes.
- [ ] Registered path bound with its `/api/v1` prefix.
- [ ] Exchange immediately before the call; no proof cached or reused.
- [ ] `X-App-ID` + `X-IAM-OBO-Access-Proof` only — no `Authorization`.
- [ ] Automatic HTTP retries disabled for these routes.
- [ ] `operation_id` retained for mutations; a fresh proof for every attempt.
- [ ] Capability files kept private and deleted when the upload finishes.

## Reference

- [API reference](api/README.md) — every operation, filters, errors, limits
- [IAM integration](iam-integration.md) — registration, scopes, webhook approval
- [Rust client](client/README.md) · [CLI](cli/README.md) · [Testing environments](testing-environments.md)
