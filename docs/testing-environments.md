# Testing environments

An IAM testing Application secret selects an isolated Briefcase environment:

```http
X-Briefcase-App-Secret: ask_<43 base64url characters>
Authorization: Bearer <IAM test access token>
X-Org-ID: tos
```

The secret selects the environment; the test actor's current IAM membership, role, tags and permissions determine what it can do. Production credentials do not authenticate a test actor. Unknown, invalid, deleted, or mismatched secrets never fall back to production.

Each environment has a **2 GiB** storage ceiling, including retained versions and reservations. Briefcase permits **10 active environments** across the deployment. Exceeding the test storage ceiling returns `In test enviorment you are limited to a total storage of 2gb per enviorment.`

## Create and manage in IAM

Create the environment, bootstrap test actors, and import `tos>briefcase` in IAM.
Give Briefcase the returned test `app_secret`. The first metadata lookup or sign-in
validates it with IAM and initializes Briefcase storage automatically. No manual
pairing, Briefcase production login, or IAM environment root key is needed.

Briefcase fixes the application ID to its configured IAM application and asks IAM
to authenticate the secret. IAM returns the environment UUID and authoritative
lifecycle metadata. Briefcase validates this on every selected request; a secret
prefix alone never selects a world or authorizes an actor.

IAM owns environment creation, identities, permissions, names, secret rotation,
retirement, restoration, and reset. Briefcase owns files, file grants, storage limits,
and provider cleanup. IAM name changes appear on next use. Rotation accepts the
new application secret automatically. Retirement blocks new requests; restoration
allows them again. After IAM resets and the application is reimported, Briefcase
erases the old files and identity projections before accepting the new world.
Provider object deletion runs through its durable cleanup queue.

Briefcase keeps its local storage record for recovery and cleanup; IAM retirement
does not immediately physically delete files. Local Briefcase cleanup and explicit
retirement are still available, and a locally retired record must be restored
explicitly. The local idle retirement policy applies only to legacy pairings.

## Optional creation through Briefcase

The API provisions the IAM application testing environment and then creates the empty Briefcase plane. Configure the production Briefcase application and its dependency catalog in IAM first. Production members authorized by IAM can create environments using:

```bash
briefcase env create integration --description 'Release integration tests'
```

Or `POST /organizations/{org_id}/testing-environments` with a production bearer, matching `X-Org-ID`, an `Idempotency-Key`, and:

```json
{"name":"integration","description":"Release integration tests"}
```

An optional `iam_test_key` joins an existing IAM dependency environment. The JSON result contains the environment metadata (`id`, `name`, `iam_environment_id`, and other fields) alongside `key`; **key is the IAM test application secret**, not a separate Briefcase-generated credential. The CLI stores it privately under the environment UUID. Reuse the same idempotency key and request after an uncertain response.

The optional key is the **32-character alphanumeric IAM environment root key**,
not an `ask_…` Application secret. Omit it to provision a new IAM test world.
For the CLI, supply `--iam-test-key` or set `BRIEFCASE_IAM_TEST_KEY` from your
secret manager before running `briefcase env create`. The Rust client exposes
the same option as `TestingEnvironmentCreate.iam_test_key`, typed as
`Option<IamEnvironmentKey>`.

Existing paired environments are supported and migrate to application-only validation on their next use. The backend uses the official IAM SDK for provisioning and verification.

## Test sign-in

In testing, **SLT accepts either an IAM-issued test login code or an existing
Carbon/Silicon public ID**, such as `alice` or `worker:tos`. Select the paired
environment with its app secret and send either value in the existing `slt` field:

```http
POST /api/v1/auth/slt
X-Briefcase-App-Secret: ask_<43 base64url characters>
Idempotency-Key: test-login-operation-0001
Content-Type: application/json

{"slt":"worker:tos"}
```

The ID shortcut signs in as that actor in the paired IAM world, selecting its
current active organizations and Briefcase’s approved scopes. An IAM-issued
SLT retains the organization grants selected during that IAM login. Both paths
issue the same access/refresh session. Its current memberships, roles and tags determine file permissions.
The actor must already exist in that IAM world. Production login still requires
a one-time IAM login code; a public actor ID never authenticates production.
Reuse the exact input, app secret and idempotency key after an uncertain exchange.

