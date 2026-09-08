# Using the `briefcase-client` package

The Rust package is the primary interface to Silicon Briefcase. Everything the
service exposes to a client is a method on one type, and nothing the service
does internally is reachable from here.

Its Briefcase behavior is **stateless**. It holds no login session or API cache:
a `Config` goes in, a `Client` comes out, and access/refresh tokens or
environment UUID-to-key mappings that survive between runs belong to the
calling program. The one intentionally process-external behavior is dependency
maintenance: after an ordinary operation completes, an hourly best-effort
background check may advance `briefcase-client` in the consuming `Cargo.lock`.
This changes the next build, not the running program.

```toml
[dependencies]
briefcase-client = "0.2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

See the [documentation index](../README.md) and [paired testing-environment guide](../testing-environments.md). Production Briefcase's canonical IAM ID is `tos>briefcase`; example member paths elsewhere in this guide must be replaced with real IAM public IDs.

This guide targets client 0.2 and API contract 0.5. Read the
[0.2 migration guide](../migration-0.2.md) when upgrading from 0.1.

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
secret. Obtain an IAM short-lived token (SLT) minted for Briefcase's canonical
`{org_id}>{handle}` Application ID and give only that one-use value to the
Briefcase backend. The normal login is unscoped; omit the organization in IAM
to receive all reachable organizations in `SessionTokens::organizations`:

```bash
iam --no-org login --app-id 'tos>briefcase'
```

```rust
use briefcase_client::{Client, Config};

let anonymous = Client::connect(
    Config::for_sign_in("https://backend.briefcase.teamofsilicons.com/api/v1/")?
).await?;
let tokens = anonymous.login_with_slt(&slt).await?;

