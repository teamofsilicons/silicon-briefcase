# Testing environments

Honeycomb owns shared environment creation, application imports, credential rotation,
cleaning, disabling, restoration and expiry. IAM authenticates test identities and
OBO proofs. Briefcase owns isolated files, permissions, storage limits and cleanup.

## Create and manage through Honeycomb

Install and sign in to the official [Honeycomb CLI](https://docs.honeycomb.teamofsilicons.com/installation/),
then run its environment commands directly or through Briefcase:

```bash
briefcase env manage create tos integration --description 'Release integration tests'
briefcase env manage list
briefcase env manage get <environment-id>
briefcase env manage import <environment-id> 'briefcase' --revision <current-revision>
briefcase env manage action <environment-id> clean --revision <current-revision>
```

`briefcase env manage <arguments...>` invokes `honeycomb environments <arguments...>`
through the Rust client's `honeycomb::manage_environment`. It uses Honeycomb's own
saved session, authorization and retry state. Do not pass Briefcase bearer tokens,
`--test`, or app secrets to this management command. Read the current revision
before each mutation and follow Honeycomb's recovery instructions after an uncertain
result. [Honeycomb's testing guide](https://docs.honeycomb.teamofsilicons.com/testing-environments/)
describes imports, dependency pins, root-key retrieval and lifecycle actions.

Honeycomb's 32-character alphanumeric `testing_key` grants test-world administration.
The imported Briefcase application's IAM `app_secret` selects its sandbox and does
not grant a signed-in actor administrator permissions. No manual pairing or root-key
entry is needed for ordinary Briefcase use.

The old direct Briefcase management commands and HTTP routes return
`testing_environment_managed_by_honeycomb` with recovery guidance. `briefcase env current`
still reads the selected Briefcase test environment. Use Honeycomb for lifecycle
management, including cleaning; an app secret cannot authorize local self-service cleanup.

## Select Briefcase's test plane

```http
X-Briefcase-App-Secret: ask_<43 base64url characters>
Authorization: Bearer <IAM test access token>
X-Org-ID: interface-test-org
```

Briefcase validates the app secret with IAM for `briefcase` and discovers the
shared environment automatically. The test actor's current membership, role, tags
and permissions determine its file access. Production credentials do not authenticate
a test actor. Unknown, invalid, disabled, deleted or mismatched secrets fail without
falling back to production.

The environment's production owner (for example, `tos`) manages its lifecycle
through Honeycomb.
`X-Org-ID` selects the **data organization inside that world** (for example,
`interface-test-org`); these IDs do not need to match. Each bearer or OBO request
must receive current IAM authority for that exact data organization, application
and world. Briefcase never substitutes the control owner for the selected org.
Files and upload capabilities remain partitioned by both world UUID and data org;
reset/version fences still invalidate earlier requests and reservations.

Public testing URLs carry the world UUID and data org, never a credential.
Resolving this routing context grants no access: only entries with public-link
policy are visible under that world's tenant RLS. A private path, wrong world,
missing world selector, pending reset, or inactive world remains unavailable.

Each environment has a **2 GiB** storage ceiling, including retained and deleted
versions and upload reservations. Briefcase permits **10 active environments**
across the deployment; restoration also requires capacity. Exceeding test storage
returns `In test enviorment you are limited to a total storage of 2gb per enviorment.`

## Test sign-in

In testing, **SLT accepts either an IAM-issued test login code or an existing
Carbon/Silicon public ID**, such as `c:alice` or `si:worker`. Select the paired
environment with its app secret and send either value in the existing `slt` field:

```http
POST /api/v1/auth/slt
X-Briefcase-App-Secret: ask_<43 base64url characters>
Idempotency-Key: test-login-operation-0001
Content-Type: application/json

{"slt":"si:worker"}
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
let session = client.login_with_slt_with_key("si:worker", &login_key).await?;
let client = Client::connect(config.with_token(session.access_token)).await?;
let page = client.list_entries(&ListEntries::default()).await?;
# Ok(())
# }
```

`EnvironmentKey` validates and redacts the app secret. The library holds no session store; callers own token refresh and persistence. Root environment administration belongs to Honeycomb, not a Briefcase test session.

## Browser

From the sign-in screen, choose **Sign in to a test environment** and enter the
app secret and IAM-issued test SLT or existing actor ID. No production login is
required. The organization is optional; IAM supplies the granted workspaces.

When signed in to production, open **Test environments** in the sidebar. Choose
**Manage environments in Honeycomb** to administer the shared world, or enter the
Briefcase app secret and **Test SLT or Carbon/Silicon ID** to enter an existing one.
Credentials are masked and cleared when the drawer closes. An uncertain exchange
keeps its operation identity for retry with the same input.

The gateway keeps credentials server-side. The tab stores only the public environment
UUID. A persistent testing banner shows the environment, signed-in identity and exit
control. Production and testing sessions use separate HttpOnly cookies; leaving
testing restores the production session or asks you to sign in.

## Lifecycle and authority

Honeycomb sends authenticated participant operations independently of member test
sessions. The shared `environment_id`, lifecycle revision, cleaning generation and
key version fence stale work. Cleaning blocks access, clears files, versions, grants,
uploads and other test records, and reports completion only after provider cleanup.
Old requests, jobs or webhook generations cannot repopulate cleared data.

Disabling blocks access immediately. Restoration makes retained data available when
authorized, but never reverses a clean. Honeycomb decides inactivity and recovery
policy; Briefcase reports activity and executes the requested lifecycle operations.
Shared environments are excluded from Briefcase's legacy idle-retirement policy.
A public environment UUID conveys no authority.

See [Honeycomb participant integration](honeycomb-integration.md) for the protected
service contract, required configuration and deployment acceptance boundaries.

## Storage and webhook isolation

Use a separate PostgreSQL database for testing, distinct roles, encrypted environment credentials, and environment-specific S3 prefixes. Startup verifies that production and test DSNs resolve to different actual databases. IAM signs test webhook envelopes; Briefcase verifies the raw signature, matches the root digest returned by live IAM application validation, and routes only to that environment. A public UUID is never an authorization credential.

Testing notifications and email outbox records are local to the test plane. The worker does not send test invitations to real email addresses. Cleaning, retirement and in-flight requests use database lifecycle fences so an old request cannot repopulate a cleaned plane.

## Share a sandbox file or folder

Enable “Anyone with the link can view” using the same share command as production.
The returned URL contains `?test_environment=<public-UUID>`, never an app secret.
Anonymous reads, folder navigation, previews and downloads preserve that selector.
A missing, retired or mismatched sandbox returns an error without trying production.
The UUID selects a sandbox; each entry must still have effective public-link access.

SDK consumers can use `Config::with_public_testing_environment(id)` with the
`public_entry`, `public_children`, `public_content` and `public_download` methods.
This setting applies only to anonymous reads and grants no signed-in actor authority.
The HTTP public endpoint accepts the same `test_environment` query parameter.

## Backend regression coverage

`cargo test --locked --lib imported_world -- --test-threads=1` exercises real
PostgreSQL control/data databases under the restricted `briefcase_api` role,
with IAM and object storage served by local HTTP fixtures. Set
`BRIEFCASE_TEST_CONTROL_DATABASE_URL` and `BRIEFCASE_TEST_DATA_DATABASE_URL`
to disposable migrated-admin databases; without both, these checks skip.
The cases cover separate control/data organizations, bearer and delegated
folder/raw recording paths, wrong world/org/audience/actor, missing disclosure
or endpoint scope, rejected proofs, exact body binding, public/private links,
and generation/reset invalidation. They do not attest deployed provider access.
