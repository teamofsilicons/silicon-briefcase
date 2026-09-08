# Delegated uploads

Applications upload for a represented IAM member in three phases: reserve a
destination, transfer private bytes, then publish with fresh authority. The
transfer does not have to finish within the reservation proof's lifetime. It
still has to fit the server's upload deadline and physical capacity.

Use the official `briefcase-client` package. Briefcase verifies control proofs
online through the official `silicon-iam-client`; a Browser-bound token is never
substituted for a Briefcase-bound bearer credential.

## IAM registration and credentials

Register these endpoints with method `POST` and an empty metadata schema. Mint
a fresh proof for the exact JSON body SHA-256 on every control call.

| Endpoint ID | Exact IAM binding path | Input |
| --- | --- | --- |
| `briefcase.uploads.reserve` | `/api/v1/obo/uploads/reserve` | Full manifest below |
| `briefcase.uploads.commit` | `/api/v1/obo/uploads/commit` | `operation_id`, `upload_id` |
| `briefcase.uploads.status` | `/api/v1/obo/uploads/status` | `operation_id` |
| `briefcase.uploads.cancel` | `/api/v1/obo/uploads/cancel` | `operation_id` |

Control calls send `X-App-ID` and `X-IAM-OBO-Access-Proof`, with no bearer.
`X-Org-ID`, when supplied, must agree with IAM. The originating token needs
delegated-issuance authority; the represented member and Briefcase application's
approved scopes must disclose current roles and membership as described in the
[IAM guide](../iam-integration.md).

The raw transfer route uses no IAM endpoint or proof: only a narrow upload
capability and the same organization/plane selector. Keep the capability private.
It cannot read files, publish content or act as a general application credential.

## Reserve

Hash the complete file first. Retain one non-nil logical operation UUID with its
unchanged manifest. This example describes an empty file; replace both size and
digest for real content.

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

An empty parent path selects `private/{actor}/apps/{originating_app}`. Otherwise
the folder must exist; delegated folder creation can build the hierarchy first.
The resolved parent, current destination entry (if any), storage target and
immutable IAM identity are frozen. A later occupant cannot inherit an old
reservation's authority. Normal independent create/update rights apply.

Reserve returns HTTP 200 with `operation_id`, `upload_id`, `state`, `expires_at`,
`published_entry_id` and `capability`. Only an idle `reserved` operation returns
a capability. A fresh-authorized unchanged reserve retry may rotate that
capability, invalidating the old one; it does not extend the deadline.

Reservations live at most 24 hours. An organization may have at most 32 pending
reservations and 5 TiB of reserved bytes, additionally constrained by its current
daily/storage allowances. Pending bytes also constrain ordinary uploads and
restores. A test environment's aggregate limit remains 2 GiB. Public usage
counters report published versions; reserved capacity is charged as an upload
only at successful publication.

## Transfer private bytes

```text
PUT /api/v1/obo/uploads/{upload_id}/content
X-Org-ID: your-organization
X-Briefcase-Upload-Capability: <private capability>
Content-Type: application/octet-stream
Content-Length: <exact reserved size>
```

Use the same `X-Testing-Environment-Key` throughout a test-plane flow. Do not
send `Authorization`, `X-App-ID`, `X-IAM-OBO-Access-Proof`, content encoding or
chunked transfer encoding. The SDK handles framing and the configured plane.

Briefcase verifies the complete length and SHA-256 before provider writes.
At most 100 MiB uses one S3 request; larger content uses the normal multipart
plan. Verified immutable bytes advance to `staged`, without a visible file or
upload charge. The reservation must have enough remaining lifetime for a full
writer lease; an almost-expired reservation is rejected before bytes are accepted.

## Commit and recovery

After transfer, obtain a new proof for a small commit body containing the
original `operation_id` and returned `upload_id`. Commit rechecks current IAM
identity, originating application, test generation, destination rights and quota.
It publishes the existing object and charges quota in one database transaction,
returning `committed` with the actual `published_entry_id`.

A repeated successful logical commit with fresh authority does not create
another version. After any uncertain response, use a fresh status proof first.

| State | Next action |
| --- | --- |
| `reserved` | Fresh reserve can issue a capability; then transfer. |
| `receiving` | Reconcile later; do not start a second writer. |
| `staged` | Obtain a fresh commit proof. |
| `committed` | Retain the published entry ID; do not re-upload. |
| `cleanup_pending` | Keep the operation record while cleanup is reconciled. |
| `cancelled`, `expired` | This operation cannot publish; start a new logical operation for a new attempt. |

Status and cancellation require fresh IAM authority for the same immutable
member/application/plane, but remain available for that member's own operation
after destination access is revoked. They return no capability or file content.
Capability issuance and publication always require current destination rights.

Cancellation never deletes a committed file; use delegated trash for that.
Pending cleanup retains its exact storage descriptor and reservation until the
provider outcome is known. Test-environment clean revokes staging and preserves
cleanup work; final purge waits for provider cleanup.

If the IAM parent token or session expires or is revoked, obtain fresh initiator
authorization. A recording outbox can retain manifests, operation/upload IDs and
state, but must not reuse an old proof or saved authorization snapshot as a
durable grant, or take over the CLI's rotating refresh token.

## Rust and CLI

`DelegatedReserveUpload::file` hashes a file with bounded memory. `prepare()`
freezes a typed manifest with `method()`, `path()`, `endpoint_id()`,
`body_sha256()` and `body_bytes()`. Obtain a proof for those exact values and
pass that same manifest to `reserve_delegated_upload`.

File-based preparation and transfer require a regular, non-symlink source.
Use the actual file path and keep its contents unchanged between hashing and
transfer. Special files such as pipes and devices are not upload sources.

Then use `transfer_delegated_upload`, followed by a separately prepared
`DelegatedCommitUpload` and `commit_delegated_upload`. `DelegatedUploadQuery`
and `DelegatedCancelUpload` prepare status and cancellation. Proof/capability
types redact debug output and are consumed by calls. No delegated SDK call
automatically retries, persists a session or runs package maintenance.

The CLI provides `app prepare-upload`, `app request ... --describe`, upload
control verbs and `app transfer`. Manifest preparation is entirely local.
Capabilities go only to an explicitly requested new private file, never normal
or JSON status output. See the [CLI guide](../cli/README.md) for examples.
