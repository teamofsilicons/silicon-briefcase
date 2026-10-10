# Briefcase HTTP API

Contract **4.0.0**, served under `/api/v1`. See the [operation inventory](operations.md), [generated reference](reference.md), and [OpenAPI](../../openapi.yaml).

## Authentication

Normal requests use `Authorization: Bearer <Silicon Accounts access token>` issued to Briefcase. `GET /accounts` is public discovery; `GET /me` verifies the caller. Accounts owns sign-in, refresh and revocation. Neither IAM credentials nor organization or testing selectors are accepted.

Every account has a drive. Listings, uploads, usage, bin and storage configuration default to the caller's drive and can select another permitted drive by its owner id or UUID. Entry routes derive the drive from the entry itself. Custodian and explicit-sharing checks apply consistently; hidden and missing entries both return 404.

Applications send `Authorization: Proof sap_…` only to the route-specific `/obo/*` operations. A proof must represent the account, target Briefcase, carry the route scope and come from a configured issuer. See [delegation](../obo.md).

## Negotiation and retries

Before credentials, request host-root `/api/version` with `Briefcase-Supported-API-Versions: v1`. The official Rust client verifies the exact operation catalog before its first operation. API namespace and package versions are independent.

Retain `Idempotency-Key` with the exact body and destination for mutation retries. Creation, uploads, invitation changes and link changes require it; other mutation routes may accept it. Authorization is rechecked on replay. Delegated JSON mutations carry a stable `operation_id`; obtain a fresh proof for each call. Do not change the body after an uncertain result without reconciling status.

Errors carry a stable code, message and request id. 401 means invalid or ended authentication; 403 insufficient authority; 404 absent or hidden data; 409 conflicting state or reused key; 416 an invalid byte range; 429 a rate limit; 503 an unavailable dependency; 507 storage exhausted.

## Content

Create a folder with `POST /entries`. Upload `multipart/form-data` to `/uploads`; choose `parent_id` or `drive`/`parent_path`, never both. Reuploading the same filename creates the next version. Reads stream `/entries/{id}/content` or `/download`; byte ranges return 206. Folder downloads stream tar.zst. `/archive` lists bounded archive entries. `/versions` is cursor-paginated; restore publishes a new current version.

Permanent paths use `/paths/{owner}/{path}`. Without a bearer only explicitly enabled link access is visible. Changed public ids and migrated organization paths redirect to canonical locations. Consumers must retain access checks after redirects.
