# Silicon Briefcase API documentation

This document explains every operation in the Silicon Briefcase OpenAPI contract. The machine-readable contract is in [`openapi.yaml`](./openapi.yaml).

## API conventions

### Base URL

```text
https://briefcase.teamofsilicons.com/api/v1
```

Briefcase is an organization-scoped filesystem for Carbons, Silicons, and applications using IAM OBO Access. Every request is evaluated against the represented actor's organization membership and effective file permissions.

### Authentication

- **Bearer authentication:** An IAM access token for a Carbon or Silicon.
- **OBO Access:** `X-IAM-OBO-Access-Proof` plus `X-App-ID` when an application acts on behalf of an actor.
- **Organization context:** Authenticated operations require `X-Org-ID`.
- **Idempotency:** Creation and upload-finalization operations require `Idempotency-Key` so retries do not create duplicate resources.

### Entry model

Files and folders are both entries. Every entry has an owner, permanent authenticated URL, parent, effective access, timestamps, and one of three inherited root types:

- **Public:** Readable by every current organization member.
- **Private:** Visible only to its owner and explicitly authorized members.
- **Tag:** Readable by members who currently possess the matching IAM tag.

Organization owners and authorized administrators currently have full administrative visibility.

### Errors

Errors use the standard `error.code`, `error.message`, and `error.request_id` envelope.

## Browsing and folder management

### `GET /entries`

Lists visible children of a folder.

- **Authentication:** Bearer or OBO Access.
- **Required header:** `X-Org-ID`.
- **Query:** Optional `parent_id`, cursor, and limit.
- **Returns:** Permission-filtered entries and `next_cursor`.

Omitting `parent_id` loads the organization root. Private folders belonging to other actors must be hidden unless at least one descendant has been shared with the caller.

### `POST /entries`

Creates a folder.

- **Authentication:** Bearer or OBO Access.
- **Required:** `name` and `Idempotency-Key`.
- **Optional:** `parent_id`, `root_type`, `tag`, and initial invitees.
- **Returns:** The created folder entry.

`root_type` is required only when creating at the organization root. A tag root also requires its IAM tag. Below an existing folder, the child inherits the parent's root type and permission boundary.

The caller needs write access to the parent. Invitees must already belong to the organization.

### `GET /entries/{entry_id}`

Returns metadata for one visible file or folder.

- **Authentication:** Bearer or OBO Access.
- **Returns:** Entry metadata and effective access.

Reading metadata must still require visibility. Possessing an entry ID or permanent URL does not grant access.

### `PATCH /entries/{entry_id}`

Renames or moves an entry.

- **Authentication:** Bearer or OBO Access.
- **Input:** New `name`, new `parent_id`, or both.
- **Returns:** Updated entry.

The caller needs write access to the entry and destination folder. Moving an entry across Public, Private, or Tag boundaries requires permission recalculation and must not accidentally broaden access.

### `DELETE /entries/{entry_id}`

Moves an entry to the bin.

- **Authentication:** Bearer or OBO Access.
- **Returns:** `204 No Content`.

Deletion is recoverable for 45 days. OBO applications cannot delete files they did not create, even when the represented actor can otherwise modify them. Recursive folder deletion must preserve all descendants for recovery.

### `POST /entries/{entry_id}/download-url`

Creates a temporary CDN URL for a file.

- **Authentication:** Bearer or OBO Access.
- **Returns:** URL and expiry.
- **Intended lifetime:** 12 hours.

The caller must currently have read access. The permanent URL remains the stable resource identifier; the temporary URL is an expiring delivery mechanism and must not be stored as an attachment reference.

## Uploads

### `POST /uploads`

Uploads a file of at most 100 MiB in one request.

- **Authentication:** Bearer or OBO Access.
- **Content type:** `multipart/form-data`.
- **Required:** `parent_id`, binary `file`, and `Idempotency-Key`.
- **Optional:** Originating `app_id` for OBO Access.
- **Returns:** Created file entry.

The caller requires write access to the destination. Files larger than 100 MiB receive `413` and must use multipart upload.

### `POST /multipart-uploads`

Initializes an S3 multipart upload for a file larger than 100 MiB.

- **Authentication:** Bearer or OBO Access.
- **Input:** Parent, name, byte size, and content type.
- **Returns:** `upload_id`, calculated part size, part count, and expiry.

Part size is calculated from the declared file size, targets roughly 1,000 parts, and remains between 8 MiB and 5 GiB. The file size cannot exceed the configured 5 TiB limit.

### `PUT /multipart-uploads/{upload_id}/parts/{part_number}`

Uploads one binary part.

- **Authentication:** Bearer or OBO Access.
- **Content type:** `application/octet-stream`.
- **Returns:** The part `ETag` header.

Part numbers start at 1. The client must retain each ETag because completion needs the ordered list. Retrying the same part number replaces that part rather than adding a duplicate.

### `POST /multipart-uploads/{upload_id}/complete`

Finalizes a multipart upload.

- **Authentication:** Bearer or OBO Access.
- **Input:** Ordered `part_number` and `etag` pairs.
- **Required header:** `Idempotency-Key`.
- **Returns:** Created file entry.

Briefcase verifies the expected parts and asks storage to assemble them. Completion must be idempotent so a network retry returns the same entry.

### `DELETE /multipart-uploads/{upload_id}`

