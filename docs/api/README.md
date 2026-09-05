# Silicon Briefcase API documentation

This document explains every operation in the Silicon Briefcase OpenAPI contract. The machine-readable contract is in [`openapi.yaml`](../../openapi.yaml).

See also the [documentation index](../README.md), [testing-environment guide](../testing-environments.md), and [IAM integration runbook](../iam-integration.md).

## API conventions

The [operation map](operations.md) cross-references all 42 contracted operations
with their Rust methods and CLI commands.

### Base URL

```text
https://backend.briefcase.teamofsilicons.com/api/v1
```

Clean permanent URLs are served by the application host,
`https://briefcase.teamofsilicons.com/org/{org_id}/{path}`.

Briefcase is an organization-scoped filesystem for Carbons and Silicons. Every request is evaluated against the represented actor's organization membership and effective file permissions.

### Authentication

- **Bearer authentication:** An IAM access token for a Carbon or Silicon on ordinary member operations. Session exchange, root-key self-service, OBO, public health/version, and signed webhook routes have their own explicit authentication rules below.
- **OBO Access:** `X-IAM-OBO-Access-Proof` plus `X-App-ID`, accepted only by
  `POST /obo/files`. Presenting a proof anywhere else is a request error.
- **Organization context:** Authenticated operations require `X-Org-ID`.
- **Idempotency:** Creation and upload-finalization operations require `Idempotency-Key` so retries do not create duplicate resources.

Briefcase uses the official registry-published `silicon-iam-client` 1.2.0 for
all IAM operations, with runtime dependency auto-updates disabled. At startup
it performs IAM's mandatory `GET /api/version` handshake,
advertises support for `v1`, verifies the selected version in both the response
header and body, and pins every later IAM request to that major. For a bearer,
it posts the token to `/api/v1/oauth/introspect` using the Briefcase
Application's HTTP Basic credentials and `X-Org-ID`. It validates the active
token's expiry, `principal_id`, actor type, membership ID, authorization epoch,
and organization, then cross-binds the synchronous `authorization` snapshot
for public actor ID, role, canonical tag IDs/names, membership version and test
environment UUID. Missing or undisclosed authority fails closed. First use
does not require a webhook; there is no userinfo call. Complete snapshots update
the caller's local directory projection under the webhook reconciliation lock,
without rolling back newer membership versions or authorization epochs.

For OBO, Briefcase submits the published single-use binding — the canonical
method, the registered path, and the lowercase hexadecimal SHA-256 digest of
the exact body bytes it received — with HTTP Basic authentication and nothing
else: no `X-Org-ID`, no `Idempotency-Key`, and no retry, because IAM consumes
the proof exactly once and a retry is indistinguishable from a replay. A
successful result must bind the issuer, the audience (Briefcase), the actor
reference (`principal_id`, `type`, `public_id`), the organization, and the
registered endpoint path, which must equal the path Briefcase served.

The IAM 1.2 OBO result includes current delegated `authorization`, limited to
the parent token's scopes intersected with the audience's approved scopes.
Briefcase requires `roles.read` and `memberships.read` disclosure, cross-binds
the actor/organization/audience/environment, and uses that role and tag set
only for the exact verified request. It never adds authority from webhooks.

### Application login and refresh broker

Briefcase exposes two stateless session endpoints so first-party clients never
need the Briefcase Application secret:

| Method | Path | Body |
| --- | --- | --- |
| `POST` | `/auth/slt` | `{ "slt": "oac_…" }` |
| `POST` | `/auth/refresh` | `{ "refresh_token": "ort_…" }` |

Both require an `Idempotency-Key` of 16–255 visible ASCII bytes. Briefcase
forwards the key to IAM unchanged. The official SDK does not expose replay
response headers, so Briefcase does not emit `Idempotency-Replayed`. A
successful no-store response mirrors IAM's Application token response:
`access_token`, rotating `refresh_token`, `token_type`, `expires_in`, `scope`,
the represented actor (`principal_id`, `type`, and `public_id`), and nullable
`org_id`.

The SLT is the only credential that can start an Application login. Clients do
not submit an IAM password, OTP, Application ID, or Application secret to
Briefcase. Refresh tokens rotate on every success; serialize refresh per token
family, atomically replace the stored token, and retry an uncertain outcome
only with the exact same token and idempotency key. A rejected refresh is
terminal.

