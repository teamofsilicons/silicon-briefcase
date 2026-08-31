# Silicon Briefcase engineering decisions

This is the append-only decision log for the Briefcase backend. Material
architecture, security, data-model, API, and operational decisions are recorded
before or alongside their implementation. A changed decision is superseded by a
new record; accepted history is not silently rewritten.

## D-001 — PostgreSQL is the metadata authority

**Status:** Accepted

PostgreSQL 16 or newer is authoritative for entries, tree relationships,
permissions, access requests, content-version metadata, multipart state,
organization storage configuration, idempotency records, audit events, webhook
receipts, and the transactional outbox. Constraints and transactions enforce
tenant and lifecycle invariants. Object bytes do not live in PostgreSQL.

Organization-qualified keys and row-level security are defense in depth. The
application still includes `org_id` in every query; RLS does not replace explicit
authorization.

## D-002 — Object bytes live in S3-compatible storage

**Status:** Accepted

The platform S3 bucket is the default. Every managed key begins with a normalized
organization segment and an opaque entry/version identifier; user-controlled
names never become storage paths. An organization may instead configure its own
bucket through an assumed IAM role. Static customer AWS credentials are never
accepted or stored.

Storage access is behind an application port. Metadata is committed only after a
storage operation succeeds, and compensation/cleanup is queued when a later
database operation fails.

## D-003 — Modular monolith with three process roles

**Status:** Accepted

One Rust package provides a library and thin `briefcase-api`,
`briefcase-worker`, and `briefcase-migrate` binaries. Domain policy, application
use cases, adapters, and HTTP transport remain separate modules. API and worker
processes can scale independently without introducing distributed transactions
between premature internal microservices.

## D-004 — Rust quality and safety baseline

**Status:** Accepted

The service uses Rust 1.98, edition 2024, a committed lockfile, rustfmt, strict
Clippy lints, `#![forbid(unsafe_code)]`, and no `unwrap`, `expect`, `todo`, or
`unimplemented` in production paths. Axum/Tokio/Tower provide HTTP and async
execution; SQLx provides explicit PostgreSQL access; the official AWS SDK
provides S3 and STS integration.

## D-005 — IAM is consulted online and authorization fails closed

**Status:** Accepted

Bearer tokens are introspected with IAM on every protected request. OBO proofs
are consumed by IAM with the Briefcase application credentials and are bound to
the expected audience, action, resource, actor, organization, application, and
expiry. Briefcase then independently evaluates entry permissions. IAM timeout,
an incomplete response, inactive membership, or an organization mismatch denies
the request; authorization is never restored from stale webhook data.

Webhook events accelerate reconciliation and invalidate local projections, but
do not replace online authorization. The IAM OBO endpoint and response schema
are absent from IAM's current OpenAPI contract, so the adapter is isolated and
its expected wire contract is documented and contract-tested.

## D-006 — Effective access is an additive capability lattice

**Status:** Accepted clarification

Public and matching-tag boundaries derive `read`. Ownership and organization
owner/admin status derive `read`, `write`, `delete`, and `manage_permissions`.
An explicit `read` grant derives only `read`; an explicit `write` grant derives
`read`, `write`, and `delete`. Only owners and organization owner/admin actors
manage permissions. There are no deny grants in v1. Inheritable ancestor grants
flow to descendants, while non-inheritable grants apply only to their entry.

Internally, `write` is decomposed into `create_child`, `update_metadata`, and
`write_content` so policy can grant only the operation a workflow requires. The
public response continues to expose the contracted four-value effective-access
model.

`write` covers creating children, renaming, moving, and restoring content.
Public membership alone permits creating a child in a Public folder, matching
the explicit "anyone can upload" product rule, but does not permit modifying or
deleting another actor's existing child.

## D-007 — Traversal visibility does not imply content access

**Status:** Accepted

An actor may see an otherwise-private ancestor when a visible descendant is
needed to traverse the tree. That synthetic visibility grants metadata needed
for navigation only and never grants file content, version, download, mutation,
or permission access. Direct lookups of hidden resources return opaque `404`
responses to avoid existence disclosure.

## D-008 — System roots are reconciled, reserved, and immutable

**Status:** Accepted clarification

Each organization has one system Public container, one system Private
container, one root for each current IAM tag, and one private actor folder for
each current member. IAM events and an idempotent reconciliation job materialize
them. First use may lazily ensure the calling actor's required roots so an event
delay does not produce an empty filesystem.