Aborts an unfinished multipart upload.

- **Authentication:** Bearer or OBO Access.
- **Returns:** `204 No Content`.

Aborting removes uploaded parts and releases storage. It does not create a file or bin entry.

## Permissions and access requests

### `GET /entries/{entry_id}/permissions`

Lists explicit permission grants on an entry.

- **Authentication:** Bearer or OBO Access.
- **Returns:** Principal, access level, inheritance, grantor, and creation time.

This returns explicit grants, not every effective permission derived from Public, Tag, ownership, ancestry, or administrator status.

### `POST /entries/{entry_id}/permissions`

Grants another organization member access.

- **Authentication:** Bearer or OBO Access.
- **Input:** Principal, `read` or `write` access, and whether the grant inherits.
- **Returns:** Created permission grant.

Only the owner or an actor with permission-management authority may grant access. The principal must be a current Carbon or Silicon in the same organization.

### `DELETE /entries/{entry_id}/permissions/{grant_id}`

Revokes an explicit permission grant.

- **Authentication:** Bearer or OBO Access.
- **Returns:** `204 No Content`.

Revocation removes that grant and any inherited access produced by it. Access derived independently from another grant, tag, Public visibility, ownership, or administrative rights remains.

### `POST /entries/{entry_id}/access-requests`

Requests read or write access to an entry.

- **Authentication:** Bearer or OBO Access.
- **Input:** Requested access and optional reason.
- **Returns:** Pending access request.

This is used when an authenticated organization member opens a permanent URL without access. The response should not reveal sensitive metadata beyond what is required to request access.

### `POST /access-requests/{request_id}/decision`

Approves or denies an access request.

- **Authentication:** Bearer or OBO Access.
- **Input:** `approve` or `deny`, plus the granted level when approving.
- **Returns:** Updated request.

The owner or an authorized organization administrator can decide. Approval creates an explicit permission grant and records the decision actor and time.

## Search

### `GET /search`

Searches visible filenames and supported document contents.

- **Authentication:** Bearer or OBO Access.
- **Query:** `q` and optional limit from 1 to 20.
- **Returns:** Ranked, permission-filtered results.

Filename matches rank above content matches. Content results include match count and optional snippets. Search indexes must be updated when access is granted, revoked, or inherited differently so they never leak inaccessible content.

## File versions

### `GET /entries/{entry_id}/versions`

Lists up to the last 50 versions of a file.

- **Authentication:** Bearer or OBO Access.
- **Returns:** Version ID, number, size, author, and time.

The caller must be able to read the current file. Whether old versions can have stricter permissions than the current entry should be explicitly decided.

### `POST /entries/{entry_id}/versions/{version_id}/restore`

Restores an older version.

- **Authentication:** Bearer or OBO Access.
- **Returns:** Updated file entry.

Restoration does not erase later history. It copies the selected content into a new current version and records the restoring actor.

## Bin

### `GET /bin`

Lists deleted entries visible to the caller.

- **Authentication:** Bearer or OBO Access.
- **Returns:** Deleted entries.

The owner sees their recoverable entries. Administrators see entries allowed by administrative policy. Deleted entries remain recoverable for 45 days.

### `POST /bin/{entry_id}/restore`

Restores a deleted entry.

- **Authentication:** Bearer or OBO Access.
- **Returns:** Restored entry.

If the original parent no longer exists or is inaccessible, Briefcase needs a defined fallback destination. Restoring a folder restores its retained descendants and permission structure.

## Organization storage

### `PUT /storage/configuration`

Configures an organization-provided S3 bucket.

- **Authentication:** Bearer token.
- **Caller:** Organization owner or authorized administrator.
- **Input:** Bucket, region, role ARN, prefix, AWS account, encryption mode, and optional KMS key.
- **Returns:** Configuration validation status.

Briefcase assumes the configured role and performs a temporary create, read, update, and delete test. The organization bucket becomes active only when all checks succeed. IAM credentials or static AWS secret keys must not be accepted.

## Complete flows

### Small upload

```text
Resolve destination folder
  -> verify write access
  -> POST /uploads
  -> store permanent URL in consuming applications
  -> request temporary URL only when rendering or downloading
```

### Multipart upload

```text
Initialize multipart upload
  -> upload every numbered part
  -> retain every ETag
  -> complete with ordered ETags
  -> receive the final file entry
```

### Access request

```text
Open permanent URL without access
  -> request read or write access
  -> owner/admin decides
  -> approval creates a grant
  -> actor can request a temporary download URL
```

## Contract gaps

- Uploading a new version to an existing file is not defined.
- Permanent authenticated content and preview endpoints are not separately documented.
- Folder-recursive permission changes need explicit conflict and inheritance rules.
- Permission listing, access-request listing, and audit-history pagination are incomplete.
- There is no endpoint to permanently erase a bin item before 45 days.
- Automatic multipart-upload expiry and cleanup are not exposed.
- Search indexing status and unsupported-document behavior are not represented.
- OBO Access needs an explicit `X-App-ID` parameter in the machine-readable contract.
- BYO-S3 configuration read, remove, rotate-role, and retest endpoints are missing.
- File replacement, copy, bulk move, and bulk delete operations are missing.
- Malware scanning, quarantine, legal hold, storage quotas, and content safety are not defined.
