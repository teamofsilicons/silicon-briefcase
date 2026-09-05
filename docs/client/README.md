# Using the `briefcase-client` package

The Rust package is the primary interface to Silicon Briefcase. Everything the
service exposes to a client is a method on one type, and nothing the service
does internally is reachable from here.

Its Briefcase behavior is **stateless**. It holds no login session or API cache:
a `Config` goes in, a `Client` comes out, and access/refresh tokens or
environment UUID-to-key mappings that survive between runs belong to the
calling program. The one intentionally process-external behavior is dependency
maintenance: by default a first ordinary request performs a best-effort
crates.io check and may advance `briefcase-client` in the nearest consuming
`Cargo.lock`.

```toml
[dependencies]
briefcase-client = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

See the [documentation index](../README.md) and [paired testing-environment guide](../testing-environments.md). Production Briefcase's canonical IAM ID is `tos>briefcase`; example member paths elsewhere in this guide must be replaced with real IAM public IDs.

## Connecting

```rust
use briefcase_client::{Client, Config};

let client = Client::connect(
    Config::new("https://backend.briefcase.teamofsilicons.com/api/v1/", "tos")?
        .with_token(token),
)
.await?;
```

`Config::new` accepts the exact versioned base this build speaks
(`/api/v1/`), without a query, fragment, or embedded user information. HTTPS
is required except for `localhost` and loopback IPs, which keeps local tests
practical without allowing a credential to cross a clear-text network.
HTTP redirects are not followed, so a response cannot forward a bearer, test
root, rotating token, or mutation body beyond that configured origin.

`connect` reads `GET /api/version` before anything else. It requires the
selected API major in the `Briefcase-API-Version` response header and JSON body
to agree, then verifies service identity, the selected/supported major, and
the exact revision, method, and path of every operation this build calls.
Duplicate IDs are refused; unknown operation IDs are compatible additive
capabilities. A deployment that changed a used operation fails here, naming
what moved, rather than halfway through a later call:

```text
briefcase serves a contract this client was not built for (serving v1);
listEntries is 1.1.0 here and 1.0.0 there
```

`Client::new_unchecked` skips the check when a caller has already decided the
pairing is acceptable. `Config` also carries the deadlines: `request_timeout`
for ordinary calls, `transfer_timeout` for anything moving file bytes (fifteen
minutes by default, because a whole file travels in one request), and
`connect_timeout`.

A `Client` is cheap to clone and shares one connection pool, so build it once.

## IAM short-lived-token login

Do not collect a Carbon/Silicon password, OTP, or the Briefcase IAM Application
secret. Obtain an organization-bound IAM short-lived token (SLT) minted for
Briefcase's canonical `{org_id}>{handle}` Application ID and give only that
one-use value to the Briefcase backend. With the IAM CLI, select the same
organization passed to `Config::new`:

```bash
iam --org tos login --app-id 'tos>briefcase'
```

```rust
use briefcase_client::{Client, Config};

let anonymous = Client::connect(
    Config::new("https://backend.briefcase.teamofsilicons.com/api/v1/", "tos")?
).await?;
let tokens = anonymous.login_with_slt(&slt).await?;

let signed_in = Client::connect(
    Config::new("https://backend.briefcase.teamofsilicons.com/api/v1/", "tos")?
        .with_token(tokens.access_token.clone()),
).await?;

// The package deliberately does not store or rotate behind your back.
// Persist both returned tokens securely, then replace both after refresh.
let next = anonymous.refresh_session(&tokens.refresh_token).await?;
# let _ = (signed_in, next);
```

An SLT lasts two minutes and is single-use. A successful refresh rotates the
refresh token, so persist the returned pair before doing later work. The
Application secret used to exchange/introspect with IAM is configured only on
the Briefcase backend and is absent from every client method and response.
The package rejects an exchange or refresh response whose `org_id` is missing
or differs from the organization in `Config`, before returning either token to
the caller.
For crash-safe retries, use `login_with_slt_with_key` or
`refresh_session_with_key`, persist the 16–255-byte `IdempotencyKey` beside the
credential before sending, and reuse that exact pair after an uncertain
transport outcome. Never retry a spent credential with a new key.

## Testing environments

Plane selection and actor authentication are separate. `EnvironmentKey`
selects one isolated Briefcase plane through `X-Testing-Environment-Key`; the
IAM bearer still says which Carbon or Silicon acts inside it. Every ordinary
method and route remains unchanged.

First create an IAM test environment in production IAM, bootstrap its test
identity, and import the canonical production Briefcase Application (for
example, `tos>briefcase`) into that IAM plane with IAM's
`import_from_production`/`iam --test ... app import` flow.
Persist the IAM environment UUID/key and the fresh test-only imported
Application ID/secret. Then create the paired Briefcase environment from a
production-authenticated Briefcase client:

```rust
use briefcase_client::{
    ApplicationId, Client, Config, IamApplicationSecret, IamEnvironmentKey,
    TestingEnvironmentCreate,
};