System roots have internal kinds not exposed by the v1 entry schema. They cannot
be renamed, moved, deleted, or granted directly. User-created root entries are
still supported by `POST /entries`; their names do not determine their boundary.
Because an organization root has no parent from which a member can derive
`create_child`, only current IAM owners and administrators may create these
additional root entries.

## D-009 — Trees are tenant-local and cycle-free

**Status:** Accepted

Entries use UUIDv7 identifiers. A parent and child must share an organization;
sibling names are unique by exact Unicode code-point sequence among non-deleted
entries. Names are trimmed, 1–255 bytes, exclude NUL and `/`, and cannot be `.`
or `..`. Database recursive checks reject cycles.

Children inherit `root_type` and tag from their parent. Because the v1 PATCH
contract has no explicit acknowledgement for a potentially access-broadening
move, cross-boundary moves are rejected. A future explicit operation may safely
add atomic reclassification.

## D-010 — Mutations are idempotent beyond the minimum contract

**Status:** Accepted

All externally initiated mutations accept or require an idempotency key, even
where the current OpenAPI document omits it. Records are scoped by organization,
represented actor, originating application, operation, and key. The canonical
request digest must match on replay; key reuse with different input is a
conflict. Completed responses are replayed, and abandoned in-progress records
expire through worker cleanup.

Required headers remain exactly as published in OpenAPI. For operations where
the header is not yet declared, its use is optional until the contract is
updated.

## D-011 — Audit and outbox changes are transactional

**Status:** Accepted

Every create, metadata access, mutation, delete, restore, and download-url action
records actor, organization, originating application, action, entry, request ID,
and timestamp. Audit payloads contain no object data or credentials. The latest
100 events per entry are retained, as required by the product brief.

Domain changes, audit records, idempotency results, and outbox events commit in
one transaction. Workers claim outbox jobs with leases and `FOR UPDATE SKIP
LOCKED`, retry with bounded exponential backoff and jitter, and retain terminal
failure information for operations.

## D-012 — Upload bodies are streamed through bounded temporary files

**Status:** Accepted

The published API sends small files and multipart parts through Briefcase rather
than directly to S3. Axum bodies are streamed to private temporary files with
incremental byte counts and SHA-256 checksums, then the AWS SDK streams those
files to S3. File data is never aggregated into process memory. JSON and upload
routes have separate body limits and deadlines.

Small upload accepts at most 100 MiB inclusive. Multipart is required above that
threshold and permits objects through 5 TiB, subject to S3's part constraints.

## D-013 — The written multipart formula is canonical

**Status:** Accepted clarification

Part size follows the written algorithm: ceil size over 1,000 target parts,
round up to a whole MiB, then clamp to 8 MiB through 5 GiB. Part count is ceil
file size over that result. This makes a 1 TiB upload use 1,049 MiB parts and
1,000 parts. The product document's 1 TiB/1 GiB/1,024-row example conflicts with
its formula and is treated as illustrative rather than normative.

Multipart sessions expire after 24 hours. Completion verifies the exact ordered
part-number/ETag set and declared byte total before publishing an entry.

## D-014 — Current entry permission governs all retained versions

**Status:** Accepted clarification

Historical versions do not have independent ACLs. Current read access allows
version listing; current write access is required to restore. Restore copies the
selected object into a new monotonically numbered current version and never
rewrites history. The worker retains the newest 50 versions and removes older
objects after their metadata is safely retired.

The current API has no operation for uploading replacement content, so initial
upload and restore are the only v1 version creators. No undocumented replacement
route is invented.

## D-015 — Deletion preserves a recoverable subtree for 45 days

**Status:** Accepted clarification

Deleting an entry marks its complete active subtree with one deletion batch and
a purge time 45 days later. Grants, versions, and original parent relationships
remain intact. Restore is atomic for the retained subtree. If the original
parent no longer exists or cannot accept the entry, restore falls back to the
actor's private folder; a conflicting sibling name receives a deterministic
` (restored <short-id>)` suffix.

Restore is accepted only while the database's authoritative `purge_after` is
still in the future. The deleted root and its subtree are locked before the
deadline can race retention scheduling; once the 45-day window has elapsed,
the entry is no longer recoverable through the API and fails closed as not
found.

Only owners, explicit writers, and organization owner/admin actors see eligible
bin entries. Permanent early erasure is not exposed because it is absent from
the contract.

## D-016 — Search is PostgreSQL FTS first and permission-filtered at query time

**Status:** Accepted

PostgreSQL full-text search is the initial search implementation behind a port.
Filename matches are ranked ahead of extracted-content hits, with at most 20
results. Authorization is joined and evaluated during the search query; an index
document is never treated as authorization, preventing stale grants from
leaking content.