Adding a valid Briefcase `X-Testing-Environment-Key` selects the mapped IAM
test plane for either operation. Briefcase then uses that environment's stored
IAM root key and test-only Briefcase Application credential together. It never
falls back to production credentials, and production/test SLTs, access tokens,
and refresh tokens are mutually rejected.

### IAM webhook receiver

`POST https://backend.briefcase.teamofsilicons.com/webhook/` (outside `/api/v1`) accepts IAM's published application-event envelope. It
requires `X-Silicon-IAM-Event-ID`, `X-Silicon-IAM-Timestamp`,
`X-Silicon-IAM-Key-Version`, and
`X-Silicon-IAM-Signature: v1=<64 lowercase hex characters>`. The signature is
HMAC-SHA-256 over `{timestamp}.{exact raw body bytes}`. Briefcase rejects an
unexpected key version, a timestamp outside the configured five-minute window,
duplicate header values, a signature mismatch, any envelope version other than
`1.0`, or a body `event_id` that differs from the signed header.

Briefcase keeps a keyring indexed by `X-Silicon-IAM-Key-Version`, so the prior
secret can remain active during rotation. It projects IAM's scoped
`data.current` snapshots: the organization handle comes from
`current.organization.org_id` or a member's `organization.org_id`, while each
organization, tag, and membership uses its own resource version for ordering.
Known events without the required scoped identity fail closed. Authenticated
unknown event types are acknowledged and ignored for forward compatibility.

The same receiver also accepts IAM's signed testing-environment envelope,
`{"test":{"testing_key":"…","metadata":{…},"data":{…}}}`. It verifies the
HMAC over the exact outer bytes before parsing, then compares the authenticated
root key against active environments without timing leakage. Only the matched
environment UUID is used for routing; the root key and outer payload are never
logged or persisted. The receiver normalizes `test.metadata` plus `test.data`
to the production event shape and applies it in the separately configured
shared test database under the `<environment_uuid>:<public_org_id>` tenant.

### Briefcase testing environments

Testing environments are full Briefcase data planes backed by a shared test
database whose rows are isolated by environment ID and PostgreSQL row-level
security. Each environment is capped at 2 GiB, and a deployment permits at most
ten simultaneously active environments.

Lifecycle operations are always production-plane bearer operations. They
require `X-Org-ID`, and the organization in the URL must match it:

| Method | Path | Result |
| --- | --- | --- |
| `GET`, `POST` | `/organizations/{org_id}/testing-environments` | List or create |
| `GET`, `PATCH`, `DELETE` | `/organizations/{org_id}/testing-environments/{environment_id}` | Read, update, or retire |
| `GET` | `…/{environment_id}/key` | Retrieve the current Briefcase root key |
| `POST` | `…/{environment_id}/key-rotations` | Replace the key immediately |
| `POST` | `…/{environment_id}/iam-pairings` | Replace the paired IAM environment credentials |
| `POST` | `…/{environment_id}/cleanings` | Erase isolated data, retaining the environment |
| `POST` | `…/{environment_id}/restorations` | Restore before purge and issue a new key |
| `GET` | `/testing-environment` | Describe the environment selected by its key |
| `POST` | `/testing-environment/cleanings` | Clean using the key as sole authority |

All mutations require `Idempotency-Key`. `PATCH` additionally requires the
strong ETag returned by reads, in the exact form `If-Match: "{version}"`.
Secret-bearing create, key-read, rotation, restoration, and auth responses use
`Cache-Control: no-store`. The root-authorized self-description is always
`Cache-Control: private, no-store` even though its body contains only metadata.

Creation accepts `name`, optional `description`, and the already-created IAM
testing environment's `iam_environment_id`, `iam_environment_key`, canonical
test Briefcase `iam_app_id`, and fresh test-only `iam_app_secret`. The paired
IAM key and Application secret are encrypted at rest. Both are necessary:
IAM's environment key selects a plane but does not replace normal Application
authentication, and IAM rejects the production Briefcase secret in a test
plane. The test Application uses a different secret, but its canonical
Application ID must equal the Briefcase service Application ID configured by
this deployment. One IAM testing-environment UUID can be paired with only one
Briefcase environment at a time. A retry of a completed create is recovered from encrypted
idempotency state before the supplied IAM credentials are contacted again, and
only for the exact organization, actor, originating Application, request body,
and idempotency key that completed it.

