# Calling Briefcase on behalf of a member

This guide is for an Application that wants to create a file in Silicon
Briefcase **for a Carbon or Silicon it already represents**, without ever
holding a Briefcase credential of its own. It is self-contained.

On-behalf-of (OBO) means IAM mints a proof for **one exact request**. Briefcase
verifies that proof with IAM, consumes it, and then acts as the represented
member — with that member's current role and tags, never more. There is no
long-lived delegated key to store, leak, or revoke.

- Audience Application: `tos>briefcase`
- Briefcase API base: `https://backend.briefcase.teamofsilicons.com/api/v1`
- IAM base: `https://backend.iam.teamofsilicons.com/`

`X-Org-ID`, `org_id` and the `{org_id}` URL segment carry the **Team** ID; they
are wire names from the IAM contract, kept verbatim throughout this guide.

## Choose an operation

Briefcase supports the following proof-authorized operations. Discover the
audience's registered endpoints with
`GET {iam}/api/v1/obo-access/applications/tos>briefcase/endpoints`
(URL-encode the `>`). Register the fixed paths below before issuing proofs;
registration is separate in production and each imported IAM test Application.

| Endpoint ID | Method and registered path | Metadata schema |
| --- | --- | --- |
| `briefcase.files.create` | `POST /api/v1/obo/files` | `path`, `name`, `content_type` — all required strings |
| `briefcase.folders.create` | `POST /api/v1/obo/folders/create` | Empty object `{}` |
| `briefcase.entries.list` | `POST /api/v1/obo/entries/list` | Empty object `{}` |
| `briefcase.files.read` | `POST /api/v1/obo/files/read` | Empty object `{}` |
| `briefcase.entries.trash` | `POST /api/v1/obo/entries/trash` | Empty object `{}` |
| `briefcase.uploads.reserve` | `POST /api/v1/obo/uploads/reserve` | Empty object `{}` |
| `briefcase.uploads.commit` | `POST /api/v1/obo/uploads/commit` | Empty object `{}` |
| `briefcase.uploads.status` | `POST /api/v1/obo/uploads/status` | Empty object `{}` |
| `briefcase.uploads.cancel` | `POST /api/v1/obo/uploads/cancel` | Empty object `{}` |

- **Large, slow, or recoverable uploads:** use reserve → private transfer →
  fresh-authorized commit. Retain the logical operation UUID and reconcile an
  uncertain result with status. See [delegated uploads](api/delegated-uploads.md).
- **Small, immediate uploads:** the compatible one-shot `files.create` route
  sends raw file bytes. Its proof must still be valid after the body arrives;
  the 5 TiB size ceiling does not guarantee a transfer fits the proof lifetime.
  A fresh proof after an uncertain one-shot result is a new upload attempt.
- **Folder hierarchy, browsing, download and deletion:** use the JSON controls
  below with the represented member's current permissions.

The byte-only `PUT /api/v1/obo/uploads/{upload_id}/content` is not registered in
IAM. It uses the reservation's private upload capability, not an IAM proof or
member bearer. It cannot publish a file; only fresh-authorized commit can.

