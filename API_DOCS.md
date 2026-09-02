# Silicon Briefcase API documentation

This document explains every operation in the Silicon Briefcase OpenAPI contract. The machine-readable contract is in [`openapi.yaml`](./openapi.yaml).

## API conventions

### Base URL

```text
https://backend.briefcase.teamofsilicons.com/api/v1
```

Clean permanent URLs are served by the application host,
`https://briefcase.teamofsilicons.com/org/{org_id}/{path}`.

Briefcase is an organization-scoped filesystem for Carbons and Silicons. Every request is evaluated against the represented actor's organization membership and effective file permissions.

### Authentication

- **Bearer authentication:** An IAM access token for a Carbon or Silicon. This
  is the only credential the contracted API accepts.
- **OBO Access:** `X-IAM-OBO-Access-Proof` plus `X-App-ID`, accepted only by
  `POST /obo/files`. Presenting a proof anywhere else is a request error.
- **Organization context:** Authenticated operations require `X-Org-ID`.
- **Idempotency:** Creation and upload-finalization operations require `Idempotency-Key` so retries do not create duplicate resources.

Briefcase verifies a bearer online in two fail-closed steps. It sends the token
as form data to the configured IAM introspection endpoint using the Briefcase
application's HTTP Basic credentials and `X-Org-ID`, then sends that same token
to `/api/v1/oauth/userinfo` using Bearer authentication and the same
`X-Org-ID`. The responses must agree on `principal_id`/`sub`, actor type,
membership ID, and public organization handle. Introspection expiry is a Unix
integer. A current `org_role` and `tags` array are also required from userinfo;
missing authorization facts never fall back to webhook projections.

For OBO, Briefcase submits the published single-use binding — the canonical
method, the registered path, and the lowercase hexadecimal SHA-256 digest of
the exact body bytes it received — with HTTP Basic authentication and nothing
else: no `X-Org-ID`, no `Idempotency-Key`, and no retry, because IAM consumes
the proof exactly once and a retry is indistinguishable from a replay. A
successful result must bind the issuer, the audience (Briefcase), the actor
reference (`principal_id`, `type`, `public_id`), the organization, and the
registered endpoint path, which must equal the path Briefcase served.

The published result carries no role or tags, so Briefcase pairs it with its
own IAM membership projection. When that projection has not caught up yet, the
request runs with the least authority any member has — no tags and no
administrative access — which can only ever under-grant.

### IAM webhook receiver

`POST /webhook/` accepts IAM's published application-event envelope. It
requires `X-Silicon-IAM-Event-ID`, `X-Silicon-IAM-Timestamp`,
`X-Silicon-IAM-Key-Version`, and
`X-Silicon-IAM-Signature: v1=<64 lowercase hex characters>`. The signature is
HMAC-SHA-256 over `{timestamp}.{exact raw body bytes}`. Briefcase rejects an
unexpected key version, a timestamp outside the configured five-minute window,
duplicate header values, a signature mismatch, any envelope version other than
`1.0`, or a body `event_id` that differs from the signed header.

The published envelope has no top-level tenant handle and aggregate UUIDs are
not public organization identifiers. Every event delivered to Briefcase must
therefore include the public IAM organization handle as the string
`data.org_id`; an event without it is rejected instead of being routed by an
aggregate UUID.

### Entry model

Files and folders are both entries. Every entry has an owner, an
organization-relative `path`, the clean `permanent_url` built from it — always
carrying the organization, as `https://briefcase.teamofsilicons.com/org/{org_id}/{path}` —
a parent, effective access, timestamps, and one of three inherited root types:

- **Public:** Readable by every current organization member.
- **Private:** Visible only to its owner and explicitly authorized members.
- **Tag:** Everyone currently carrying the matching IAM tag may read, add, and
  change what is inside. Deletion is not shared: it stays with whoever created
  the entry and with organization owners and admins, so a shared tag folder
  cannot be emptied by anyone who happens to hold the tag.

A file additionally carries `render` — the renderer a client should open
(`image`, `video`, `document`, `spreadsheet`, `presentation`, `audio`,
`archive`, `code`, or `unsupported`) — and the authenticated `content_url` and
`download_url` for its bytes.

`effective_access` answers "what can I do here?" with independent labels:
`read`, `write` (add content that does not exist yet), `update` (change what
does), `delete`, and `manage_permissions`. Update never implies delete, and
write never implies update.