If IAM rotates the environment root or test-only Application secret, the
creator or a current organization admin/owner can submit the complete
replacement tuple to `POST …/{environment_id}/iam-pairings`. Briefcase first
validates the replacement environment and Application in IAM, then atomically
replaces the encrypted UUID, environment key, Application ID, and Application
secret. The Briefcase root key and sandbox data remain intact. The operation is
idempotent, and advancing the control version fences requests that had loaded
the prior IAM credentials.

After an IAM organization projection exists in the Briefcase sandbox, the IAM
environment UUID cannot change. A different UUID returns HTTP409 with code
`testing_environment_iam_rebind_requires_new_environment`, leaving the pairing,
control version, and data unchanged. Create a new Briefcase sandbox for a
different IAM plane. Same-UUID root/Application-secret updates remain allowed;
changing UUID is also allowed before the first projection. Cleaning retains
the projection and therefore does not enable a different-plane switch.

Briefcase returns its own independent 32-character alphanumeric root key. That
key selects the Briefcase sandbox and is root authority for it, but ordinary
file operations still require a bearer or OBO proof issued inside the paired
IAM plane. Pass it as `X-Testing-Environment-Key` on ordinary API, auth-broker,
and OBO calls. Omitting it selects production; there is no cross-plane lookup
or fallback.

The creator and current organization admins/owners can read or rotate the key,
clean, retire, restore, and edit the environment. A key holder can inspect and
clean its own environment without a bearer. Cleaning erases content, versions,
permissions, activity, notifications, idempotency records, storage settings,
and consumption atomically while retaining the paired IAM identity projection,
so the next test-plane request can rebuild deterministic roots without waiting
for a new IAM event. Before source metadata is removed, exact provider delete
and multipart-abort descriptors are committed to the durable cleanup queue;
the worker performs and retries that physical work afterward. `erased_rows`
counts logically removed database rows. Retirement destroys the current key
immediately and keeps data recoverable for 30 days; restoration issues a new
key. An environment is automatically retired after thirty days without accepted
test-plane activity. Retirement starts a separate 30-day recovery window.

The ten-environment ceiling counts active environments only. Listings can also
include retained soft-deleted environments, so `status=all` may return more
than ten records during the 30-day recovery window.

### Entry model

Files and folders are both entries. Every entry has an owner, an
organization-relative `path`, the clean `permanent_url` built from it — always
carrying the organization, as `https://briefcase.teamofsilicons.com/org/{org_id}/{path}` —
a parent, effective access, timestamps, and one of three inherited root types:

- **Public:** Readable by every current organization member.
- **Private:** Visible only to its owner and explicitly authorized members.
- **Tag:** Matching tag members may read and add children. Updating or deleting another member's existing entry still requires explicit rights or organization-admin/owner authority; tag membership alone does not grant those operations.

A file additionally carries `render` — the renderer a client should open
(`image`, `video`, `document`, `spreadsheet`, `presentation`, `audio`,
`archive`, `code`, or `unsupported`) — and the authenticated `content_url` and
`download_url` for its bytes.

`effective_access` answers "what can I do here?" with independent labels:
`read`, `write` (add content that does not exist yet), `update` (change what
does), `delete`, and `manage_permissions`. Update never implies delete, and
write never implies update — so write on a folder adds new files to it and
never replaces a file that is already there, which is an update on that file.

The owner of a folder can always read what is inside it, including files
another member added through a grant. Reading is all that ownership of the
folder conveys: renaming, replacing, and deleting an entry stay with whoever
created it, so a shared folder cannot be quietly rewritten by the member who
happens to own it.

The reserved containers carry no `owner`. They are structure that IAM
reconciliation maintains, so no member is named as their proprietor.

