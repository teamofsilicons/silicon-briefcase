# Official Rust client

`briefcase-client` **2.0.0** speaks Briefcase API contract 2.0.0. It is the shared implementation used by the CLI and browser gateway. Full reference: [docs.briefcase.teamofsilicons.com](https://docs.briefcase.teamofsilicons.com/).

```toml
[dependencies]
briefcase-client = "2.0.0"
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

For initial sign-in, use `Config::for_sign_in`, `iam_info`, `login_with_slt` and the durable-key equivalents. The backend holds the production IAM Application secret; users present only the IAM SLT/access/refresh credentials. Every IAM 5 token belongs to exactly one account and organization. Configure `with_organization` for that returned `org_id`; changing it cannot retarget the bearer. Keep additional accounts in separate clients and credential stores. The package owns no session or API cache. Applications own persistence and token rotation.

`EnvironmentKey::new(test_app_secret)` plus `Config::with_environment` selects testing and redacts Debug output. Shared lifecycle management uses `honeycomb::manage_environment(&arguments)`, which invokes the official Honeycomb CLI with its own session and retry state. The library stores no Honeycomb credentials. Legacy direct management methods receive `testing_environment_managed_by_honeycomb`; use Honeycomb rather than an IAM root pairing. See [Testing environments](../testing-environments.md).

## Listing and content

`list_entries` and `bin` return `EntryPage`; continue from `next_cursor` until null. `entry_at` resolves a path, `entry` an ID, `create_folder` accepts `NewFolder`, and `update_entry` / `delete_entry` mutate only authorized entries. Traversal entries contain safe navigation fields rather than hidden metadata.

`upload` accepts a local file or bytes, a destination and optional name/content type. The backend chooses single or multipart S3 storage. Reusing a name updates the same file and retains every version. Keep an explicit `IdempotencyKey` for reliable upload retries.

`Upload::self_destructing(minutes)` (1 to 43,200) makes a new file delete itself permanently that long after the upload finishes; it never enters the bin, and deleting it by hand earlier is also permanent. An existing name is refused with `self_destruct_requires_new_file`, and later versions do not change the timer. `Entry::self_destruct_at` reports the deadline. `make_permanent(id)` stops the timer; only the creator and organization admins and owners may call it (others receive a forbidden error), and a file without a running timer returns `not_self_destructing`.

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
    expires_in_minutes: None,
};
let invitation = client.invite(id, &request, &IdempotencyKey::random()).await?;
let link = client.set_link_access(id, true, &IdempotencyKey::random()).await?;
let expiring_link = client.set_expiring_link_access(id, 120, &IdempotencyKey::random()).await?;
let logs = client.logs(id, None).await?;
# Ok(())
# }
```

Keep caller-owned operation keys instead of generating a new one on each retry. `invitations(id, cursor)` lists explicit member and tag grants; `revoke_invitation` removes either.

Set `Invite::expires_in_minutes` (1 to 43,200) for an expiring share: read-only, its own grant, and gone the moment its time passes, leaving any other access the recipient holds. `Invitation::expires_at` reports when it ends. `change_expiring_share(id, grant, ExpiryChange::ExpireIn(minutes) | ExpiryChange::Permanent, key)` restarts its clock or keeps it for good; an ended share is not found, and a permanent grant returns `not_an_expiring_share`. `revoke_invitation` ends one early. `set_expiring_link_access(id, minutes, key)` does the same for anyone-with-the-link access (`LinkAccess::expires_at`); `set_link_access(id, true, key)` makes an expiring link permanent and `false` ends it, while a permanent link returns `link_already_permanent`. No notification is sent when an expiring share ends. `link_access` reports explicit and inherited public visibility. Read is always included, write applies to folders only, and delete cannot be granted. Email recipients resolve only through IAM-verified contacts known to Briefcase; see [Sharing](../sharing.md).

`public_entry`, `public_children`, `public_content`, and `public_download` explicitly omit the configured bearer. They serve only entries whose link access is enabled. Public folder lists are paginated and downloads are streamed. An unavailable link is a not-found error.

`logs(id,cursor)` reads 365 days of audit records in pages. `activity` is the latest 100 events. `notifications` returns the newest 20 plus unread count; `mark_notifications_read` marks the complete inbox. `effective_access` supports bulk capability inspection.

## Delegated applications

Use `delegated::DelegatedManifest` to serialize once and retain the request method, path, endpoint ID and SHA-256 for integrity/idempotency. `OboProof` wraps a reusable IAM OBO access token. Keep the same manifest and logical operation ID when retrying an uncertain mutation.

Available typed manifests cover folder creation, listing, file reads, trash, staged upload reserve/commit/status/cancel, and the critical `DelegatedInvite` / `DelegatedLinkAccess` operations. The last two require user approval in the IAM endpoint catalog. Call `invite_on_behalf_of` and `set_link_access_on_behalf_of` with the prepared manifests; `DelegatedInvite.invitation.expires_in_minutes` and `DelegatedLinkAccess.expires_in_minutes` make them expiring shares, part of the immutable request body. All operations stay inside `apps/<app-id>/` and retain the subject's normal permissions, including for owner subjects.

For large or recoverable transfers, reserve private staging, upload with the returned capability, then commit with a valid OBO access token. A capability cannot publish or download content. See [OBO](../obo.md) and [delegated uploads](../api/delegated-uploads.md).

## Errors and maintenance

Use `Error::is_not_found`, `is_forbidden`, `is_unauthenticated`, `code` and `retry_after` instead of parsing error prose. A contract mismatch is actionable before the first authenticated call. Error diagnostics redact tokens, app secrets, storage credentials and OBO proofs.

The Rust client is a normal project dependency. API calls never query crates.io, run Cargo, or change a consuming project's lockfile. Update dependencies explicitly and rebuild. `Config::with_auto_update` and `Config::with_update_manifest` remain compatibility no-ops, and `update_status()` always returns `Disabled`. Honeycomb manages CLI installation and updates.

Use the [operation map](../api/operations.md), [sandbox example](examples/sandbox.rs), and crate API docs for additional request builders, storage configuration and testing context methods. All surfaces in this release target v1; development 0.x compatibility is not provided.

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

> **Live integration baseline — October 3, 2026:** Briefcase 3.0.0 and IAM 5.0.0 are deployed. Browser popup and expanded saved-workspace interface changes are a separate follow-up; use the API/CLI contracts below now, and check the application before relying on those interface additions.

Use a valid IAM OBO access token for the approved endpoint graph. The historical `OboProof` name remains for source compatibility; it now wraps a reusable token, is cloneable and redacts debug output.

```rust
use briefcase_client::{ApplicationId, DelegatedCreateFolder, OboProof};

let manifest = DelegatedCreateFolder {
    operation_id, // keep this UUID with the unchanged logical request
    parent_path: String::new(),
    name: "recordings".into(),
}.prepare()?;
let token = OboProof::new(obo_access_token_from_iam)?;
let folder = client.create_folder_on_behalf_of(
    &ApplicationId::new("browser")?, token.clone(), &manifest,
).await?;
```

The SDK sends `X-IAM-OBO-Access-Token` and `X-App-ID`, with no configured actor bearer. Prepared manifests retain exact JSON and SHA-256 for integrity/idempotency, not IAM proof issuance. Preserve the manifest and logical operation UUID after an uncertain mutation result. A still-valid token may be reused; the initiating app handles OBO refresh and revocation through IAM. No delegated SDK call automatically retries or stores credentials.

For file upload, prepare `DelegatedReserveUpload`, call `reserve_delegated_upload`, transfer bytes with the returned `UploadCapability`, then prepare `DelegatedCommitUpload` and call `commit_delegated_upload` with a valid OBO token. `DelegatedUploadQuery` and `DelegatedCancelUpload` reconcile or abandon unpublished content. The capability permits byte staging only. Current identity, destination permission and quota are checked at publication.

`create_file_on_behalf_of` is retired and returns a configuration error before transmitting bytes. The raw route returns 410. Use the same resumable protocol for small uploads. See the [OBO migration guide](../obo.md) and [delegated uploads](../api/delegated-uploads.md).

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
| Invitations | `invitations(id, cursor)`, `invite`, `revoke_invitation`, `change_expiring_share`, `effective_access` |
| Anonymous links | `link_access`, `set_link_access`, `set_expiring_link_access`, `public_entry`, `public_children`, `public_content`, `public_download` |
| Self destruct | `Upload::self_destructing`, `make_permanent` |
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
| Testing environments | `honeycomb::manage_environment` for lifecycle; existing app-secret selection and current-context reads for file access |

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

In a paired test environment, the SLT can be an IAM-issued test login code or an existing Carbon ID (e.g. `c:alice`)
or Silicon ID (e.g. `si:worker`). Configure the test app secret and pass that
ID to `login_with_slt`, or use `briefcase --test <environment-id> login <actor-id>`.
IAM issues the test session and determines its current access. Production
continues to require a one-time IAM login code.


## Submit a bug report

Use `Client::submit_report_with_key(&BugReport { message, pr, isi }, &key)`
with a persisted `IdempotencyKey` for retry-safe submission. `pr` and `isi` are
optional. The response contains a durable report `id` and `accepted: true`.
Reports use the client's organization, bearer, and test environment; testing
reports are isolated and removed by cleanup. No Space Station integration is
involved. The convenience `submit_report` creates a new key for each call.


## Control a local daemon

`briefcase_client::daemon::request(runtime_directory, &Request::Status)` reads
live status. `Request::Register { state }` registers an existing private state
directory; `Request::Stop` requests graceful shutdown. These stateless functions
require an explicit runtime directory and never start a process or load tokens.
The CLI uses the same SDK transport. Service installation is available through
`briefcase daemon install` on macOS/Linux.

## Operational telemetry

`Config` defaults to telemetry enabled. Use `.with_telemetry(false)` to disable
backend observation of this client's requests, including version negotiation
and login. The SDK is stateless: callers own preferences and can explicitly
pass `briefcase_client::telemetry::enabled_from_env()` for the conventional
`BRIEFCASE_TELEMETRY=off` override. SDK requests are attributed as `sdk`.

For explicit diagnostics, construct `telemetry::Event::new(Source::Sdk,
"upload", Stage::Completed)` and call `telemetry::submit(api_base, &event)` only
when your preference is enabled. It is a bounded, anonymous, best-effort relay;
ignore delivery failure in the business operation. Include only fixed operation
names and numeric progress or timing. Never include user content or credentials.
No Space Station key belongs in a distributed client.