UTF-8 text is indexed directly. Rich-document extraction is an asynchronous
provider seam; unsupported formats and extraction failures leave filename
search working and expose no partial content. Moving to a dedicated search
engine requires equivalent query-time permission enforcement.

## D-017 — OBO origin is server-derived and more restrictive

**Status:** Accepted

`origin_app_id` comes only from an IAM-verified `X-App-ID`; request-body `app_id`
is advisory and must match. The represented Carbon or Silicon remains the entry
owner. An OBO application cannot delete any entry or subtree it did not create,
even when the represented actor otherwise has delete access.

Automatic routing into `Private/{actor}/apps/{app_id}` occurs only when a future
contract permits omitting the destination. The current upload and folder APIs
have explicit parent semantics, so Briefcase validates the supplied parent and
records origin without silently changing it.

## D-018 — Temporary delivery URLs are short-lived S3/CDN signatures

**Status:** Accepted

A temporary URL is issued only after fresh IAM validation and current Briefcase
read authorization. It expires in 12 hours and is never stored as an attachment
identity. The stable permanent URL is derived from the entry UUID and public
Briefcase base URL.

The current contract does not define the authenticated permanent-content route
or CDN signing protocol. The URL builder and storage presigner remain adapter
boundaries until that contract is added; metadata endpoints do not accidentally
serve object bytes.

## D-019 — BYO S3 activates only after destructive-safe validation

**Status:** Accepted

Briefcase assumes the configured role using its workload identity, verifies the
expected AWS account, and tests create, read, overwrite, and delete under a
random validation key inside the configured prefix. The organization switches
to that location only after cleanup succeeds. SSE-S3 and SSE-KMS are supported;
SSE-KMS requires a key ARN. Failed validation retains the prior active storage
location and records a redacted reason.

## D-020 — Webhooks are signed, deduplicated projections

**Status:** Accepted interim contract

`POST /webhook/` accepts versioned IAM events with event ID, aggregate version,
timestamp, type, organization, and a minimal payload. Requests require an HMAC
SHA-256 signature over timestamp plus raw body, use a five-minute replay window,
and are deduplicated by event ID. A repeated ID is a duplicate only when the
tenant and exact signed-body SHA-256 match the retained receipt; altered bytes
or an RLS-hidden cross-tenant collision fail with the same opaque conflict.
Receipt aggregate type and UUID come directly from the signed envelope rather
than being inferred from payload shape. Stale aggregate versions do not
overwrite newer projections, and membership actors prefer IAM's public handle
over legacy principal UUID fields.

IAM's published contract does not yet specify delivery signatures or schemas.
The endpoint therefore lives behind a versioned adapter and will be aligned when
IAM publishes the authoritative webhook contract.

## D-021 — Contract gaps remain visible

**Status:** Accepted

`openapi.yaml` remains the public v1 route authority and route coverage is
tested. Product-required behavior with no safe wire contract—content
replacement, permanent authenticated content, previews, access-request inbox,
audit listing, early purge, and full BYO-S3 lifecycle—is represented by internal
ports/state where needed but not exposed through invented public endpoints.
Closing a gap requires updating OpenAPI, API documentation, this log, and code in
the same change.

## D-022 — Security controls not defined by product are deployment gates

**Status:** Accepted

Malware scanning, quarantine policy, quotas, legal holds, and content-safety
classification are absent from the product and API contracts. The storage and
publication workflow retains seams for them, but production enablement must not
claim those controls exist. A deployment risk review must either add their
contracts or explicitly accept the exposure before public upload traffic.

## D-023 — Database migrations are an explicit release step

**Status:** Accepted

Forward SQLx migrations are embedded for verification but are executed only by
`briefcase-migrate` with a separately configurable database principal. API and
worker replicas never migrate on startup. Production schema changes follow
expand, backfill, observe, and contract phases so mixed application versions can
run safely during a rollout.

## D-024 — Storage locations are immutable per content version

**Status:** Accepted

Every file version records the storage-configuration revision, opaque object
key, provider version identifier, byte count, and checksum associated with its
bytes. Activating or replacing an organization BYO bucket affects new versions
only. Existing content remains readable from its original location until an
explicit, audited migration moves and verifies it; configuration rotation never
silently strands old versions.

## D-025 — OBO actions use a route-stable vocabulary

**Status:** Accepted interim contract