Organization owners and authorized administrators hold every operation on
every piece of content in their organization: read, write, update, delete, and
permission management, anywhere, without needing a grant. The reserved
containers themselves — the Public, Private, and Tag bases and the per-member
folders IAM keeps in step with membership — are structure rather than content,
so nobody renames, moves, deletes, or shares those, an administrator included. Anything the caller may not read is reported as `404`, never as a
permission error, so the API never confirms that a hidden entry exists.

### Errors

Errors use the standard `error.code`, `error.message`, and `error.request_id`
envelope. Two codes are specific to organization limits:
`daily_upload_limit_exhausted` (`429`, with `Retry-After`) and
`storage_limit_exhausted` (`507`).

## Version and compatibility

### `GET /api/version`

Reports the API majors this build serves and every operation's contract
revision. Also served at `/api/v1/version`.

- **Authentication:** none; this is what a client reads before it has anything else.
- **Request header:** optional `Briefcase-Supported-API-Versions`, the client's majors newest first, comma separated.
- **Returns:** `service`, `selected_api_version`, `supported_api_versions`, `contract_version`, `build`, and `operations`.

The server selects the newest major both sides support, names it in
`Briefcase-API-Version`, and answers `406` when there is no overlap rather than
guessing. `Vary` names the request header, so a cache never serves one client's
selection to another.

Each operation carries a `version`, bumped whenever its request or response
shape changes observably; adding an operation leaves the others alone. A client
that checks its own operations against this list fails at startup instead of at
the first call that no longer means what it did.

### Public health endpoints

`GET /healthz` returns `{"status":"ok"}` when the API process serves requests. `GET /readyz` checks the production pool and, when configured, the test pool; success returns `{"status":"ready"}`. These are host-root paths, outside `/api/v1`, and require no credential. Readiness is not proof of successful webhook delivery, OBO registration, S3 CRUD, or a complete user workflow.

## Browsing and folder management

### `GET /entries`

Lists folder contents, or filters everything the caller can reach.

- **Authentication:** Bearer.
- **Required header:** `X-Org-ID`.
- **Query:** Optional `parent_id` or `path`, optional `filter`, cursor, and limit (default and maximum 100).
- **Returns:** Permission-filtered entries, newest first, and `next_cursor`.

Entries the caller may not see are filtered after the page is read, and the
page is refilled from the next position rather than answered short, so a full
page means what it says. A cursor that Briefcase did not issue is a request
error (`400`), never a conflict.

Without `filter`, the listing browses one level: the organization roots, or the
contents of `parent_id`/`path`. With `filter` and no parent, it searches every
entry the caller may reach, which is how a `location:` predicate selects a
subtree. Private folders belonging to other actors stay hidden unless at least
one descendant has been shared with the caller.

#### The filter language

Terms combine with `and` unless separated by `or`; `not` or a leading `-`
negates; parentheses group. A bare word is shorthand for `contains:`.

| Filter | Meaning |
| --- | --- |
| `last:N` / `first:N` | Take N entries chronologically, 1-100, in a single page |
| `sort:newest` / `sort:oldest` | Presentation order; newest first by default, so the oldest is last |
| `between:DD-MM-YYYY=DD-MM-YYYY` | Last changed within the range, both days inclusive |
| `after:DD-MM-YYYY` / `before:DD-MM-YYYY` | Last changed on or after / strictly before a day |
| `from:@{carbon:id}` | Created by that member |
| `to:@{silicon:id}` | Explicitly shared with that member |
| `for:@{id}` | Reachable by that member |
| `contains:'term'` | Name or extracted content matches; `*` is a wildcard |
| `has:'term'` | Extracted content matches |
| `name:'term'` | Name matches |
| `location:'private/cos:tos'` | Path prefix |
| `is:X` | `file`, `folder`, a renderer, or an extension such as `md` |
| `permissions:X` | The caller's own effective access: `read`, `write`, `update`, `delete`, `manage_permissions` |

The first segment of `@{...}` may name a principal kind; identifiers may
themselves contain colons, so only that first segment is ever consumed.
Filtering never returns anything the caller could not already see.

```text
GET /entries?filter=last:5 location:'private' (contains:'apple' or contains:'cat') is:md
```

### `POST /entries`

Creates a folder.

- **Authentication:** Bearer.
- **Required:** `name` and `Idempotency-Key`.
- **Optional:** `parent_id` or `parent_path`, `root_type`, `tag`, and initial invitees.
- **Returns:** The created folder entry.

