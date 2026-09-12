# Briefcase HTTP API

Official contract **1.0.0**, served below `https://backend.briefcase.teamofsilicons.com/api/v1/`. Use the [OpenAPI document](../../openapi.yaml) for complete request and response schemas and the [operation inventory](operations.md) for all 58 contracted operations. The [HTTP reference](reference.md) lists every method, authority, parameter, request field, and response. Human-facing links use `https://briefcase.teamofsilicons.com/org/{org_id}/{path}`.

## Authentication and negotiation

A normal request presents `Authorization: Bearer <IAM access token>` and `X-Org-ID: <organization>`. IAM owns identities, organizations, roles, tags and active membership. Briefcase uses the official `silicon-iam-client` 1.7.0 and verifies live IAM authorization. Missing or conflicting identity, organization, audience, role, tag, or testing-plane facts fail closed. Signed webhooks update local projections but do not replace request authentication.

`GET /iam` discovers the public application ID. `POST /auth/slt` exchanges a Briefcase-targeted short-lived IAM token; `POST /auth/refresh` rotates a refresh token. Both require a durable `Idempotency-Key`. `GET /auth/status` reports current identity and authorized organizations. IAM chooses the user's organization grants during login; a client-supplied organization cannot manufacture consent.

Negotiate before sending secrets: `GET /api/version` (host-root) with `Briefcase-Supported-API-Versions: v1`. The equivalent versioned path is `/version`. Check the service identity, selected major, and exact operation catalog. Versioned calls also enforce negotiation. See the [version policy](../version-policy.md).

The optional `X-Briefcase-App-Secret` header selects an IAM-paired test plane. It is an `ask_` test application secret and never replaces the actor bearer or OBO proof. See [testing environments](../testing-environments.md).

## Requests, retries and errors

Use JSON except for multipart uploads and streaming content. Never put credentials in a query string. The API rejects ambiguous security headers and wrong-plane credentials. Errors use the documented JSON error envelope and include a request ID. Hidden entries and nonexistent entries both return 404; do not infer an entry exists from a failed access check.

Supply an `Idempotency-Key` on create, upload, invitation writes, link updates, version/bin restore, storage validation, and environment mutations. Keep the exact key, body and resource across retries. Reusing a key for a different request returns conflict. A replay still checks current authorization. OBO operations instead bind a stable `operation_id` into a new single-use proof on every attempt.

| Status | Meaning |
| --- | --- |
| 400 / 422 | Invalid input or incompatible parameters |
| 401 | Missing, invalid, expired, or wrongly bound authentication |
| 403 | Visible resource, insufficient authority |
| 404 | Absent or hidden resource |
| 406 | No shared API major |
| 409 | Conflict, stale operation, or changed idempotent request |
| 410 | Retired API major |
| 413 | Request exceeds an applicable size limit |
| 416 | Unsatisfiable file byte range |
| 429 | Rate/capacity limit; honor Retry-After |
| 503 | IAM, storage, or another dependency unavailable |
| 507 | Organization or testing storage exhausted |

## Organization folders and app isolation

```text
public/
private/<carbon-or-silicon-id>/
<tag-folder>/
apps/<app-id>/public/
apps/<app-id>/private/<carbon-or-silicon-id>/
```

Public means readable within the organization. Anyone-with-link sharing is an independent explicit setting. Members may create content in the public container. Private roots remain hidden unless the viewer owns them or can traverse them to a shared descendant. Tag folders follow current IAM tags. An ancestor needed for traversal returns only safe navigation metadata and reachable descendants; access to one child never reveals its siblings.

The org base and private container do not accept file uploads. A new folder at the org base requires a public/private/tag boundary; nested folders inherit their parent's boundary. Reserved containers cannot be renamed, moved or deleted. Apps and app namespaces are materialized on first delegated use.

Every OBO action is restricted to `apps/<calling-app-id>/…`, even when the represented member is an organization owner. Inside that namespace the normal member permissions still apply. App origin metadata is attribution, not a substitute for path and user authorization. See [OBO](../obo.md).

## Listing, filtering and search

`GET /entries` lists the organization base or a `parent_id`, with `limit` from 1 to 100 and an opaque `cursor`. Responses contain `items` and `next_cursor`. Continue until the cursor is null, including after a short page. `GET /entries/{id}` reads an entry. The permanent path endpoint resolves by organization-relative path, including traversal folders. `PATCH /entries/{id}` renames or moves; `DELETE /entries/{id}` moves the authorized subtree to the bin.

The filter language applies only to authorized data. Quote a complete expression in shells:

```text
location:private/* is:md (contains:apple or contains:cat) last:5
is:file not is:archive permissions:update
```

| Predicate | Meaning |
| --- | --- |
| `is:file`, `is:folder`, `is:directory` | Entry kind |
| `is:image`, `video`, `document`, `spreadsheet`, `presentation`, `audio`, `archive`, `code`, `unsupported` | Renderer category |
| `is:md`, `is:pdf` | Exact extension fallback, up to 16 alphanumeric characters |
| `has:term` | Extracted document content |
| `contains:term` | Filename or extracted content |
| `name:term` | Filename only |
| `location:private/*` | Anchored path prefix and wildcard |
| `permissions:read`, `write`, `update`, `delete`, `manage_permissions` | The caller's effective capability |