Briefcase asks IAM to consume proofs for these exact actions:
`briefcase.entries.list`, `briefcase.folder.create`,
`briefcase.entry.read`, `briefcase.entry.update`, `briefcase.entry.delete`,
`briefcase.file.temporary_url`, `briefcase.file.upload`,
`briefcase.multipart.initiate`, `briefcase.multipart.upload_part`,
`briefcase.multipart.complete`, `briefcase.multipart.abort`,
`briefcase.permissions.list`, `briefcase.permissions.grant`,
`briefcase.permissions.revoke`, `briefcase.access_request.create`,
`briefcase.access_request.decide`, `briefcase.search`,
`briefcase.versions.list`, `briefcase.versions.restore`,
`briefcase.bin.list`, `briefcase.bin.restore`, and
`briefcase.storage.configure`.

Entry, upload, request, and version operations bind the relevant path UUID as
the proof resource. List/search bind the organization ID, while folder creation
and upload bind the resolved parent ID. These names remain adapter-local until
IAM publishes its OBO action registry; any rename requires coordinated IAM and
Briefcase deployment.

## D-026 — Traversal-only entries remain structurally redacted

**Status:** Accepted clarification

Traversal visibility from D-007 is represented internally by a dedicated view
containing only the folder ID, parent, name, and inherited root type. It is not
coerced into the full `Entry` response with fabricated values, and owner,
timestamps, origin, content metadata, or effective access are not leaked.

The current `GET /entries` OpenAPI response permits only full `Entry` objects.
Until that contract adds an explicit redacted traversal variant, the HTTP
handler omits traversal-only nodes. This leaves traversal to some directly
shared descendants incomplete in v1, but preserves the stronger non-disclosure
invariant. Exposing those ancestors requires an OpenAPI and API documentation
change rather than an undocumented response shape.

## D-027 — External IAM identifiers use one bounded opaque-text invariant

**Status:** Accepted clarification

Actor, organization, application, and tag identifiers remain opaque,
case-sensitive IAM values. Each must contain 1 through 255 UTF-8 bytes, with no
leading or trailing whitespace and no Unicode control characters. Briefcase
does not apply type-specific syntax until IAM publishes stronger identifier
contracts.

The shared bound matches persistence columns and keeps request metadata and
structured logs safe from unbounded or line-forging values. Validation happens
at every deserialization and adapter boundary rather than relying only on a
database constraint.

## D-028 — Organization storage configuration is bearer-only

**Status:** Accepted contract resolution

`API_DOCS.md` explicitly requires a bearer token for
`PUT /storage/configuration`, while the OpenAPI document inherits the global
bearer-or-OBO alternatives. The operation-specific prose is treated as the
narrower authority: Briefcase rejects OBO credentials for this endpoint and
still requires the represented actor to be a current organization owner or
administrator. This prevents an application proof from changing the
organization's durable storage trust boundary.

The next contract revision should express this override directly on the
OpenAPI operation so generated clients and security review see the same rule.

## D-029 — Storage tenant namespaces and External IDs are server-derived

**Status:** Accepted

IAM organization identifiers are opaque and may contain characters meaningful
to an object-store path. Platform S3 targets therefore use
`<configured-prefix>/<sha256(org-id)>`; the lowercase hexadecimal digest is the
only tenant segment. Raw organization identifiers and user-controlled path
syntax are never interpolated into platform object keys.

For organization-owned buckets, Briefcase supplies an STS External ID derived
as `silicon-briefcase:<sha256(org-id)>`. Clients cannot provide or override it.
The value is unique per organization, stable across configuration rotation,
and used as confused-deputy protection rather than as a secret.

## D-030 — The worker is privileged, leased, and failure-honest

**Status:** Accepted interim contract

The worker uses a database principal distinct from API replicas and refuses to
start unless its effective PostgreSQL role is a superuser or has `BYPASSRLS`.
Cross-tenant outbox claims use bounded leases and `FOR UPDATE SKIP LOCKED`.
Failures receive deterministic jitter over capped exponential delays, and the
configured maximum attempt transitions the event to retained `dead_letter`
state with only a stable redacted error code.

The only currently produced topic, `briefcase.domain-events.v1`, implies an
external delivery stream but no transport, acknowledgement, or consumer
contract is configured. The worker records `external_dispatch_unconfigured`
and retries it rather than falsely marking it delivered. Unknown topics use
`unsupported_outbox_topic` under the same retry and dead-letter policy.