`root_type` is required only when creating at the organization base. A tag root
also requires its IAM tag. Below an existing folder, the child inherits the
parent's root type and permission boundary.

The organization base holds exactly the reserved containers: Public, Private,
and one folder per tag. Declaring a kind at that level chooses which container
the new folder goes into — Public, the caller's own folder inside Private, or
that tag's folder — so a member's material always sits somewhere the folder
structure describes, and two members can use the same folder name without ever
meeting each other's. Any current member may do this; it is their own space, so
it needs no administrative authority. A tag the caller does not carry has no
container they can reach, and answers `404` exactly as a tag that does not
exist.

Below the base the caller needs write access to the parent, and invitees must
already belong to the organization. Two destinations are always refused:
directly inside the Private container, and any folder assigned to another
member — the latter is reported as `404`.

### `GET /entries/{entry_id}`

Returns metadata for one visible file or folder.

- **Authentication:** Bearer.
- **Returns:** Entry metadata and effective access.

Reading metadata must still require visibility. Possessing an entry ID or permanent URL does not grant access.

### `PATCH /entries/{entry_id}`

Renames or moves an entry.

- **Authentication:** Bearer.
- **Input:** New `name`, new `parent_id`, or both.
- **Returns:** Updated entry.

The caller needs write access to the entry and destination folder. Moving an entry across Public, Private, or Tag boundaries requires permission recalculation and must not accidentally broaden access.

### `DELETE /entries/{entry_id}`

Moves an entry to the bin.

- **Authentication:** Bearer.
- **Returns:** `204 No Content`.

Deletion is recoverable for 45 days and needs the `delete` right specifically;
being able to update an entry is not enough. Recursive folder deletion
preserves every descendant for recovery.

### `GET /entries/{entry_id}/content`

Streams the current file bytes for in-place rendering.

- **Authentication:** Bearer.
- **Optional header:** `Range`, as a single `bytes=` range.
- **Returns:** The bytes, or `206 Partial Content` for a range.

Briefcase relays the bytes itself rather than signing a provider URL, so every
read stays bound to a current IAM identity and a permanent URL never becomes a
bearer capability. Responses are hardened for untrusted content:
`Content-Security-Policy: sandbox; default-src 'none'; frame-ancestors 'none'`,
`X-Content-Type-Options: nosniff`, `Cache-Control: private, no-store`,
`Referrer-Policy: no-referrer`, and `Cross-Origin-Resource-Policy: same-origin`.
Range support is what lets a media player seek.

### `GET /entries/{entry_id}/download`

Streams the same bytes as an attachment, as `application/octet-stream`, for
anyone with read access.

### `GET /org/{org_id}/{path}`

Serves the clean permanent URL, for example
`/org/tos/private/cos:tos/top_secret/this_secret.md`.

- **Authentication:** Bearer.
- **Query:** Optional `disposition` of `inline` or `attachment`.
- **Returns:** The entry and its effective access, or the file bytes when a disposition is requested.

The organization segment must match `X-Org-ID`. Anything the caller cannot read
answers `404`, so the URL is safe to share: a recipient without access sees
exactly what they would see for a path that never existed, and can then request
access.

A folder shared only through something inside it resolves as a traversal view:
`visibility` is `traversal`, the folder opens, and it lists exactly the entries
the caller was given. Its owner, timestamps, and effective access are withheld,
because being able to walk to a share is not the same as being a member of the
folder. When nothing inside it remains accessible, the folder answers `404`
like anything else the caller may not see.

## Uploads

### `POST /uploads`

Uploads a file of any supported size in one request.

- **Authentication:** Bearer.
- **Content type:** `multipart/form-data`.
- **Required:** binary `file`, `Idempotency-Key`, and exactly one of `parent_id` or `path`.
- **Returns:** The created file entry, or the updated entry when the upload published a new version.

The destination folder is named either by identifier or by the same path its
permanent URL shows, which is how a client stores a file at an exact location.

