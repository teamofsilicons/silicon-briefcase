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

## What Briefcase exposes

One endpoint. Discover it with
`GET {iam}/api/v1/obo-access/applications/tos>briefcase/endpoints`
(URL-encode the `>`).

| Endpoint ID | Method and registered path | Metadata schema |
| --- | --- | --- |
| `briefcase.files.create` | `POST /api/v1/obo/files` | `path`, `name`, `content_type` — all required strings |

Briefcase serves other `/obo/` routes, but they are not in its IAM catalog, so
no proof can be bound to them and the exchange answers `404 not_found`. This
endpoint is the delegated surface today.

## Before you start

You need all five of these. Briefcase fails closed on any of them.

1. **Your Application is registered in IAM, in the same Team as Briefcase.**
   OBO never crosses a Team. IAM derives the Team from the two Applications and
   refuses to accept one from you.
2. **A subject token: the member's IAM access token, issued to *your*
   Application** (`oat_…`). A token issued to some other Application is refused.
3. **`obo.issue` on that subject token.** Without it the exchange answers
   `403 obo_subject_token_forbidden`.
4. **`roles.read` and `memberships.read` disclosure**, in both the subject token
   and Briefcase's currently approved scopes. Briefcase requires the delegated
   authorization snapshot and answers `403` when role or membership disclosure
   is missing. Never infer authority from an undisclosed (`null`) field.
5. **Your Application's own secret**, used only for HTTP Basic and the HMAC on
   the IAM exchange. It is never sent to Briefcase.

The member must be an active member of your Application's Team at verification
time. Ending a session does not by itself extend or revoke IAM authority.

## The shape of the call

Four steps, in this order, every time:

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
| `X-Testing-Environment-Key` | Sandbox only; see [Sandboxes](#sandboxes). |
| `Authorization` | **Never.** A bearer alongside a proof is `400 ambiguous_authentication`. |

Returns `201` with the created entry.

## Two rules that break integrations

**Exact bytes.** Hash the file once and send that same buffer. Briefcase
recomputes the digest from what it actually received, and any difference is a
binding failure.

**One proof, one request, no retry.** A proof is valid for one verification or
60 seconds, whichever comes first. IAM consumes it exactly once; a retry is
indistinguishable from a replay and is refused as one. Exempt this call from any
automatic HTTP retry layer. To try again, mint a fresh proof.

## The operation in detail

Everything that decides where the file lands travels as proof-bound
**metadata**, not as a header or query parameter, so a proof you legitimately
obtained cannot be redirected somewhere else.

| Metadata key | Meaning |
| --- | --- |
| `path` | Destination folder. Empty selects the member's private Application folder. |
| `name` | The file name to create. |
| `content_type` | Media type of the bytes; empty defaults to `application/octet-stream`. |

- **Any supported size**, up to 5 TiB. Briefcase decides internally how to store
  it; you always send one request body.
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

An empty `path` selects the represented member's private Application folder,
`private/{actor}/apps/{app_id}`. It is created on first use and reserved from
then on, so each Application keeps its own space inside the member's storage —
this is the destination to use unless you have a reason not to.

Any other `path` must name an existing folder the member may add content to.
Their own permissions decide, using the current role and tags IAM disclosed for
this exact request, so a tag-scoped destination works when the member's tags
reach it. Briefcase does not create intermediate folders for you.

Every file records the Application that acted, and appears in the entry's
history alongside the member who was represented.

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
plane. To use OBO there, send the Briefcase root key for the plane paired with
the proof's IAM environment:

```http
X-Testing-Environment-Key: <32-character Briefcase root key>
```

The root key does not replace the proof, and a proof from the wrong plane cannot
fall back to production. Register the endpoint in the paired IAM test plane
before testing. Full setup is in the
[testing-environment guide](testing-environments.md).

## Rust

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

## CLI

Useful for trying the flow before you write code:

```bash
briefcase app upload --app-id 'tos>your-app' ./report.pdf              # hidden proof prompt
briefcase app upload --app-id 'tos>your-app' --proof-stdin ./report.pdf < proof.txt
```

The destination, name and media type come from the proof, so the command takes
no path argument. Pass proofs through the hidden prompt or `--proof-stdin`,
never `--proof` in a shared shell, where the process list would expose them.

## Checklist

- [ ] Same Team as Briefcase; subject token issued to your Application.
- [ ] `obo.issue`, `roles.read`, `memberships.read` present.
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