let request = TestingEnvironmentCreate::new(
    "checkout-e2e",
    iam_environment_id,
    IamEnvironmentKey::new(iam_environment_key)?,
    ApplicationId::new("tos>briefcase")?,
    IamApplicationSecret::new(iam_test_app_secret)?,
).described("ephemeral integration run");

let created = production.create_testing_environment(&request).await?;
// created.environment.id is safe metadata; created.key is a secret.
// The response never echoes either IAM secret.

let sandbox = Client::connect(
    Config::new("https://backend.briefcase.teamofsilicons.com/api/v1/", "tos")?
        .with_environment(created.key.clone())
        .with_token(test_iam_access_token),
).await?;
sandbox.list_entries(&briefcase_client::ListEntries::default()).await?;
```

With IAM 1.2's complete online authorization snapshots, first use and post-clean bootstrap do not wait for a webhook. Exchange a test SLT and make a bearer-authenticated `list_entries` call. Do not mutate a member's profile just to force a bootstrap event. Signed webhooks still reconcile other members and resource lifecycle changes; pending production webhook review is documented in the [IAM runbook](../iam-integration.md).

The management surface is:

| Task | Client method |
| --- | --- |
| List/create | `testing_environments`, `create_testing_environment` |
| Read/update | `testing_environment`, `update_testing_environment` |
| Retrieve/rotate root key | `testing_environment_key`, `rotate_testing_environment_key` |
| Replace paired IAM plane | `replace_testing_environment_iam_pairing` |
| Erase contents | `clean_testing_environment` |
| Retire/restore | `delete_testing_environment`, `restore_testing_environment` |
| Root-key-only self service | `current_testing_environment`, `clean_current_testing_environment` |

Every environment mutation also has a `_with_key` form: create, update,
delete, restore, root-key rotation, IAM re-pairing, managed cleaning, and
root-key-only cleaning. The short convenience forms generate a new
`IdempotencyKey` and are
appropriate for an attempt whose response is observed immediately. A durable
caller should generate and persist the key together with the complete request
before sending, call the matching `_with_key` method, and reuse that exact key
and request after an uncertain transport result. This is especially important
for create, restore, and root-key rotation because replaying the original
mutation is how a generated root key is recovered without generating another.

The two self-service methods fail locally with `this action is only possible
for a test environment` when no key is configured. Production credentials do
not become test credentials, test credentials do not work in production, and
the IAM and Briefcase root keys are different secret types by design.
Likewise, all UUID-addressed management methods fail locally when the `Config`
contains a testing key; construct a production client for lifecycle work.

### Hands-on verification

Use the [read-only sandbox example](examples/sandbox.rs) with a test bearer and its paired Briefcase root key. It negotiates the contract, describes the selected plane, and lists one page. It prints no credentials and performs no cleanup or lifecycle mutation. See the [testing guide](../testing-environments.md) for a manual checklist.

## Automatic package maintenance

On the first ordinary request for a client (clones share the result), it checks
crates.io with a short timeout. Contract negotiation and IAM SLT, refresh, and
OBO exchanges deliberately defer maintenance: their short-lived or one-use
credentials are sent immediately, and the next ordinary request performs the
due check. If a newer stable `briefcase-client` exists and a consuming
`Cargo.toml` can be found, it runs an exact `cargo update -p briefcase-client
--precise <version>`; the current process keeps its compiled version and the
next build uses the updated lockfile. Network/Cargo failures never fail the API
request and are visible through `client.update_status()`.

```rust
let managed = Config::new(base, org)?
    .with_update_manifest("./Cargo.toml");
let caller_managed = Config::new(base, org)?
    .with_auto_update(false);
```

`BRIEFCASE_CLIENT_AUTO_UPDATE=off` is the process-wide opt-out. Use it for
reproducible builds, read-only source trees, and programs whose dependency
automation is owned elsewhere. The `briefcase` CLI disables this embedded hook
because it maintains its complete installed binary separately.

## Addressing entries

Every entry has a stable `Uuid` and an organization-relative path — the same
path its permanent URL shows. Both work:

```rust
use briefcase_client::Destination;

let by_path = client.entry_at("private/cos:tos/notes/report.pdf").await?;
let by_id = client.entry(by_path.id).await?;
let folder = Destination::path("private/cos:tos/notes");
```

## Browsing

```rust
use briefcase_client::ListEntries;

// One folder, one page.
let page = client.list_entries(&ListEntries::in_folder(folder.clone())).await?;

// Everything reachable that matches a filter, following every page.
let markdown = client
    .list_all_entries(&ListEntries::matching("is:md location:'public'"), 1_000)
    .await?;
```

Entries the caller may not see are already gone from the answer, and a page is
refilled rather than answered short, so a full page means what it says. Follow
`next_cursor` until it is `None` to walk a folder by hand.

The filter language is the service's, documented in
[API guide](../api/README.md):
`is:`, `name:`, `contains:`, `has:`, `location:`, `from:`, `to:`, `for:`,
`permissions:`, `before:`, `after:`, `between:`, `first:`, `last:`, `sort:`,
combined with `and`, `or`, `not`, and parentheses.

## Files

One operation uploads a file of any size; the service decides internally
whether the bytes travel as a single request or a durable multipart transfer.

```rust
use briefcase_client::{ByteRange, Upload, guess_content_type};

