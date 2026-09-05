# briefcase-client

The official Rust client for [Silicon Briefcase][service], the
organization-scoped file service used by Carbons, Silicons, and IAM-authorized
applications.

Everything the service exposes to a client, and nothing it does internally.
Application behavior remains stateless: it holds no login session or API
cache — a `Config` goes in and a `Client` comes out. Its default-on maintenance
hook may advance this package in the consuming project's `Cargo.lock` on the
first ordinary request; contract negotiation and short-lived IAM credential
exchanges defer it. Disable that explicitly when a caller owns dependency
updates.

```toml
[dependencies]
briefcase-client = "0.1"
```

```rust
use briefcase_client::{Client, Config, Destination, ListEntries, Upload};

let client = Client::connect(
    Config::new("https://backend.briefcase.teamofsilicons.com/api/v1/", "tos")?
        .with_token(token),
)
.await?;

for entry in client.list_entries(&ListEntries::default()).await?.items {
    println!("{}", entry.path);
}

let stored = client
    .upload(&Upload::file(Destination::path("private/cos:tos/notes"), "./report.pdf")?)
    .await?;
```

`connect` verifies the service identity, selected/supported API major, and the
exact ID/version/method/path of every operation this client calls before the
first real call. Unknown operation IDs are additive; duplicate IDs are
refused. An incompatible pairing therefore fails at startup rather than
mid-request.

`Config::new` accepts exactly the compiled `/api/v1/` base and requires HTTPS,
with clear-text HTTP limited to `localhost` and loopback IPs for local tests.
The version response header and body must select the same API major.

IAM login uses `Client::login_with_slt`; the SLT must be organization-bound,
and the returned session must match the organization in `Config`. The IAM
Application secret stays on the Briefcase backend. Testing environments use a
typed 32-character `EnvironmentKey` in `Config::with_environment`,
independently of the bearer credential. Production-only management methods
create, inspect, re-pair, rotate, clean, retire, and restore those planes. Every
environment mutation has a caller-key `_with_key` variant for safely replaying
an unchanged request after an uncertain result; upload, entry update, and
version restore accept caller-owned keys too. Persist that `IdempotencyKey`
before the first attempt.

Hidden permanent-URL paths can be sent directly to
`Client::request_access_by_path`; the package does not resolve entry metadata
first. The UUID-addressed `request_access` method remains available when the
stable identifier is already known.

Full guide: [docs/package.md][guide]. The `briefcase` command-line client is
built on this package and lives in the same repository.

[service]: https://briefcase.teamofsilicons.com
[guide]: https://github.com/teamofsilicons/briefcase-client-rust/blob/main/docs/package.md
