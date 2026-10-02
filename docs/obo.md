# On-behalf-of access

> **Integration preview for Briefcase 3.0.0 / IAM 5.0.0.** These docs are published ahead of the coordinated runtime rollout. Check `/api/version` before switching a production client; a published guide does not mean the new service is live.

OBO lets an application act for a carbon or silicon after that person or custodian-approved identity authorizes a feature. Login grants an application session for one account and organization. Request Briefcase OBO consent separately, when the user chooses a feature that reads or stores files. IAM displays the selected endpoints, dependencies and destination account/organization before approval.

## Migrate from Briefcase 2.x

Upgrade the service and official Rust client/CLI to 3.0.0 together. Delegated operation revisions change to 3.0.0; the API namespace remains `/api/v1`. Ordinary file operations keep their existing revisions. The official client verifies the operation inventory before transmitting credentials and rejects an incompatible service.

Replace `X-IAM-OBO-Access-Proof: obo_...` with `X-IAM-OBO-Access-Token: oba_...`. Old proof headers and token classes are rejected. Stop minting a proof for each body digest. Approved OBO access tokens are reusable within their endpoint graph until expiry or revocation; refresh them through IAM using the OBO refresh token. The SDK's historical `OboProof` type now wraps this access token and can be cloned. It never refreshes or persists OBO tokens itself.

`POST /api/v1/obo/files` is retired with HTTP 410 and code `obo_upload_retired`; the service rejects it before staging bytes. `create_file_on_behalf_of` returns a local configuration error without sending bytes or credentials. Use reserve, transfer and commit for every delegated upload, including small files.

## Choose an operation

Configure these endpoints in the Briefcase application catalog in Honeycomb. Each uses POST. The IDs below are the public endpoint IDs currently used by Briefcase and its SDK; do not invent a different ID or repoint one to another route. Discover accepted endpoint details from IAM/Honeycomb before requesting consent.

| Endpoint ID | Route | Purpose |
| --- | --- | --- |
| `briefcase.folders.create` | `/api/v1/obo/folders/create` | Create a child folder |
| `briefcase.entries.list` | `/api/v1/obo/entries/list` | List entries visible to the represented identity |
| `briefcase.files.read` | `/api/v1/obo/files/read` | Read a current file |
| `briefcase.entries.trash` | `/api/v1/obo/entries/trash` | Recoverably remove an app-created entry |
| `briefcase.uploads.reserve` | `/api/v1/obo/uploads/reserve` | Reserve private staging |
| `briefcase.uploads.commit` | `/api/v1/obo/uploads/commit` | Publish staged content |
| `briefcase.uploads.status` | `/api/v1/obo/uploads/status` | Reconcile an upload |
| `briefcase.uploads.cancel` | `/api/v1/obo/uploads/cancel` | Abandon unpublished content |
| `briefcase.invitations.create` | `/api/v1/obo/invitations` | Invite a recipient; critical |
| `briefcase.link_access.update` | `/api/v1/obo/link-access` | Change public link access; critical |

The application obtains user approval for the graph through IAM's OBO authorization flow, exchanges the resulting code using its own application credentials and keeps access/refresh tokens in server-side secret storage. See the [IAM SDK reference](https://docs.iam.teamofsilicons.com/client/) for the authorization, exchange, refresh and revocation contract.

## Send a delegated request

```text
POST /api/v1/obo/folders/create
Content-Type: application/json
X-App-ID: your-app
X-IAM-OBO-Access-Token: oba_<access-token>
X-Org-ID: selected-organization

{"operation_id":"66a263a8-2dd7-42ea-aa88-3177f48ca6be","parent_path":"","name":"recordings"}
```

Do not send an actor bearer alongside OBO credentials. `X-Org-ID` is optional for control requests but must match IAM's selected destination when supplied. Test calls also supply the paired `X-Briefcase-App-Secret`; mismatched or inactive test planes never fall back to production.

Briefcase verifies the access token online with its own IAM application credentials for the fixed receiving endpoint and HTTP route. It checks the receiving app, selected endpoint, originating app, represented identity, destination organization and test plane. IAM consent is necessary but does not bypass Briefcase file permissions, app namespaces, storage limits or current membership. Missing identity/role disclosure fails closed. Unknown tags grant no tag-based access.

The token can cover an approved dependency chain. Each receiver verifies its own endpoint with its own credentials; sharing a token across a chain does not permit an unrelated endpoint. An ATA token never becomes OBO authority.

## Keep retries safe

Prepare typed JSON manifests once. Their SHA-256 supports local integrity and logical idempotency; it is not an IAM per-request signature. Retain the same non-nil `operation_id` and unchanged manifest when retrying a mutation after an uncertain response. Every call checks current authority again. A valid token may be reused; an expired token must be refreshed, and a revoked or removed grant requires authorization again. Do not silently retry permission failures.

Listing and reading use `DelegatedListEntries` and `DelegatedReadFile`; create a new manifest when the cursor, range or other input changes. Retain no verified authorization snapshot as a durable permission grant.

## Upload and publish

1. Hash the regular source file, then prepare `DelegatedReserveUpload` with the logical operation ID, destination, name, media type, size and SHA-256.
2. Call `reserve_delegated_upload` with a valid OBO access token. The response contains state, upload ID, deadline and (only when available) a narrow upload capability.
3. Transfer bytes with that capability, organization and test selector. Do not send IAM, app or actor credentials to the byte transfer route.
4. Prepare `DelegatedCommitUpload` and commit with a currently valid OBO token. Publication rechecks membership, destination rights and quota, even if reserve previously passed.
5. If a response is lost, check status with `DelegatedUploadQuery` before deciding whether another action is needed. A successful logical commit does not create duplicate file versions.

The transfer capability cannot read or publish. It may outlive a particular OBO access token, but cannot outlive the reservation. See [delegated uploads](api/delegated-uploads.md) for limits, states and cleanup behavior, and the [Rust client guide](client/README.md) for typed calls.

## Migration checklist

- [ ] Upgrade to the matching Briefcase 3.0.0 operation contract and IAM 5 SDK.
- [ ] Request feature-specific OBO consent after login.
- [ ] Register exact endpoint IDs and dependency relationships.
- [ ] Use `oba_` tokens and `X-IAM-OBO-Access-Token` with no actor bearer.
- [ ] Store OBO refresh tokens securely and handle expiry/revocation.
- [ ] Replace raw one-shot uploads with reserve, transfer and commit.
- [ ] Preserve mutation IDs and manifests for uncertain outcomes.
- [ ] Test carbons, silicons, selected organizations and isolated test planes.
- [ ] Verify denied file access, missing consent, invalid endpoint and wrong token class fail closed.