Organization owners and authorized administrators hold every operation on
every piece of content in their organization: read, write, update, delete, and
permission management, anywhere, without needing a grant. The reserved
containers themselves — the Public, Private, and Tag bases and the per-member
folders IAM keeps in step with membership — are structure rather than content,
so nobody renames, moves, deletes, or shares those, an administrator included. Anything the caller may not read is reported as `404`, never as a
permission error, so the API never confirms that a hidden entry exists.

### Errors

Errors use the standard `error.code`, `error.message`, and `error.request_id`
envelope. Two codes are specific to upload allowances:
`daily_upload_limit_exhausted` (`429`, with `Retry-After`) and
`organization_upload_limit_exhausted` (`507`).

## Browsing and folder management

### `GET /entries`

Lists folder contents, or filters everything the caller can reach.

- **Authentication:** Bearer.
- **Required header:** `X-Org-ID`.
- **Query:** Optional `parent_id` or `path`, optional `filter`, cursor, and limit (default and maximum 100).
- **Returns:** Permission-filtered entries, newest first, and `next_cursor`.

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

Any current member may create a folder at the organization base, declaring
which kind it is; it is their own space, so this needs no administrative
authority. Below the base the caller needs write access to the parent, and
invitees must already belong to the organization. Two destinations are always
refused: directly inside the Private container, and any folder assigned to
another member — the latter is reported as `404`.

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
between 8 MiB and 5 GiB, up to the 5 TiB maximum. The multipart session is
durable, so an interrupted transfer resumes rather than restarting. A file
beyond 5 TiB receives `413`.

Uploading a name an active file already carries is how that file is updated:
the bytes become its next version, the response is that same entry, and the
history keeps the previous fifty versions. Creating a file needs write access
on the folder; replacing one needs update access on the file itself. A folder
of the same name is a conflict.

Every upload is charged against two organization allowances: **100 GiB per UTC
day**, and **100 TiB in total**. Both count uploaded bytes rather than stored
bytes, so deleting a file frees storage but not allowance, and a restored
version costs nothing because nothing was uploaded. An upload that does not fit
is refused before its bytes are stored: a spent day answers `429` with
`Retry-After` set to the seconds remaining until 00:00 UTC, and a spent total
answers `507`. Concurrent uploads racing for the last of an allowance serialize
on the organization's counter, so the limit cannot be overshot.

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
active file already carries publishes that file's next version.

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

## Bin

### `GET /bin`

Lists deleted entries visible to the caller, newest deletion first.

- **Authentication:** Bearer.
- **Query:** Optional `cursor` and `limit` from 1 to 100.
- **Returns:** A page of deleted entries and the cursor for the next one.

The owner sees their recoverable entries. Administrators see entries allowed by administrative policy. Deleted entries remain recoverable for 45 days.

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
  -> POST /entries/{id}/access-requests with the rights wanted
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

## Contract gaps

These are deliberately absent rather than quietly invented. Each needs a
product decision before it can be added.

- Uploading a new version of an existing file is not defined; a new upload to
  the same folder is a new file, and version history grows only through restore.
- Permission listing and access-request listing are not paginated, and there is
  no endpoint to list pending requests awaiting the caller's decision.
- Folder-recursive permission changes have no explicit conflict rules beyond
  additive inheritance.
- There is no endpoint to permanently erase a bin item before its 45 days
  elapse.
- Multipart-upload expiry and cleanup happen in the worker but are not exposed.
- Search indexing status and unsupported-document behavior are not represented
  in a response.
- Listing an archive's contents without extracting it is a client concern:
  Briefcase reports `render: archive` and serves the bytes, and does not parse
  the container.
- IAM must guarantee that the bearer accepted by the configured introspection
  endpoint is also accepted by `/oauth/userinfo` with the same organization
  context, including current `org_role` and `tags` claims.
- IAM's OBO result carries no role or tags, so an application request derives
  them from Briefcase's membership projection and runs with the least authority
  when that projection is behind. An IAM endpoint that answers a member's
  current role and tags to an application credential would remove the gap.
- IAM's webhook event schemas must require a public organization handle at
  `data.org_id` for every event routed to Briefcase.
- BYO-S3 configuration read, remove, rotate-role, and retest endpoints are
  missing.
- File replacement, copy, bulk move, and bulk delete operations are missing.
- Malware scanning, quarantine, legal hold, storage quotas, and content safety
  are not defined.