Until external cleanup contracts exist, maintenance is limited to safe local
work: reconciling filename search projections and deleting expired in-progress
idempotency records. Expired multipart uploads and retained version objects due
for purge are counted for operations telemetry but are not mutated because
correct completion requires provider-side abort or object deletion. Adding
those workflows requires an object-store cleanup port with durable outcome
recording and retry semantics.

## D-031 — Content checksums preserve provider semantics

**Status:** Accepted clarification

Briefcase never treats an S3 `ETag` as a content digest and never invents a
full-object SHA-256 from multipart part hashes. Small uploads calculate SHA-256
while staging, send the exact value with `PutObject`, and persist it as a
provider-verified `SHA256/FULL_OBJECT` checksum only after S3 accepts the write.

Multipart sessions explicitly request `SHA256/COMPOSITE`. Every part is hashed
while staging, the digest is sent with `UploadPart`, and the ordered digest is
sent again with that part during completion. Publication requires the final
object size and a provider-returned `SHA256/COMPOSITE` checksum. Version storage
therefore records checksum algorithm, checksum type, and provider-encoded value
instead of assuming every version owns a raw 32-byte full-object SHA-256.

Same-target restores ask S3 to calculate SHA-256 for the copied object.
Cross-target restores download with provider checksum validation, calculate a
fresh full-object SHA-256 over the staged bytes, and require the destination to
verify that digest. Any missing or contradictory integrity evidence fails
closed before metadata publication.

## D-032 — Direct S3 delivery is the secure interim URL behavior

**Status:** Accepted interim contract

The current object-store adapter issues a twelve-hour S3-presigned `GetObject`
URL after fresh IAM and Briefcase authorization. Briefcase does not replace its
host with `BRIEFCASE_CDN_BASE_URL`, because doing so would invalidate the AWS
signature and produce a URL that cannot be trusted to work.

Serving temporary URLs from `cdn.briefcase.teamofsilicons.com` requires an
explicit CDN origin, signing-key, cache, revocation, and organization-owned
bucket contract plus validated deployment configuration. Until that contract
lands, the API returns the valid direct S3 URL and preserves the configured
12-hour expiry; the CDN base setting remains inactive rather than implying an
unsigned or signature-invalid delivery path.

## D-033 — Runtime database principals are process-specific

**Status:** Accepted

API replicas and the cross-tenant worker never share a database credential.
The API reads `BRIEFCASE_DATABASE_URL` and refuses to start when its effective
role is a superuser or has `BYPASSRLS`, making PostgreSQL row-level security a
real defense in depth. The worker reads the separately scoped
`BRIEFCASE_WORKER_DATABASE_URL` and refuses to start unless its role is a
superuser or has `BYPASSRLS`, because leased outbox and retention work must scan
organizations without accepting a caller-provided tenant context.

The migrator retains a third credential and is the only runtime allowed to own
or evolve the schema. Local Docker provisioning creates canonical restricted
API and privileged worker roles; the final migration removes default public
schema access and grants those roles only data-plane table, sequence, and
function privileges. Production may use different role names, but deployment
must apply equivalent grants and the startup capability checks remain
mandatory.

## D-034 — Multipart completion is reconciled, never compensated blindly

**Status:** Accepted

Completing an S3 multipart upload and publishing PostgreSQL metadata cannot be
one atomic transaction. Briefcase therefore persists a `completing` state and
the idempotency record's reserved entry ID before calling S3. An expired-lease
retry reuses that same ID and may resume `completing`; it never allocates a
second file identity.

If S3 completion returns an ambiguous error, Briefcase performs a checksum-
enabled `HeadObject` against the upload's unique immutable key. Matching size,
`SHA256/COMPOSITE` type, and part-count suffix are sufficient to finish the
database publication. An absent or unavailable object leaves the durable state
and lease for later reconciliation; Briefcase does not reset the upload to an
uploadable state, delete the idempotency record, or delete a possibly completed
object. This converts the provider-to-database failure window into a retryable
state instead of an orphan or duplicate publication.

## D-035 — Organization system-root ownership is custodial

**Status:** Accepted clarification

The entries schema requires every folder to reference a current or historical
organization member as owner and creator. Public, Private, and Tag system roots
therefore retain a member as their persistence custodian (using a freshly
verified caller when available, otherwise preferring a current owner or
administrator and permitting a historical member when none remain), and that
field is not an authorization source for
organization-scoped system containers. Public access comes from the Public
boundary, Tag access comes from the caller's current IAM tags, and every current
member receives read-only navigation access to the Private container. Only the
reconciler creates member folders directly beneath Private; each such actor root
is owned by its represented Carbon or Silicon and may contain that actor's
private entries.