let upload = Upload::file(folder.clone(), "./report.pdf")?
    .with_content_type(guess_content_type("report.pdf"));
let entry = client.upload(&upload).await?;

// Uploading the same name again publishes that file's next version and
// returns the same entry.
let versions = client.versions(entry.id).await?;
client.restore_version(entry.id, versions[1].id).await?;

// Bytes come back as a stream, whole or one range at a time.
client.download_to_file(entry.id, "./local-copy.pdf").await?;
let head = client
    .read_content(entry.id, Some(ByteRange::inclusive(0, 1023)))
    .await?
    .bytes()
    .await?;
```

Uploads are idempotent: the client generates a key per call, and
`Upload::with_idempotency_key` lets a caller supply their own when the retry
happens in their process rather than inside this one. Version restoration has
the same durable form as `restore_version_with_key`.

## Folders and sharing

```rust
use briefcase_client::{AccessRight, ActorRef, NewFolder, NewGrant, RootType};

// At the organization base, a declared kind chooses the container: Public,
// the caller's own folder inside Private, or that tag's folder.
let notes = client
    .create_folder(&NewFolder::at_base("notes", RootType::Private))
    .await?;

let grant = client
    .grant(
        notes.id,
        &NewGrant::new(ActorRef::carbon("cos:tos"), [AccessRight::Read, AccessRight::Write])
            .inheriting(),
    )
    .await?;
client.revoke(notes.id, grant.id).await?;
```

Request access to a permanent-URL path without first resolving metadata the
caller is not allowed to see:

```rust
use briefcase_client::{AccessRight, NewAccessRequest};

let wanted = NewAccessRequest::new([AccessRight::Read])
    .because("reviewing the handbook");
let pending = client
    .request_access_by_path("private/cos:owner/handbook.pdf", &wanted)
    .await?;
# let _ = pending;
```

The path operation returns the same access-request record as
`request_access(entry_id, ...)`, without returning the entry's name, owner, or
other metadata. Use `request_access_by_path_with_key` with a key persisted
beside the exact path, rights, and reason when the caller must safely recover
an uncertain result.

For a caller-managed crash retry, attach a persisted key with
`NewFolder::with_idempotency_key`, and use `update_entry_with_key` for rename or
move. Reuse both the same key and the same request after an uncertain result.

The rights are independent. `write` adds content that is not there yet;
`update` changes content that is; neither implies `delete`. Granting a member
who already holds a grant amends it in place, so widening access never has to
pass through a revocation.

## Reading the answers

```rust
match client.entry_at(path).await {
    Ok(entry) => { /* ... */ }
    Err(error) if error.is_not_found() => {
        // Also what a hidden entry looks like: Briefcase never confirms that
        // an entry the caller may not read exists.
    }
    Err(error) if error.code() == Some("daily_upload_limit_exhausted") => {
        let wait = error.retry_after();
    }
    Err(error) => return Err(error),
}
```

`Error::code` carries the service's stable code, `is_not_found`,
`is_forbidden`, `is_unauthenticated`, and `is_retryable` cover the common
branches, and `retry_after` carries the delay a spent allowance names.

## Applications

An application never uses the bearer surface. It obtains a single-use proof
from IAM over the exact bytes it is about to send, and calls one operation:

```rust
use briefcase_client::OnBehalfOfUpload;

let entry = client
    .create_file_on_behalf_of(&OnBehalfOfUpload::file("tos>app-notes", proof, "./generated.md"))
    .await?;
```

The destination, name, and media type travel inside the proof rather than in
the request, so an application cannot redirect a proof it legitimately
obtained. The client never sends its own bearer token here, because presenting
both credentials at once is a request error. A refused proof must never be
retried: IAM consumes it exactly once.

## Everything else

| What you want | Method |
| --- | --- |
| The deployment's contract | `version`, `health`, `ready` |
| List, filter, walk | `list_entries`, `list_all_entries` |
| One entry | `entry`, `entry_at`, `permanent_url` |
| Create, rename, move, delete | `create_folder`, `update_entry`, `delete_entry` |
| Bytes | `upload`, `read_content`, `read_content_at`, `download`, `download_to_file` |
| Versions | `versions`, `restore_version` |
| Sharing | `permissions`, `grant`, `revoke`, `effective_access` |
| Access requests | `request_access`, `request_access_by_path`, `decide_access_request` |
| Inbox | `notifications`, `mark_notifications_read` |
| History | `activity` |
| Search | `search` |
| Bin | `bin`, `restore_from_bin` |
| Consumption | `usage` |
| Organization storage | `configure_storage` |
| Applications | `create_file_on_behalf_of` |
| IAM SLT session | `login_with_slt`, `refresh_session` |
| Testing environments | `testing_environments`, `create_testing_environment`, lifecycle/key/self methods |