Boolean expressions support implicit AND, `or`, `not`, leading `-`, and parentheses. `last:`, `first:` and `sort:` are top-level modifiers. Maximum expression length is 1,024 bytes, with 32 predicates and a take limit of 100. Date filters accept `between:DD-MM-YYYY=DD-MM-YYYY` (inclusive), `after:DD-MM-YYYY`, and `before:DD-MM-YYYY`. `from:@{ID}` selects creators, `to:@{ID}` explicit recipients, and `for:@{ID}` accessible-to members. `first:N` and `last:N` select chronological windows; `sort:oldest` / `sort:newest` choose ordering. Unsupported syntax returns validation errors.

`GET /search?q=…` returns at most 20 relevant visible files from names and extracted document text. Extraction is asynchronous; a new file's name is immediately available while extracted content may arrive later.

## Upload and download

`POST /uploads` is multipart form data with one `file` and exactly one destination: `parent_id` or `path`. The client submits one upload; the server chooses storage transfer internally. Files through **100 MiB inclusive** use a single S3 upload. Larger files use S3 multipart with:

```text
part_size = clamp(round_up_to_MiB(ceil(file_size / 1000)), 8 MiB, 5 GiB)
part_count = ceil(file_size / part_size)
```

The server stages and hashes incoming bytes, validates storage constraints, and publishes atomically. Uploading an existing file name updates that file ID in place; replacing content requires update authority independently of create authority. A same-name folder conflicts. All immutable versions remain retained.

`GET /entries/{id}/content` streams a file for preview; `GET /entries/{id}/download` downloads a file or a **tar.zst folder archive**. Folder archives contain only readable files and required traversal directories. Compression and S3 reads are backpressured; there is no whole-file or whole-archive buffer. Four simultaneous archives per API process bound compression and blocking-thread usage; additional requests receive a retryable capacity response. Disconnecting stops compression. An archive walks live state: concurrent deletion or revoked access can interrupt it. Treat any interrupted response as an incomplete download.

File delivery supports one byte range for seeking, returns 206 or 416 where appropriate, and stays on the Briefcase origin. Folder archives reject Range. Content is served with safe content disposition, no-store, nosniff, and a sandbox policy; provider URLs and storage credentials never reach clients. The web UI renders supported formats in its sandboxed preview pipeline and offers download for unsupported formats. Folder downloads stay compressed; opening an archive does not execute its contents.

## Invitations, links and logs

See [Sharing and audit logs](../sharing.md) for complete behavior, Postmark setup, email-directory limitations and examples.

- `GET/POST /entries/{id}/invitations`: member ID, verified email or dynamic IAM tag.
- `DELETE /entries/{id}/invitations/{grant_id}`: revoke a grant.
- `GET/PUT /entries/{id}/link-access`: inspect or set anonymous read/download access.
- `GET /public/{org}/{path}`: anonymous metadata, folder listing, inline file, or download.
- `GET /entries/{id}/logs`: preceding 365 days, paginated, including descendant folder changes.
- `GET /entries/{id}/activity`: latest 100 events.

The explicit member-grant endpoints `/entries/{id}/permissions` remain part of v1 for typed member-only callers. Invitations add email/tag resolution. Effective access can be inspected in bulk with `POST /permissions/effective`, for up to 100 entry IDs and/or paths. Delete and manage-permission capabilities belong to the creator or organization administration and cannot be granted through invitations.

The notification inbox returns the latest 20 notifications and an unread count. `POST /notifications/read` marks the entire inbox read. Durable email delivery is asynchronous; the permission grant does not depend on Postmark availability.

## Versions, bin and storage

`GET /entries/{id}/versions` returns up to 100 immutable versions and `next_cursor`. Every version reports its monotonic number, SHA-256, byte length, creator, timestamp and source. `POST /entries/{id}/versions/{version_id}/restore` copies that content into a new current version. It requires update authority, an idempotency key and quota capacity.

`GET /bin` is paginated. Files and folders remain recoverable for **45 days**. `POST /bin/{id}/restore` restores a deletion-batch root and its retained subtree atomically, rechecking current authority and destination constraints. The worker deletes provider objects only after the recovery window and durable cleanup checks; old active versions are never pruned by count.

Default per-organization limits are **100 GiB uploaded per UTC day** and **1 PiB total storage**. They are configurable per organization in PostgreSQL. `GET /usage` reports actual consumed and allowed bytes, including retained versions. Reservations and concurrent uploads count toward quota before publication. The daily window resets at midnight UTC.

Organization admins can `PUT /storage/configuration` with bucket, region, role ARN, prefix, account ID and encryption mode. Briefcase assumes the role and performs a temporary create/read/update/delete probe before activating the bucket. Use a new operation key after correcting a failed configuration. Static AWS access keys are not accepted from clients.

## Operational surfaces

Host-root `GET /healthz` checks liveness; `GET /readyz` checks configured database readiness. Host-root `POST /webhook/` receives IAM-signed events. Verify timestamp, signature/key version, exact raw body and event replay rules; see [IAM integration](../iam-integration.md). Internal S3 multipart operations, cleanup controls and worker leases are not public client APIs.