API requests first run a read-only consistency preflight covering the caller's
active role and order-independent current IAM tag-name set, the singleton roots,
every active projected member and tag, and the current tag-root name. A
consistent organization stays off the advisory-lock hot path. A mismatch is
rechecked under an organization-scoped transaction advisory lock before caller
projection and reconciliation; root-level metadata mutations use the same lock
so a user name cannot race a system-root rename. Online caller refreshes retain
the webhook aggregate-version watermark while applying the freshly verified
role, active membership, and de-duplicated tag set, so stale events remain stale
and the next higher event can still apply. IAM tag names first resolve existing
projected rows; a missing name receives a version-zero name-as-ID projection,
while a collision with a newer webhook-owned identity fails closed.
IAM-projection transactions acquire the lock before writing their projection
rows. Reconciliation then uses the schema's system-identity uniqueness
constraints plus conflict-safe inserts after online-IAM caller projection and
after accepted IAM events, materializing missing Public, Private, active-tag,
and active-member roots for both pre-existing and newly observed organizations.
Canonical display names are used when available; a bounded collision-safe name
is used only when an older user entry already occupies that sibling name. An
active tag rename reconciles its existing system root under the same lock and
uses the same deterministic collision candidates; this internal projection
update does not weaken the public API's system-root immutability. Root identity
and authorization always derive from `system_kind`, boundary, tag, and owner
fields rather than display text.

Before creating or renaming any root, the locked reconciler validates every
existing Public, Private, Tag, and actor root as a live folder with its exact
reserved identity, boundary, parent, tag, and custodial-member relationship,
and without file, originating-application, or deletion metadata.
The read-only consistency preflight and active-tag lookup apply the same shape.
An existing reserved identity with any contradictory shape fails closed with
the single internal `malformed existing system root` integrity error. Briefcase
does not reparent, undelete, replace, or otherwise mutate that root or the user
tree beneath it; recovery requires an explicit operator-reviewed repair.

Tag removal and member deactivation never soft-delete a system root or its
subtree: doing so would place retained user content on the purge path without a
product-level reassignment contract. Such roots remain available to
organization owners and administrators for recovery, while ordinary access is
still governed by live IAM membership and tags (and any explicit descendant
grant). A future archive, reassignment, or purge workflow must define its data
ownership and recovery behavior before reconciliation may destructively retire
these roots.

## D-036 — Durable multipart completion cannot be aborted

**Status:** Accepted

Once an upload enters `completing`, its provider outcome may be ambiguous even
when PostgreSQL has not published the file entry yet. An abort at that point
could discard the only durable reconciliation state while S3 already contains
the completed object. Briefcase therefore accepts aborts only from `initiated`
or `uploading`; `completing` and `completed` return a conflict, while repeated
aborts of `aborted` or `expired` sessions remain idempotent. Reconciliation or
retention maintenance must resolve a durable completion before cleanup.

## D-037 — Large restores use bounded, checksum-reconciled multipart transfer

**Status:** Accepted

Restoring an object never assumes that S3 `CopyObject` or a whole-object
temporary file can handle the service's 5 TiB maximum. Same-target restores use
one conditional provider copy through 5 GiB and the canonical multipart plan
above that limit. Multipart-copy parts are conditional on the source `ETag`,
and any failed attempt aborts its destination session and best-effort deletes a
possibly completed but unpublished destination key.

Cross-target restores up to 100 MiB retain the bounded single-request path.
Larger restores use the canonical multipart plan and keep only one source range
in private temporary storage at a time. Every range is conditional on the
checksum-validated source object's `ETag`, is hashed locally, and is uploaded
with its exact SHA-256. The ordered local bytes verify a persisted
`SHA256/FULL_OBJECT` source, while the ordered binary part digests verify a
persisted `SHA256/COMPOSITE` source. Destination publication requires S3's
exact composite checksum to match the locally calculated value. Any failure
before publication triggers both multipart abort and best-effort destination
deletion, preventing a failed restore from being mistaken for a valid version.

## D-038 — Restore identity, provider versions, and cleanup are exact

**Status:** Accepted clarification

Every content read binds the provider version identifier persisted with its
Briefcase version when S3 supplies one. This includes download signatures,
whole-object and ranged reads, metadata probes, and both single-request and
multipart copy sources. Compensation deletes the exact destination provider
version returned by S3; when no version was returned, Briefcase first discovers
the current version and never performs a blind delete after failed discovery.

