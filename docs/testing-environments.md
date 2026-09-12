# Testing environments

An IAM testing Application secret selects an isolated Briefcase environment:

```http
X-Briefcase-App-Secret: ask_<43 base64url characters>
Authorization: Bearer <IAM test access token>
X-Org-ID: tos
```

The secret selects the environment; the test actor's current IAM membership, role, tags and permissions determine what it can do. Production credentials do not authenticate a test actor. Unknown, invalid, deleted, or mismatched secrets never fall back to production.

Each environment has a **2 GiB** storage ceiling, including retained versions and reservations. Briefcase permits **10 active environments** across the deployment. Exceeding the test storage ceiling returns `In test enviorment you are limited to a total storage of 2gb per enviorment.`

## Create a paired environment

The API provisions the IAM application testing environment and then creates the empty Briefcase plane. Configure the production Briefcase application and its dependency catalog in IAM first. Production members authorized by IAM can create environments using:

```bash
briefcase env create integration --description 'Release integration tests'
```

Or `POST /organizations/{org_id}/testing-environments` with a production bearer, matching `X-Org-ID`, an `Idempotency-Key`, and:

```json
{"name":"integration","description":"Release integration tests"}
```

An optional `iam_test_key` joins an existing IAM dependency environment. The result contains `environment` and `key`; **key is the IAM test application secret**, not a separate Briefcase-generated credential. The CLI stores it privately under the environment UUID. Reuse the same idempotency key and request after an uncertain response.

IAM currently requires its environment root key together with the test Application secret when a service validates the testing context. Briefcase stores that pairing encrypted, so callers only pass the app secret. A secret from an arbitrary, unregistered IAM environment cannot independently bootstrap Briefcase: create/register the paired environment through this flow first. The backend uses the official IAM SDK for provisioning and verification.

## CLI

```bash
briefcase --test <environment-id> login <test-slt>
briefcase --test <environment-id> ls
briefcase --test <environment-id> put ./fixture.txt private/me:tos
briefcase --test <environment-id> usage --json
```

Pass the app secret directly when the UUID is not saved locally:

```bash
export BRIEFCASE_APP_SECRET='ask_…'
briefcase --org tos login <test-slt>
briefcase --org tos ls
unset BRIEFCASE_APP_SECRET
```

`--app-secret` is the equivalent explicit option. The CLI resolves and stores the environment mapping, then uses that environment's own login session. Production sessions remain separate. Every invocation selected into testing prints a footer on **stderr**, including errors; JSON and file bytes on stdout remain parseable.

## Rust client

```rust,no_run
use briefcase_client::{Client, Config, EnvironmentKey, ListEntries};
# async fn example() -> briefcase_client::Result<()> {
let client = Client::connect(
    Config::new("https://backend.briefcase.teamofsilicons.com/api/v1/", "tos")?
        .with_environment(EnvironmentKey::new(std::env::var("BRIEFCASE_APP_SECRET").unwrap())?)
        .with_token(std::env::var("BRIEFCASE_TEST_TOKEN").unwrap()),
).await?;
let page = client.list_entries(&ListEntries::default()).await?;
# Ok(())
# }
```

`EnvironmentKey` validates and redacts the app secret. `IamEnvironmentKey` is the distinct 32-character IAM root key used only for optional dependency provisioning or pairing replacement. The library holds no session store; callers own token refresh and persistence.

## Browser

In organization settings, open Testing environments. Enter the test app secret and a fresh IAM test sign-in token. The gateway keeps credentials server-side and attaches the test session to the existing browser session. A persistent testing banner identifies the environment. Exit returns the tab to production without signing out the production session. A tab stores only the public environment UUID, never its app secret.

## Lifecycle and authority

| Action | Route beneath `/organizations/{org_id}/testing-environments` |
| --- | --- |
| List active/deleted/all | `GET /?status=active|deleted|all` |
| Read metadata | `GET /{id}` |
| Rename/describe | `PATCH /{id}` with strong `If-Match` |
| Read selected app secret | `GET /{id}/key` |
| Replace IAM pairing | `POST /{id}/iam-pairings` |
| Erase Briefcase data | `POST /{id}/cleanings` |
| Retire locally | `DELETE /{id}` |
| Restore during recovery window | `POST /{id}/restorations` |

Management uses a production actor session and creator/administrator authorization. Data cleaning is also available to a holder of the test secret at `POST /testing-environment/cleanings`; it requires an idempotency key. This erases isolated Briefcase data and schedules provider cleanup without deleting the IAM dependency environment.

Rotate test credentials **in IAM**, then replace the entire paired credential set using `briefcase env pair-iam`. The new app secret immediately replaces the selector; the old one fails. There is no independent Briefcase key-rotation endpoint. Pairing replacement cannot transfer existing identity-bound data to a different IAM environment. Use a new environment when identities change.

Retirement immediately invalidates local access. Restoration requires an active, valid IAM pairing and reactivates its current app secret. IAM retirement or secret invalidation also prevents further data-plane requests because Briefcase validates the live testing context. Briefcase does not silently restore or erase a shared IAM dependency graph. Idle Briefcase planes retire after 30 days and retain their recorded recovery deadline; metadata responses report the authoritative `purge_after`.

## Storage and webhook isolation

Use a separate PostgreSQL database for testing, distinct roles, encrypted environment credentials, and environment-specific S3 prefixes. Startup verifies that production and test DSNs resolve to different actual databases. IAM signs test webhook envelopes; Briefcase verifies the raw signature, matches the encrypted IAM root, and routes only to the paired environment. A public UUID is never an authorization credential.

Testing notifications and email outbox records are local to the test plane. The worker does not send test invitations to real email addresses. Cleaning, retirement and in-flight requests use database lifecycle fences so an old request cannot repopulate a cleaned plane.