An endpoint missing from the selected audience's live catalog cannot receive a
proof. Confirm [registration and scope disclosure](iam-integration.md#obo-registration-is-separate)
before sending either production or sandbox operations.

## Before you start

You need all five of these. Briefcase fails closed on any of them.

1. **Your Application is registered in IAM, in the same Team as Briefcase.**
   OBO never crosses a Team. IAM derives the Team from the two Applications and
   refuses to accept one from you.
2. **A subject token: the member's IAM access token, issued to *your*
   Application** (`oat_…`). A token issued to some other Application is refused.
3. **`obo.issue` on that subject token.** Without it the exchange answers
   `403 obo_subject_token_forbidden`.
4. **`self.membership.read` and `self.identity.read` disclosure**, in both the subject token
   and Briefcase's currently approved scopes. Briefcase requires the delegated
   authorization snapshot and answers `403` when role or membership disclosure
   is missing. Never infer authority from an undisclosed (`null`) field.
5. **Your Application's own secret**, used only for HTTP Basic and the HMAC on
   the IAM exchange. It is never sent to Briefcase.

The member must be an active member of your Application's Team at verification
time. Ending a session does not by itself extend or revoke IAM authority.

## JSON controls and recoverable uploads

All eight JSON operations use `POST`, `Content-Type: application/json`, and an
empty IAM metadata object. Bind the complete serialized JSON body, not file
bytes or a subset of its fields. Send `X-App-ID` and
`X-IAM-OBO-Access-Proof`, never a bearer. A supplied `X-Org-ID` must agree with
IAM; a sandbox also needs its separate IAM testing app secret.

| Operation | JSON inputs | Result |
| --- | --- | --- |
| Folder create | `operation_id`, `parent_path`, `name` | `201` created entry |
| Entries list | Optional `parent_id` or `path`, `filter`, `cursor`, `limit` | `200` page; limit 1–100 |
| File read | `entry_id`; optional `range`, `download` | `200` or `206` bytes |
| Entry trash | `operation_id`, `entry_id` | `204`; recoverable bin deletion |
| Upload reserve | `operation_id`, `parent_path`, `name`, `content_type`, `size`, `sha256` | `200` status and an idle reservation's private capability |
| Upload commit | `operation_id`, `upload_id` | `200` status and published entry ID |
| Upload status | `operation_id` | `200` current state; no capability |
| Upload cancel | `operation_id` | `200` cancellation or cleanup-pending state |

Mutation `operation_id` values are caller-generated, non-nil UUIDs. Persist
the UUID and unchanged request before sending. After an uncertain result,
obtain a fresh proof for the same logical operation; never reuse the consumed
proof. A successful repeated commit does not publish a second version. A
repeated trash cannot delete an entry that has since been restored.

An empty creation `parent_path` selects the member's private app folder.
Otherwise the parent must already exist and be writable. Create a hierarchy
one folder at a time. Listing and reads retain ordinary permission filtering;
range, disposition and pagination values are proof-bound JSON, not override
headers or query parameters. See the [exact JSON API contract](obo.md#json-controls-and-recoverable-uploads).

For uploads, prepare the complete file manifest before requesting a proof:

```bash
briefcase app prepare-upload --operation-id "$UPLOAD_OPERATION_ID" \
  --parent-path '' ./recording.webm > manifest.json
briefcase app request upload-reserve --body manifest.json --describe
# Ask IAM for a fresh proof using the described endpoint, method and exact body.
briefcase app request upload-reserve --body manifest.json \
  --app-id 'tos>your-app' --capability-file upload.capability
# The hidden prompt accepts the proof; keep the returned upload_id.
briefcase app transfer "$UPLOAD_ID" ./recording.webm \
  --capability-file upload.capability
```

Transfer only stages bytes. Prepare `{"operation_id":"<original UUID>",
"upload_id":"<returned UUID>"}` in `commit.json`, describe `upload-commit`,
obtain a new IAM proof, and send `briefcase app request upload-commit --body commit.json
--app-id 'tos>your-app'`. Use `upload-status` with a body containing only the
original `operation_id` after an uncertain response. The
[upload guide](api/delegated-uploads.md) specifies states, limits, cancellation
and cleanup. Keep capability files owner-only; they are credentials.

The official Rust client prepares the exact body and binding through typed
`DelegatedReserveUpload`, `DelegatedCommitUpload`, `DelegatedUploadQuery` and
`DelegatedCancelUpload` requests. Use the prepared value for both IAM proof
issuance and its matching SDK call. The [operation map](api/operations.md)
lists every SDK method and CLI verb.

## One-shot upload: the shape of the call

For the compatible raw-byte endpoint, use these four steps:

```text
1. Discover   GET  {iam}/api/v1/obo-access/applications/tos%3Ebriefcase/endpoints
2. Hash       body_sha256 = lowercase hex SHA-256 of the EXACT file bytes
3. Exchange   POST {iam}/api/v1/obo-access/exchanges          -> access_proof (obo_…)
4. Call       POST {briefcase}/obo/files  with those exact bytes + the proof
```

Briefcase then calls IAM's `verify` itself, which consumes the proof, and only
then writes anything.

### Exchange request

```json
{
  "subject_token": "oat_…",
  "audience": "tos>briefcase",
  "endpoint_id": "briefcase.files.create",
  "metadata": {
    "path": "",
    "name": "report.pdf",
    "content_type": "application/pdf"
  },
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

`registered_path` is `/api/v1/obo/files` — the catalog path **including
`/api/v1`**, exactly as registered. The file never goes to IAM, only its digest.
The bytes travel straight to Briefcase, and the proof commits to what they will
be.

### Calling Briefcase

```http
POST /api/v1/obo/files
X-App-ID: tos>your-app
X-IAM-OBO-Access-Proof: obo_…
Content-Type: application/octet-stream

<the raw file bytes>
```

| Header | Rule |
| --- | --- |
| `X-App-ID` | Your canonical `{org_id}>{handle}`. Must equal IAM's `issuer_app_id`. |
| `X-IAM-OBO-Access-Proof` | The `access_proof` from the exchange; always starts `obo_`. |
| `X-Org-ID` | Optional. If sent, it must agree with the Team IAM reports. |
| `X-Briefcase-App-Secret` | Sandbox only; see [Sandboxes](#sandboxes). |
| `Authorization` | **Never.** A bearer alongside a proof is `400 ambiguous_authentication`. |

Returns `201` with the created entry.

## Two rules that break integrations

**Exact bytes.** Hash the file once and send that same buffer. Briefcase
recomputes the digest from what it actually received, and any difference is a
binding failure.

**One proof, one request.** A proof is valid for one verification or
60 seconds, whichever comes first. IAM consumes it exactly once; a retry is
indistinguishable from a replay and is refused as one. Disable automatic HTTP
retries. For a JSON mutation, keep its logical operation UUID and inputs but
mint a fresh proof. The one-shot endpoint has no separate logical retry UUID;
a fresh proof can create another version, so do not blindly resend it.

## One-shot upload in detail

Everything that decides where the file lands travels as proof-bound
**metadata**, not as a header or query parameter, so a proof you legitimately
obtained cannot be redirected somewhere else.

| Metadata key | Meaning |
| --- | --- |
| `path` | Destination folder. Empty selects the member's private Application folder. |
| `name` | The file name to create. |
| `content_type` | Media type of the bytes; empty defaults to `application/octet-stream`. |

- **Size ceiling: 5 TiB**, subject to quota and HTTP deadlines. The complete
  body must arrive before IAM verification and proof expiry. Use the staged
  protocol for large or slow transfers instead of relying on this ceiling.
- **Versioning is automatic.** A name an active file already carries publishes
  that file's next version rather than a duplicate.
- **The proof identifier is the idempotency key**, so a proof cannot create two
  files.
- **Quota is the member's.** The Team's storage and daily upload allowances
  apply exactly as they do to that member's own uploads.

This is not a recoverable operation: after an uncertain response, a fresh proof
is a *new* attempt, not an idempotent retry. Read back the destination folder
before re-sending if a duplicate would matter.

## Where files go

Every operation stays inside `apps/<calling-app-id>/`. The default empty
destination is `apps/<app-id>/private/<represented-member-id>`; a public
upload names `apps/<app-id>/public`. App folders are materialized on first use.
Normal private visibility, inherited grants and tag membership still apply.
An owner subject does not bypass the app namespace boundary. An entry's
originating-app metadata is attribution rather than a separate ownership rule.

## Critical sharing operations

Register `briefcase.invitations.create` at `POST /api/v1/obo/invitations` and
`briefcase.link_access.update` at `POST /api/v1/obo/link-access` as **critical**
IAM endpoints. They require user approval. Both use empty endpoint metadata;
the entire operation is bound into the exact JSON body SHA-256.

```json
{"operation_id":"<uuid>","entry_id":"<uuid>","invitation":{"principal":{"type":"carbon","id":"alex:tos"},"access":["read"],"inherit":true}}
```

```json
{"operation_id":"<uuid>","entry_id":"<uuid>","enabled":true}
```

Use `delegated::DelegatedInvite` and `delegated::DelegatedLinkAccess` in Rust,
or `briefcase app request invite --body manifest.json --describe` and
`briefcase app request link-access --body manifest.json --describe` in the CLI.
Mint a fresh IAM proof from the described binding, then repeat without
`--describe` and supply `--app-id`. Proofs are one-use, including uncertain
responses; keep the logical operation UUID and exact bytes for retries.

## Errors

Every error is `{"error": {"code": …, "message": …, "request_id": …}}`.

| Status | Typical code | Meaning and what to do |
| --- | --- | --- |
| `400` | `ambiguous_authentication` | You sent `Authorization` with a proof. Send only the proof. |
| `400` | `invalid_app_id`, `invalid_org_id` | Use the canonical `{org_id}>{handle}` and a valid Team ID. |
| `401` | `unauthenticated` | Proof missing, malformed, expired, already consumed, or refused by IAM — including altered bytes. **Do not retry** — mint a new proof. |
| `403` | `forbidden` | A binding failed (issuer ≠ `X-App-ID`, wrong audience, path or Team mismatch, wrong endpoint), authorization was undisclosed, or the member cannot write to the destination. |
| `404` | `not_found` | The destination folder is missing or invisible to the member — deliberately indistinguishable. Do not retry unchanged. |
| `413` | — | Body above the accepted size. |
| `422` | `invalid_obo_metadata`, `invalid_name`, `invalid_path`, `invalid_content_type` | The proof-bound metadata failed validation. Re-read the catalog and compare what you actually bound. |
| `429` / `507` | — | Rate limited, or the Team's storage allowance is exhausted. |
| `503` | — | IAM or a dependency is unavailable. This is *not* a refused proof; that is `401`. |

At the IAM exchange, expect `403` (Team mismatch, inactive membership, missing
`obo.issue`), `404` (unknown `endpoint_id` or audience), `409` (proof consumed,
or an idempotency key reused with different input), `410 proof_expired` (more
than 60 seconds elapsed), and `422` (metadata does not satisfy the schema).

## Sandboxes

A Briefcase testing environment is a full replica paired with one IAM test
plane. To use OBO there, send the IAM testing app secret for the plane paired with
the proof's IAM environment:

```http
X-Briefcase-App-Secret: <ask_ test Application secret>
```

The test app secret does not replace the proof, and a proof from the wrong plane cannot
fall back to production. Register the endpoint in the paired IAM test plane
before testing. Full setup is in the
[testing-environment guide](testing-environments.md).

## One-shot Rust call

Use the official `briefcase-client` package. It never sends your bearer on this
call, never retries it, and never stores a session.

```rust
use briefcase_client::OnBehalfOfUpload;

// `proof` is the access_proof you just exchanged for exactly these bytes.
let entry = client
    .create_file_on_behalf_of(
        &OnBehalfOfUpload::file("tos>your-app", proof, "./report.pdf"),
    )
    .await?;
```

`OnBehalfOfUpload::bytes(app_id, proof, bytes)` takes an in-memory body instead.
The proof is redacted in debug output and consumed by the call. Keep the source
file unchanged between hashing and sending, and use a real file — not a symlink,
pipe or device.

## One-shot CLI call

Useful for trying the flow before you write code:

```bash
briefcase app upload --app-id 'tos>your-app' ./report.pdf              # hidden proof prompt
briefcase app upload --app-id 'tos>your-app' --proof-stdin ./report.pdf < proof.txt
```

The destination, name and media type come from the proof, so the command takes
no path argument. Pass proofs through the hidden prompt or `--proof-stdin`,
never `--proof` in a shared shell, where the process list would expose them.

## One-shot checklist

- [ ] Same Team as Briefcase; subject token issued to your Application.
- [ ] `obo.issue`, `self.membership.read`, `self.identity.read` present.
- [ ] Digest taken over the exact bytes you will send.
- [ ] `/api/v1/obo/files` bound as the path, with its `/api/v1` prefix.
- [ ] Destination bound as metadata, not as a header or query parameter.
- [ ] Exchange immediately before the call; no proof cached or reused.
- [ ] `X-App-ID` + `X-IAM-OBO-Access-Proof` only — no `Authorization`.
- [ ] Automatic HTTP retries disabled for this route.

## Reference

- [API reference](api/README.md) — every operation, filters, errors, limits
- [IAM integration](iam-integration.md) — registration, scopes, webhook approval
- [Rust client](client/README.md) · [CLI](cli/README.md) · [Testing environments](testing-environments.md)
