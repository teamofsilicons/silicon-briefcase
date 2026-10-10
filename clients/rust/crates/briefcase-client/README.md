# Briefcase Rust client

`briefcase-client` 4.0.0 is the official client for the Briefcase 4.0 contract. It performs API calls and exposes typed requests and responses. The caller owns persistence, session refresh and UI.

```toml
[dependencies]
briefcase-client = { version = "4.0.0", features = ["accounts"] }
```

```rust,no_run
use briefcase_client::{Client, Config, Destination, ListEntries, Upload};
# async fn example() -> briefcase_client::Result<()> {
let client = Client::connect(
    Config::new("https://api.briefcase.teamofsilicons.com/api/v1/")?
        .with_token(std::env::var("BRIEFCASE_TOKEN").unwrap_or_default()),
).await?;
let entries = client.list_entries(&ListEntries::default()).await?;
for entry in entries.items { println!("{}", entry.permanent_url); }
let file = client.upload(&Upload::file(Destination::path("notes"), "report.pdf")?).await?;
println!("{}", file.id);
# Ok(())
# }
```

`Client::connect` checks the service name, API major and all 57 operation identifiers, methods, paths and revisions. `new_unchecked` is available when the caller already validated the pairing. API request execution does not install or update dependencies.

## Sign-in

Briefcase access tokens are issued by Silicon Accounts with audience `briefcase`. Enable the `accounts` feature to sign in as Briefcase's public client. `accounts::SignIn::start_device_sign_in` returns a verification code and URL for a Carbon. `finish_device_sign_in` polls while honoring the interval, pending state, slow-down, expiry and denial. Silicons use `exchange_short_lived_token` with a fresh token from `silicon-accounts login --app briefcase -q`.

The result carries the account, access token and rotating refresh token. Store tokens privately. Refresh single-flight and persist the entire replacement pair before using it. A reused refresh token ends the sign-in; do not blindly retry an uncertain refresh. Public clients use `client_id` without an app secret. Revoke refresh tokens at `/v1/oauth/revoke` when signing out.

## Drives and files

`drives()` lists the caller's drive and the drives of Silicons it looks after. `shared()` lists entries explicitly shared with it. Account uuids are permanent, case-sensitive text; display the account's current `c:` / `si:` id.

`Destination::path` names a path in the caller's drive. `Destination::in_drive("c:ada", "plans")` names another drive the caller can access. `entry_at` resolves a permanent owner address. Requests never claim another identity: a custodian always acts as itself.

The client supports entry listing and search, folder creation, streamed uploads and downloads, byte ranges, rename/move, permissions, invitations, expiring shares, public links, self-destruct cancellation, bin restore, versions, activity, archive listings, notifications, usage and verified S3 configuration. Paginated methods expose the service cursor. Hidden entries return the same not-found response as missing entries.

## Acting from another app

Apps sign accounts in to themselves. To act at Briefcase, issue a Silicon Accounts User verification proof for receiving app `briefcase` and the operation's documented scope. Pass a `Proof` to the typed `delegated` methods. Briefcase validates the proof and the configured issuer allow-list, acts as its account, and records the issuing app.

Scopes include `briefcase.entries.list`, `briefcase.files.read`, `briefcase.files.write`, `briefcase.folders.create`, and the remaining operation catalogue in the API documentation. Each route requires its exact scope. Normal issuers stay inside `apps/<issuing-app>/` of the account's drive. Explicit first-party issuers can reach all entries the represented account can access. An app may trash only entries it created.

Delegated uploads reserve, stream bytes using the upload capability, then commit. The service resolves the drive from the reservation. Keep a stable operation id across safe retries and use the status/cancel operations after uncertainty.

## Errors and telemetry

`Error::code`, `hint`, `status`, `request_id` and `retry_after` retain actionable service diagnostics. `is_not_found` covers missing and inaccessible entries. Contract mismatches name the incompatible operation before file mutations begin. Credentials are redacted from debug output.

Telemetry can be disabled through the client configuration. It contains operational measurements, never file contents or tokens. See the crate's API documentation and `briefcase docs cli` for command examples.