Every restore requires an idempotency key, reserves its new Briefcase version ID
in that record, and derives the destination key from the retained ID. Retries
reuse the same key and first reconcile a current object only when its exact size
and provider checksum match the persisted source evidence; a mismatch is
deleted by exact provider version before transfer restarts. The restore lease is
renewed every minute with a ten-minute window, so a multi-part transfer cannot
lose ownership merely because it exceeds the generic five-minute mutation
lease.

Cancellation before PostgreSQL publication best-effort aborts any provider
multipart session and removes the unpublished destination. Once publication is
invoked, object ownership transfers to reconciliation and the cleanup guard is
disarmed: an ambiguous PostgreSQL `COMMIT` must retain bytes rather than risk
deleting an object whose version row became durable. Retries then either replay
the committed entry or reconcile the reserved key. A process crash and an
ambiguous provider create response still require lifecycle/orphan maintenance;
asynchronous drop cleanup is not treated as a durable job system.

## D-039 — Published IAM wire contracts supersede interim adapter assumptions

**Status:** Accepted; supersedes the wire-contract portions of D-005, D-020,
and D-025

Bearer authorization uses the published form-encoded introspection contract
with Briefcase application Basic authentication and `X-Org-ID`, followed by
`GET /api/v1/oauth/userinfo` using the original bearer and the same
organization header. Briefcase cross-binds `principal_id` to `sub`, actor type,
membership ID, and public organization handle, interprets `expires_at` as Unix
seconds, and requires current `org_role` and `tags` before constructing local
authority. Any absent, expired, unsupported, or contradictory fact fails
closed.

OBO verification uses the published singular `action` request and response,
HTTP Basic authentication, and `X-Org-ID`. Its required `Idempotency-Key` is
deterministically derived as `briefcase-obo-v1-<sha256(raw-proof)>`, preserving
safe retry behavior without disclosing the proof. The official actor shape is
`principal_id`, `type`, and `public_id`; Briefcase validates the principal UUID
and uses only `public_id` as its actor handle. IAM's published OBO response does
not carry current organization role or tags, so Briefcase accepts those fields
only as a coordinated top-level extension and otherwise denies access. The
route-stable action vocabulary in D-025 remains unchanged.

IAM application webhooks use exactly `X-Silicon-IAM-Event-ID`,
`X-Silicon-IAM-Timestamp`, `X-Silicon-IAM-Key-Version`, and
`X-Silicon-IAM-Signature: v1=<lowercase-hex-hmac>`. The HMAC covers
`timestamp + '.' + exact raw body`, the configured key version must match, and
the signed header event ID must equal the `spec_version: "1.0"` envelope event
ID. Authentication and freshness checks precede JSON parsing; durable event-ID
deduplication and aggregate-version ordering remain as decided in D-020.

The published webhook envelope contains no tenant field and its `data` object
is unconstrained. Briefcase therefore requires every event intended for this
receiver to carry the public IAM organization handle at `data.org_id`. It never
infers a tenant from the aggregate UUID and rejects events missing that routing
fact. IAM must make this event-data field contractual. IAM must also guarantee
that one bearer is valid for the paired introspection/userinfo exchange and
publish role/tag facts for OBO (or an equivalent current-authorization lookup)
before those flows can operate without coordinated extensions.

## D-040 — Provider cleanup precedes retention metadata deletion

**Status:** Accepted; supersedes the cleanup deferral in D-030 and clarifies
D-014 and D-015

Every destructive provider operation is represented by a durable
`object_cleanup_jobs` row containing the exact immutable storage descriptor and
provider identifier that created the object or multipart upload. API-initiated
multipart aborts enqueue this snapshot in the same transaction that changes the
upload from `initiated` or `uploading` to `aborted`; an ambiguous commit, an API
process crash, or an immediate provider failure therefore leaves worker-visible
cleanup state. Successful API cleanup deliberately leaves the job for one
idempotent worker confirmation. Expiry maintenance atomically transitions only
`initiated` and `uploading` rows to `expired` while enqueuing the same snapshot.
It also reconciles legacy `aborted` and `expired` rows. Neither path ever queues
or aborts a `completing` or `completed` upload.

The fixed 24-hour multipart deadline is calculated by the domain and persisted
on each upload. The former worker-only TTL environment variable was removed
because changing it could not affect those rows and presented a false operator
control; cleanup consumes the authoritative `expires_at` value.

