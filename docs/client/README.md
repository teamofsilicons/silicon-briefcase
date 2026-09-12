# Official Rust client

`briefcase-client` **1.0.1** speaks Briefcase API contract 1.0.0. It is the shared implementation used by the CLI and browser gateway. Full reference: [docs.briefcase.teamofsilicons.com](https://docs.briefcase.teamofsilicons.com/).

```toml
[dependencies]
briefcase-client = "1.0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
uuid = { version = "1", features = ["v4"] }
```

Install the published crate from crates.io, or use the local crate path when developing this repository.

## Connect and authenticate

```rust,no_run
use briefcase_client::{Client, Config, Destination, ListEntries, Upload};
# async fn example() -> briefcase_client::Result<()> {
let client = Client::connect(
    Config::new("https://backend.briefcase.teamofsilicons.com/api/v1/", "tos")?
        .with_token(std::env::var("BRIEFCASE_TOKEN").unwrap()),
).await?;
let page = client.list_entries(&ListEntries::default().limit(100)).await?;
let file = client.upload(&Upload::file(Destination::path("private/me:tos"), "report.pdf")?).await?;
client.download(file.id).await?.write_to_file("report-copy.pdf").await?;
# Ok(())
# }
```

`connect` negotiates before sending credentials. `new_unchecked` is available when the caller has already verified the deployment. Configuration accepts only the canonical `/api/v1/` path, HTTPS except on loopback, and no embedded credentials/query/fragment. Redirects are refused so credentials and mutation bodies cannot move to another origin.

For initial sign-in, use `Config::for_sign_in`, `iam_info`, `login_with_slt` and the durable-key equivalents. The backend holds the production IAM Application secret; users present only the IAM SLT/access/refresh credentials. Login may be unscoped; select one of the returned authorized organizations with `with_organization`. The package owns no session or API cache. Applications own persistence and token rotation.

`EnvironmentKey::new(test_app_secret)` plus `Config::with_environment` selects testing. `IamEnvironmentKey` is the distinct 32-character IAM root credential used for dependency provisioning/pairing only. Both redact Debug output. See [Testing environments](../testing-environments.md).

## Listing and content

`list_entries` and `bin` return `EntryPage`; continue from `next_cursor` until null. `entry_at` resolves a path, `entry` an ID, `create_folder` accepts `NewFolder`, and `update_entry` / `delete_entry` mutate only authorized entries. Traversal entries contain safe navigation fields rather than hidden metadata.

`upload` accepts a local file or bytes, a destination and optional name/content type. The backend chooses single or multipart S3 storage. Reusing a name updates the same file and retains every version. Keep an explicit `IdempotencyKey` for reliable upload retries.

`read_content` accepts an optional `ByteRange`; `download` also accepts folders and streams tar.zst. Consume `ContentStream::chunk` or `write_to_file` to keep memory bounded. `bytes` deliberately collects the entire response and is appropriate only for known small payloads. Treat any streaming failure as an incomplete download.

`versions` returns the first `FileVersionPage`; use `versions_page(id, cursor)` for older versions. Each version includes SHA-256 and source. `restore_version_with_key` creates a new version with the selected content, rather than rewinding or erasing history.

## Invitations and public links

```rust,no_run
use briefcase_client::{AccessRight, Client, IdempotencyKey, Invite, Recipient};
# async fn example(client: &Client, id: uuid::Uuid) -> briefcase_client::Result<()> {
let request = Invite {
    principal: Recipient::Tag("engineering".into()),
    access: vec![AccessRight::Read, AccessRight::Update],
    inherit: true,
};
let invitation = client.invite(id, &request, &IdempotencyKey::random()).await?;
let link = client.set_link_access(id, true, &IdempotencyKey::random()).await?;
let logs = client.logs(id, None).await?;
# Ok(())
# }
```

Keep caller-owned operation keys instead of generating a new one on each retry. `invitations(id, cursor)` lists explicit member and tag grants; `revoke_invitation` removes either. `link_access` reports explicit and inherited public visibility. Read is always included, write applies to folders only, and delete cannot be granted. Email recipients resolve only through IAM-verified contacts known to Briefcase; see [Sharing](../sharing.md).

`public_entry`, `public_children`, `public_content`, and `public_download` explicitly omit the configured bearer. They serve only entries whose link access is enabled. Public folder lists are paginated and downloads are streamed. An unavailable link is a not-found error.

`logs(id,cursor)` reads 365 days of audit records in pages. `activity` is the latest 100 events. `notifications` returns the newest 20 plus unread count; `mark_notifications_read` marks the complete inbox. `effective_access` supports bulk capability inspection.

## Delegated applications

Use `delegated::DelegatedManifest` to serialize once and obtain the exact method, path, endpoint ID and SHA-256 to bind into an IAM proof. `OboProof` is consumed once. Never retry a proof; acquire a new proof for the same immutable manifest and logical operation ID.

Available typed manifests cover folder creation, listing, file reads, trash, staged upload reserve/commit/status/cancel, and the critical `DelegatedInvite` / `DelegatedLinkAccess` operations. The last two require user approval in the IAM endpoint catalog. Call `invite_on_behalf_of` and `set_link_access_on_behalf_of` with the prepared manifests. All operations stay inside `apps/<app-id>/` and retain the subject's normal permissions, including for owner subjects.

For large or recoverable transfers, reserve private staging, upload with the returned capability, then commit with a fresh proof. A capability cannot publish or download content. See [OBO](../obo.md) and [delegated uploads](../api/delegated-uploads.md).

## Errors and maintenance

Use `Error::is_not_found`, `is_forbidden`, `is_unauthenticated`, `code` and `retry_after` instead of parsing error prose. A contract mismatch is actionable before the first authenticated call. Error diagnostics redact tokens, app secrets, storage credentials and OBO proofs.

The default-on maintenance helper checks crates.io at most hourly and can update a consuming Cargo lockfile. `Config::with_auto_update(false)` or `BRIEFCASE_CLIENT_AUTO_UPDATE=false` disables it. Production services should own dependency upgrades through reviewed builds. Delegated operations and unfinished transfers do not trigger package maintenance before authorization or during streaming.

Use the [operation map](../api/operations.md), [sandbox example](examples/sandbox.rs), and crate API docs for additional request builders, storage configuration and environment lifecycle methods. All surfaces in this release target v1; development 0.x compatibility is not provided.

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

## Delegated request examples

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
proof. Trash requires both the represented member's delete permission and confinement to the calling application's namespace.

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
| Versions | `versions`, `versions_page`, `restore_version_with_key` |
| Invitations | `invitations(id, cursor)`, `invite`, `revoke_invitation`, `effective_access` |
| Anonymous links | `link_access`, `set_link_access`, `public_entry`, `public_children`, `public_content`, `public_download` |
| Inbox | `notifications`, `mark_notifications_read` |
| History | `activity`, `logs` |
| Search | `search` |
| Bin | `bin`, `restore_from_bin`, `restore_from_bin_with_key` |
| Consumption | `usage` |
| Organization storage | `configure_storage`, `configure_storage_with_key` |
| Delegated JSON | `create_folder_on_behalf_of`, `list_entries_on_behalf_of`, `read_file_on_behalf_of`, `trash_entry_on_behalf_of` |
| Delegated uploads | `reserve_delegated_upload`, `transfer_delegated_upload`, `commit_delegated_upload`, `delegated_upload_status`, `cancel_delegated_upload` |
| One-shot delegated upload | `create_file_on_behalf_of` |
| IAM SLT session | `login_with_slt`, `refresh_session` |
| Testing environments | `testing_environments`, `create_testing_environment`, lifecycle/key/self methods |

## Public IAM information and live login status

The stateless client exposes the same discovery and authentication inspection as
the CLI. Construct an unscoped configuration so no organization selection is
needed:

```rust,no_run
# async fn inspect() -> Result<(), briefcase_client::Error> {
use briefcase_client::{Client, Config};
let config = Config::for_sign_in("https://backend.briefcase.teamofsilicons.com/api/v1/")?
    .with_auto_update(false);
let client = Client::connect(config.clone()).await?;
let iam = client.iam_info().await?;
println!("Request an IAM SLT for {}", iam.app_id);
let signed_in = Client::connect(config.with_token("caller-owned-access-token")).await?;
let status = signed_in.login_status().await?;
if let Some(actor) = status.actor {
    println!("{} {}", actor.actor_type.as_str(), actor.principal_id);
}
# Ok(())
# }
```

`iam_info()` always omits the member bearer and returns only the public app ID
and optional Briefcase/IAM test-environment IDs. `login_status()` checks the
current token online; inactive tokens return `authenticated: false`. Active
sessions include `actor` (principal UUID, type, nullable public identifier),
current organizations, and access-token expiry as a Unix timestamp. A session
without grants remains authenticated; its public identifier may be absent.
Network, IAM, and test-plane failures remain errors. The caller owns token
refresh and storage; the Rust package does not read `SILICON_HOME` or persist
credentials. The stateful CLI handles these responsibilities.