There is one upload endpoint because a client should not have to know how large
"large" is. Briefcase stages the bytes, then decides internally: a file within
the single-request limit goes to storage in one call, and anything above it is
split into parts by the sizing formula, targeting roughly 1,000 parts clamped
between 8 MiB and 5 GiB, up to the 5 TiB maximum. The provider multipart session is durable for backend cleanup and reconciliation. There is no public resumable-upload session or part API; clients must not assume they can resume an interrupted HTTP body from an arbitrary byte offset. A file
beyond 5 TiB receives `413`.

Uploading a name an active file already carries is how that file is updated:
the bytes become its next version, the response is that same entry, and the
history keeps the previous fifty versions. Creating a file needs write access
on the folder; replacing one needs update access on the file itself. A folder
of the same name is a conflict.

Two organization limits apply: **100 GiB of uploads per UTC day** and **1 PiB
of storage**, both configurable per organization. The daily figure counts
uploaded bytes and returns at midnight UTC; the storage figure counts what is
currently kept, so deleting content returns capacity as soon as the bytes are
really gone — which is after the 45-day bin, not when the entry is binned.
Restoring a historical version uploads nothing but does store a second copy, so
it answers to the storage ceiling alone.

An upload that does not fit is refused before its bytes are stored: a spent day
answers `429` with `Retry-After` set to the seconds remaining until 00:00 UTC,
and a full organization answers `507`. Concurrent uploads racing for the last
of a limit serialize on the organization's counter row, so neither limit can be
overshot.

The two limits interact: a single file larger than the daily allowance can
never be uploaded, whatever the 5 TiB per-file maximum allows, because no day
has room for it.

Because the whole file arrives in one request, a very large upload occupies a
connection and temporary disk for its duration. `BRIEFCASE_UPLOAD_TIMEOUT_SECONDS`
and the staging volume are sized for the largest file an operator expects.

## Permissions and access requests

### `GET /entries/{entry_id}/permissions`

Lists explicit permission grants on an entry.

- **Authentication:** Bearer.
- **Returns:** Principal, conveyed rights, inheritance, grantor, and creation time.

This returns explicit grants, not every effective permission derived from Public, Tag, ownership, ancestry, or administrator status.

### `POST /entries/{entry_id}/permissions`

Grants another organization member access.

- **Authentication:** Bearer.
- **Input:** Principal, a non-empty set of `read`, `write`, `update`, and `delete`, and whether the grant inherits.
- **Returns:** Created permission grant.

Every set implicitly includes `read`. The rights are independent: granting
`update` does not allow deletion, and granting `write` does not allow renaming
or replacing what already exists.

Only the owner or an actor with permission-management authority may grant access. The principal must be a current Carbon or Silicon in the same organization.

Granting a principal who already holds a grant amends that grant in place and
returns it: the rights and inheritance become exactly what this request named.
There is no separate edit operation, and widening access never has to pass
through a revocation that would briefly remove it and tell the recipient their
access was taken away.

### `DELETE /entries/{entry_id}/permissions/{grant_id}`

Revokes an explicit permission grant.

- **Authentication:** Bearer.
- **Returns:** `204 No Content`.

Revocation removes that grant and any inherited access produced by it. Access derived independently from another grant, tag, Public visibility, ownership, or administrative rights remains.

### `POST /entries/{entry_id}/access-requests`

Requests read or write access to an entry.

- **Authentication:** Bearer.
- **Input:** The requested rights and an optional reason.
- **Returns:** Pending access request.

This is used when an authenticated organization member opens a permanent URL without access. The response should not reveal sensitive metadata beyond what is required to request access.

### `POST /access-requests`

Requests access using the organization-relative `path` copied from a permanent
URL, plus the same `access` and optional `reason` fields as the UUID route.

- **Authentication:** Bearer.
- **Required header:** `Idempotency-Key`.
- **Input:** Exact `path`, requested rights, and an optional reason.
- **Returns:** The pending access request only.

This is the deliberate request-access exception to normal path resolution. It
does not require existing read access and reveals no file or folder metadata.
A missing path and a path belonging to another authenticated tenant are both
reported as `404`, while the existing UUID-addressed route remains available
when a caller already knows the entry ID.

### `POST /access-requests/{request_id}/decision`

Approves or denies an access request.

- **Authentication:** Bearer.
- **Input:** `approve` or `deny`, plus the granted rights when approving.
- **Returns:** Updated request.