Provider jobs use bounded discovery, one-at-a-time leasing with `FOR UPDATE SKIP
LOCKED`, indefinite capped retry, and stable redacted failure codes. No database
transaction or row lock is held during S3. Lease expiry may cause duplicate
calls, so exact S3 version deletion and multipart abort are intentionally
idempotent. Platform and organization-managed targets are reconstructed from
snapshotted bucket, region, prefix, encryption, KMS, and role descriptors. The
STS External ID is re-derived from the trusted organization identifier rather
than persisted or accepted from a caller.

For active files, only versions below the newest 50 are scheduled and the
current version is independently excluded. A provider-confirmed delete precedes
deletion of its immutable version row. `restored_from_version_id` remains an
immutable provenance UUID but is no longer a foreign key, because a retained
restore must not pin an otherwise expired source version forever.

For a subtree whose persisted 45-day `purge_after` has elapsed, every version is
scheduled and its job remains in `object_deleted` state after provider success.
The worker deletes the subtree and its cascading metadata only when every exact
version has that result and no unresolved multipart session references the
batch. A `completing` upload blocks purge until a separate completion
reconciler determines its provider outcome; aborting it would risk deleting a
successfully completed but not yet published object.

There is a narrow availability race between the worker's final eligibility
preflight and an already-starting restore of an old, retention-eligible active
version. Exact source validation and compensating destination cleanup prevent a
corrupt restore from publishing, but that restore can fail and require retry.
Eliminating the race entirely requires a durable restore-source lease consulted
by retention; no such cross-operation lease exists in the current API contract.

## D-041 — External compensation depends on a known database outcome

**Status:** Accepted clarification of D-010, D-034, and D-038

A failure returned by PostgreSQL's final `COMMIT` does not prove that the
transaction rolled back: the server may have made the transaction durable
before the connection failed. Small-upload publication, multipart initiation,
and version-restore publication therefore map only that final boundary to a
distinct `commit_outcome_unknown` response. The public response is a retryable
HTTP 503 that instructs the caller to reuse the same idempotency key, while logs
contain only a static operation label.

On this exact outcome, Briefcase preserves the provider object or multipart
session and retains the idempotency reservation. A retry can replay a committed
transaction or reconcile the reserved identity without deleting state that
PostgreSQL may already reference. Errors known to occur before `COMMIT` still
trigger exact provider compensation and release the reservation immediately;
this includes restore transfer and lease-renewal failures as well as definite
publication errors. Successful commits disarm compensation normally.

Cancellation remains best-effort: provider cleanup is scheduled from an armed
drop guard, but the idempotency reservation may remain until lease expiry
because asynchronous cancellation cannot atomically run database release.
Likewise, if an unknown commit actually rolled back, an unrecorded S3 object
version or multipart session can remain. Bucket lifecycle rules for incomplete
multipart uploads and orphan reconciliation remain required operational safety
nets; guessing rollback and deleting provider state is never considered safe.

## D-042 — Streaming uploads authenticate before accepting file bytes

**Status:** Accepted clarification of D-005, D-012, and D-017

Briefcase validates the organization header, credential shape, and idempotency
key before Axum advances a multipart body. A bearer can be verified immediately.
An OBO proof is resource-bound, so the small-upload contract requires
`parent_id` to be serialized before `file`; Briefcase parses that bounded text
part, verifies IAM against the exact destination, and only then stages binary
chunks. The optional multipart `app_id`, when present, must equal the required
`X-App-ID` header. Requests that mix bearer and OBO credentials, omit one OBO
header, or place file bytes before the bound parent fail without staging the
file.

The OpenAPI security alternatives model OBO as the conjunction of
`X-IAM-OBO-Access-Proof` and `X-App-ID`. Organization storage configuration
overrides the global alternatives with bearer-only security, matching D-028.

## D-043 — Synchronous restores have isolated resource budgets

**Status:** Accepted clarification of D-012, D-037, and D-038

The published v1 restore operation returns a completed new version and defines
no asynchronous job resource, so Briefcase keeps it synchronous. It is isolated
from ordinary and upload routes by a configurable restore deadline and a
per-process concurrency limit. The default admits two restores for up to 48
hours each; the global request limit remains an outer cap. This bounds memory,
bandwidth, and temporary-disk amplification while allowing canonical multipart
transfer through the 5 TiB service limit.

One S3 operation receives a separate 30-minute default budget because a
canonical part can reach 5 GiB; the former one-minute default could reject a
valid part on ordinary links. All budgets are deployment controls rather than
correctness assumptions. Operators must tune them to maximum object size,
observed throughput, temporary capacity, and upstream connection limits. A
future asynchronous restore contract can replace the long-lived HTTP request
without changing the reserved-version and reconciliation invariants.