let signed_in = Client::connect(
    Config::for_sign_in("https://backend.briefcase.teamofsilicons.com/api/v1/")?
        .with_organization("tos")?
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
The package rejects a scoped exchange or refresh response whose `org_id` is
missing or differs from the organization in `Config`, before returning either
token to the caller. An unscoped configuration accepts `org_id: null` and
returns the live organization list; Briefcase still reevaluates membership on
every organization-scoped request.
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

IAM pairing updates may rotate credentials for the existing IAM environment.
After the sandbox has an IAM organization projection, switching to a different
IAM environment returns HTTP409
`testing_environment_iam_rebind_requires_new_environment`; use a new Briefcase
sandbox for that plane. The rejected update leaves existing data and pairing
unchanged.

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

After an ordinary operation completes, the package starts a due crates.io
check in the background with a short timeout. Checks run at most hourly;
clones and separately constructed clients share the throttle for the same
canonical Cargo manifest within a process. An attempt is recorded before work
starts, so failures do not trigger a check after every request and concurrent
calls cannot overlap an update.

Streaming reads defer maintenance until EOF, a stream error or drop; file
downloads wait through the final flush. Contract negotiation, IAM SLT/refresh
exchanges and every delegated proof/capability operation never trigger it.
There is no timer that runs without completed ordinary operations, and a
short-lived process may exit before background maintenance completes.

If a newer stable `briefcase-client` exists and a consuming `Cargo.toml` can be
found, it runs an exact `cargo update -p briefcase-client --precise <version>`
off the asynchronous request executor. The current process keeps its compiled
version and the next build uses the updated lockfile. Network/Cargo failures
never change the API result and are visible through `client.update_status()`;
while a check runs, that method returns the preceding result.

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

// Creates /notes at the organization base with a Private access boundary.
// Use NewFolder::in_folder for an explicit destination inside a container.
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

Use `restore_from_bin_with_key(entry_id, &key)` to recover an uncertain Bin
restore. Persist the key before sending and reuse it for that same deletion
cycle. A later deletion requires a new key; the backend rejects attempts to
reuse a completed restore key for a different deletion. `restore_from_bin`
generates a fresh key for each call.

The rights are independent. `write` adds content that is not there yet;
`update` changes content that is; neither implies `delete`. Granting a member
who already holds a grant amends it in place, so widening access never has to
pass through a revocation.

## Reading the answers

When a byte-range read returns `416`, `ApiError::unsatisfied_range_length`
contains the file length reported by `Content-Range: bytes */N`, if the server
provided it. Use this response metadata rather than an earlier file-size lookup
when recovering a download after a concurrent update.

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

Applications use fresh IAM proofs for the represented member, never a
Browser-bound or other application-bound bearer on the ordinary Briefcase API.
The API, SDK and CLI surfaces are listed in the
[operation map](../api/operations.md). No delegated SDK call automatically
retries, sends the configured bearer, stores a session or runs maintenance.

### Exact-JSON operations

Prepare a typed manifest before asking IAM to issue its single-use proof:

```rust
use briefcase_client::{ApplicationId, DelegatedCreateFolder, OboProof};

let manifest = DelegatedCreateFolder {
    operation_id, // retain this non-nil UUID with the unchanged logical request
    parent_path: String::new(), // the represented member's private app folder
    name: "recordings".into(),
}.prepare()?;

// Obtain fresh_proof from IAM with the caller application's own credentials
// and current initiating member authority. Bind manifest.endpoint_id(),
// method(), path(), body_sha256() and empty metadata {}. The SDK sends
// manifest.body_bytes() unchanged, not a second serialization.
let folder = client.create_folder_on_behalf_of(
    &ApplicationId::new("tos>browser")?,
    OboProof::new(fresh_proof)?,
    &manifest,
).await?;
```

`DelegatedListEntries` binds the parent, filter, cursor and limit;
`DelegatedReadFile` binds the file UUID, optional range and download flag;
`DelegatedTrashEntry` binds the entry UUID and logical operation UUID. Each
has the same `prepare()` interface. File reads return `ContentStream`. Each
new listing page or different read range needs a newly prepared manifest and
proof. Trash requires both the represented member's delete permission and the
originating application's authority over the content it created.

After an uncertain create/trash response, preserve the exact manifest and
operation UUID but obtain a fresh proof before retrying. Current permissions
are checked before logical replay. `OboProof` is redacted, non-cloneable and
non-serializable, and is consumed by the call.

### Staged uploads and recovery

For a long or recoverable upload, use the [staged-upload protocol](../api/delegated-uploads.md):

1. `DelegatedReserveUpload::file(operation_id, parent_path, local_path).await?`
   hashes a regular file with bounded memory. Retain that manifest and keep the
   source unchanged. Call `prepare()` before minting its reserve proof.
2. `reserve_delegated_upload(&app_id, proof, &manifest)` returns the durable
   status and, only while idle/reserved, a narrow `UploadCapability`.
3. `transfer_delegated_upload(upload_id, capability, &source)` sends
   `UploadSource::File` or `UploadSource::Bytes` using only the capability and
   the configured organization/plane. The server verifies the complete size
   and digest; this does not publish the file.
4. Prepare `DelegatedCommitUpload { operation_id, upload_id }`, obtain another
   fresh proof, and call `commit_delegated_upload`. It publishes the existing
   staged object after current identity, destination permissions and quota
   pass. Successful logical retries do not create another version.

Use `DelegatedUploadQuery` with `delegated_upload_status` after an uncertain
result, and `DelegatedCancelUpload` with `cancel_delegated_upload` to abandon
an unpublished operation. Each control request requires its own fresh proof.
Status/cancel remain available to the same immutable member/application/plane
after destination write access is lost, but never disclose a capability or
file contents. A fresh reserve can rotate an idle capability, not extend its
original deadline. Capabilities are secret, non-cloneable, non-serializable
and consumed by transfer; persist one only in caller-owned secure storage if
needed. Do not save an IAM proof or authorization snapshot as an outbox grant.

### One-shot uploads

The existing small, immediate raw-body operation remains available:

```rust
use briefcase_client::OnBehalfOfUpload;

let entry = client
    .create_file_on_behalf_of(&OnBehalfOfUpload::file("tos>app-notes", proof, "./generated.md"))
    .await?;
```

For this one-shot operation, the destination, name, and media type travel inside the proof rather than in
the request, so an application cannot redirect a proof it legitimately
obtained. The client never sends its own bearer token here, because presenting
both credentials at once is a request error. A refused proof must never be
retried: IAM consumes it exactly once. Body staging must finish while the
proof and its parent authorization are still valid. After an uncertain
one-shot result, a new proof is not an idempotent retry; use the staged protocol
when durable recovery is needed.

## Organisation-owned storage

Pass a `BucketConfiguration` to `configure_storage` to validate and activate an
organisation-owned S3 bucket. This is an owner/authorised-administrator operation.
The configuration contains the bucket, region, assumed-role ARN, prefix, AWS
account ID, encryption mode, and (for SSE-KMS) KMS key ARN. It never contains AWS
access keys or IAM application secrets.

For recoverable configuration changes, use
`configure_storage_with_key(&configuration, &operation_key)`. Retain the
`IdempotencyKey` with the exact configuration before sending, then reuse both
after a lost response. The SDK's ordinary `configure_storage` method generates a
fresh key for that call.

Inspect `BucketConfigurationStatus.status`: `Configured` means the probe and
activation completed; `Failed` means the previous configuration remains selected.
A completed failed probe is replayed by its key. After fixing the external bucket
or role, use a new key to run validation again. Existing file versions retain their
recorded storage location; subsequent versions use the activated configuration.

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
| Bin | `bin`, `restore_from_bin`, `restore_from_bin_with_key` |
| Consumption | `usage` |
| Organization storage | `configure_storage`, `configure_storage_with_key` |
| Delegated JSON | `create_folder_on_behalf_of`, `list_entries_on_behalf_of`, `read_file_on_behalf_of`, `trash_entry_on_behalf_of` |
| Delegated uploads | `reserve_delegated_upload`, `transfer_delegated_upload`, `commit_delegated_upload`, `delegated_upload_status`, `cancel_delegated_upload` |
| One-shot delegated upload | `create_file_on_behalf_of` |
| IAM SLT session | `login_with_slt`, `refresh_session` |
| Testing environments | `testing_environments`, `create_testing_environment`, lifecycle/key/self methods |