The owner or an authorized organization administrator can decide. A request
notifies the entry owner and every organization owner and admin; the decision
notifies the requester. Approval creates an explicit permission grant and
records the decision actor and time.

### `POST /permissions/effective`

Reports what the caller may do on named files and folders.

- **Authentication:** Bearer.
- **Input:** `entry_ids`, `paths`, or both, naming at least one and at most 100 targets.
- **Returns:** Effective access per readable target, plus the identifiers and paths that stayed unresolved.

A target that does not exist and one the caller cannot read are both reported as
unresolved, so a batch answer cannot be used to probe for hidden entries.

## Notifications

### `GET /notifications`

Reads the central notification inbox.

- **Authentication:** Bearer.
- **Returns:** The twenty newest notifications, newest first, and `unread_count` for the badge.

A notification is written in the same transaction as the change that caused it,
so the inbox can never claim access that was rolled back or miss access that
was committed. Kinds are `access_granted`, `access_revoked`,
`access_requested`, and `access_request_decided`. Each carries the acting
member, the rights involved, and a snapshot of the entry — name, path, kind, and
permanent URL — as it was at that moment, so the recipient can still read their
own history after losing access to it.

### `POST /notifications/read`

Marks the entire inbox read.

- **Authentication:** Bearer.
- **Returns:** The inbox afterwards, with `unread_count` at zero.

## History

### `GET /entries/{entry_id}/activity`

Reads the retained action history of one entry.

- **Authentication:** Bearer.
- **Returns:** Up to the last hundred recorded actions, newest first.

Each record names the stable action (`entry.file_created.v1`,
`entry.content_read.v1`, `entry.downloaded.v1`, `entry.metadata_updated.v1`,
`entry.subtree_deleted.v1`, `entry.subtree_restored.v1`, and so on), the actor
who performed it, the application that acted on their behalf when there was
one, and when it happened.

## Applications

### `POST /obo/files`

Creates a file for a member on behalf of another application. This is the only
operation Briefcase exposes to other applications.

- **Authentication:** `X-IAM-OBO-Access-Proof` and `X-App-ID`.
- **Content type:** `application/octet-stream`; the body is the raw file bytes.
- **Returns:** Created file entry.

Register the endpoint in IAM as `briefcase.files.create` at the path
`/api/v1/obo/files`, with metadata keys `path`, `name`, and `content_type`.
Exchange the proof over the SHA-256 digest of the exact bytes, then send those
bytes here. Because the destination travels as proof-bound metadata rather than
a header or query parameter, an application cannot redirect a proof it
legitimately obtained to another destination.

An empty `path` stores the file in the application's own folder,
`private/{actor}/apps/{app_id}`, created on first use and reserved from then
on. Any other path must name an existing folder the represented member may add
content to; their own permissions still decide. The proof identifier doubles as
the idempotency key. Any supported size is accepted here too, and a name an
active file already carries publishes that file's next version. The
organization's upload allowances apply exactly as they do to a member's own
upload.

## Search

### `GET /search`

Searches visible filenames and supported document contents.

- **Authentication:** Bearer.
- **Query:** `q` and optional limit from 1 to 20.
- **Returns:** Between zero and twenty ranked, permission-filtered results.

Filename matches rank first, then documents by how many content hits they have,
falling off from there. `content_hits` is the real number of matching
occurrences in the extracted text, not a flag. Authorization is applied again in
the search query, so permission changes do not rely on duplicating content into
ACL-specific indexes.

A filename is searched by the words inside it: `notes` finds `notes.md`,
because the separators a filename uses are split on both sides of the
comparison. Content search covers files that already are text — anything under
`text/*` plus the structured text types such as JSON, YAML, and XML — up to the
first megabyte of each. Other formats are stored and served identically and are
recorded as `unsupported` in the index rather than left waiting for an
extractor that does not exist yet.

## File versions

### `GET /entries/{entry_id}/versions`

Lists up to the last 50 versions of a file.

- **Authentication:** Bearer.
- **Returns:** Version ID, number, size, author, and time.

The caller must be able to read the current file. Historical versions inherit
the current entry's authorization boundary and do not carry independent ACLs.

### `POST /entries/{entry_id}/versions/{version_id}/restore`