## CLI

```bash
briefcase --test <environment-id> login <test-actor-id>
briefcase --test <environment-id> ls
briefcase --test <environment-id> put ./fixture.txt private/me:tos
briefcase --test <environment-id> usage --json
```

Pass the app secret directly when the UUID is not saved locally:

```bash
export BRIEFCASE_APP_SECRET='ask_…'
briefcase --org tos login <test-actor-id>
briefcase --org tos ls
unset BRIEFCASE_APP_SECRET
```

`--app-secret` is the equivalent explicit option. The CLI resolves and stores the environment mapping, then uses that environment's own login session. Production sessions remain separate. Every invocation selected into testing prints a footer on **stderr**, including errors; JSON and file bytes on stdout remain parseable.

## Rust client

```rust,no_run
use briefcase_client::{Client, Config, EnvironmentKey, IdempotencyKey, ListEntries};
# async fn example() -> briefcase_client::Result<()> {
let config = Config::new("https://backend.briefcase.teamofsilicons.com/api/v1/", "tos")?
    .with_environment(EnvironmentKey::new(std::env::var("BRIEFCASE_APP_SECRET").unwrap())?);
let client = Client::connect(config.clone()).await?;
// Persist this key before exchange and reuse it after an uncertain result.
let login_key = IdempotencyKey::random();
let session = client.login_with_slt_with_key("worker:tos", &login_key).await?;
let client = Client::connect(config.with_token(session.access_token)).await?;
let page = client.list_entries(&ListEntries::default()).await?;
# Ok(())
# }
```

`EnvironmentKey` validates and redacts the app secret. `IamEnvironmentKey` is the distinct 32-character IAM root key used only for optional dependency provisioning or pairing replacement. The library holds no session store; callers own token refresh and persistence.

## Browser

Open **Test environments** in the sidebar. Choose **Create**, provide a name
and optional description, and optionally enter an existing root key in
**IAM test key (optional)**. Leave it blank for a new IAM test world. The field
is masked, validates the root-key format, and is cleared when its dialog closes.
An uncertain create request keeps its exact input and operation ID for retry.

Under **Enter with an app secret**, enter the test app secret and either value in
**IAM test SLT or Carbon/Silicon ID**. The same masked field appears when opening an
environment with **View as testing environment**. The gateway keeps credentials server-side and attaches the
test session to the existing browser session. A persistent testing banner
identifies the environment. Exit returns the tab to production without signing
out the production session. A tab stores only the public environment UUID,
never its app secret. The drawer clears entered credentials when it closes.

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

Rotate test credentials **in IAM**, then use the new app secret directly. Discovery updates the stored credential automatically; the old one fails IAM validation. `briefcase env pair-iam` remains available for legacy management. There is no independent Briefcase key-rotation endpoint. Pairing replacement cannot transfer existing identity-bound data to a different IAM environment. Use a new environment when identities change.

Retirement immediately invalidates local access. Restoration requires an active, valid IAM pairing and reactivates its current app secret. IAM retirement or secret invalidation also prevents further data-plane requests because Briefcase validates the live testing context. Briefcase does not silently restore or erase a shared IAM dependency graph. Legacy paired Briefcase planes retire after 30 idle days and retain their recorded recovery deadline; metadata responses report the authoritative `purge_after`.

## Storage and webhook isolation

Use a separate PostgreSQL database for testing, distinct roles, encrypted environment credentials, and environment-specific S3 prefixes. Startup verifies that production and test DSNs resolve to different actual databases. IAM signs test webhook envelopes; Briefcase verifies the raw signature, matches the root digest returned by live IAM application validation, and routes only to that environment. A public UUID is never an authorization credential.

Testing notifications and email outbox records are local to the test plane. The worker does not send test invitations to real email addresses. Cleaning, retirement and in-flight requests use database lifecycle fences so an old request cannot repopulate a cleaned plane.