Restores an older version.

- **Authentication:** Bearer.
- **Required header:** `Idempotency-Key`.
- **Returns:** Updated file entry.

Restoration does not erase later history. It copies the selected content into a new current version and records the restoring actor.
The v1 contract is synchronous. Deployments therefore give this route a
separate, configurable deadline and concurrency budget; large cross-target
restores stream one bounded multipart range at a time and may keep the request
open substantially longer than an ordinary upload.

## Usage

### `GET /usage`

Reports what the organization is actually consuming.

- **Authentication:** Bearer.
- **Returns:** Exact byte counts for storage and for today's uploads, each with its limit and what remains.

Every figure is a byte count, never a percentage, so a client renders whichever
unit or proportion it prefers. `storage.used_bytes` is what every retained
version currently weighs, binned entries included, because those bytes are
still stored; `daily_uploads.resets_at` is the next midnight UTC.

Limits default to 100 GiB of uploads per UTC day and 1 PiB of storage, and
either may be set for one organization by writing its row — there is no API for
raising a limit, deliberately, because that is an operator decision rather than
a tenant one:

```sql
INSERT INTO briefcase.organization_usage (org_id, daily_window, storage_limit_bytes)
VALUES ('tos', (clock_timestamp() AT TIME ZONE 'UTC')::date, 2251799813685248)
ON CONFLICT (org_id) DO UPDATE SET storage_limit_bytes = EXCLUDED.storage_limit_bytes;
```

A null limit means the platform default, so an organization that never asked
for anything special follows the default if it later changes.

## Bin

### `GET /bin`

Lists deleted entries visible to the caller, newest deletion first.

- **Authentication:** Bearer.
- **Query:** Optional `cursor` and `limit` from 1 to 100.
- **Returns:** A page of deleted entries and the cursor for the next one.

The owner sees their recoverable entries. Administrators see entries allowed by
administrative policy. Deleted entries remain recoverable for 45 days.

A binned entry still occupies the organization's storage, because its bytes are
still stored — that is what makes it recoverable. The space returns when the
entry is permanently discarded at the end of the 45 days, and `GET /usage`
reports the larger figure until then.

### `POST /bin/{entry_id}/restore`

Restores a deleted entry.

- **Authentication:** Bearer.
- **Returns:** Restored entry.

If the original parent no longer exists or cannot accept the entry, Briefcase
uses the actor's Private folder and chooses a deterministic collision-safe name.
A folder restore atomically restores its retained descendants and permission
structure. Once the persisted 45-day `purge_after` deadline has elapsed, the
entry is no longer restorable and is returned as not found.

## Organization storage

### `PUT /storage/configuration`

Configures an organization-provided S3 bucket.

- **Authentication:** Bearer token.
- **Caller:** Organization owner or authorized administrator.
- **Input:** Bucket, region, role ARN, prefix, AWS account, encryption mode, and optional KMS key.
- **Returns:** Configuration validation status.

Briefcase assumes the configured role and performs a temporary create, read, update, and delete test. The organization bucket becomes active only when all checks succeed. IAM credentials or static AWS secret keys must not be accepted.

## Complete flows

### Upload, of any size

```text
Name the destination by parent_id or path
  -> POST /uploads with the whole file
  -> Briefcase sizes it and picks single-request or multipart storage
  -> store the returned permanent URL
  -> render with GET /entries/{id}/content, save with /download
```

### Update a file

```text
POST /uploads to the same folder with the same file name
  -> the bytes become that file's next version
  -> the same entry and permanent URL come back
  -> GET /entries/{id}/versions lists the history, up to fifty versions
```

### Access request

```text
Open a permanent URL and receive 404
  -> keep the organization-relative path from that URL
  -> POST /access-requests with that path, the rights wanted, and Idempotency-Key
  -> owner and organization admins see it in their inbox
  -> owner/admin decides
  -> approval creates a grant and notifies the requester
  -> the entry now resolves at its permanent URL
```

### Application file creation

```text
Discover briefcase.files.create in IAM
  -> hash the exact bytes
  -> exchange a proof with {path, name, content_type} metadata
  -> POST /obo/files with the bytes
  -> Briefcase verifies once, then creates the file for the member
```
