# Silicon Briefcase — Rust client and CLI

Two crates, published separately:

| Crate | What it is |
| --- | --- |
| [`briefcase-client`](./crates/briefcase-client) | The Rust package. Stateless, async, and the primary interface to the service. |
| [`briefcase-cli`](./crates/briefcase-cli) | The `briefcase` command, built on that package and stateful where a person needs it to be. |

The package is the interface; the CLI is a shell around it and has no
capability the package lacks. Anything the CLI can do, a program can do with
the same call.

- **[docs/package.md](./docs/package.md)** — using the Rust package
- **[docs/cli.md](./docs/cli.md)** — using the CLI

## Quick look

```rust
use briefcase_client::{Client, Config, Destination, Upload};

let client = Client::connect(
    Config::new("https://backend.briefcase.teamofsilicons.com/api/v1/", "tos")?
        .with_token(token),
)
.await?;

let entry = client
    .upload(&Upload::file(Destination::path("private/cos:tos/notes"), "./report.pdf")?)
    .await?;
println!("{}", entry.permanent_url);
```

```bash
# Ask IAM for an organization-bound SLT for the canonical Application first.
iam --org tos login --app-id 'tos>briefcase'
briefcase login --url https://backend.briefcase.teamofsilicons.com/api/v1/ --org tos
briefcase put ./report.pdf private/cos:tos/notes
briefcase share private/cos:tos/notes/report.pdf carbon:cos:tos --access read
```

The CLI stores the resulting rotating access/refresh session and renews it
before expiry. It never asks for an IAM password, OTP, or Application secret.
Both crates also support disposable IAM-coupled test planes; see the guides for
the bootstrap sequence and `briefcase --test <environment-uuid> <command>`.

Both crates maintain themselves from crates.io by default. The package advances
the consuming Cargo project's lockfile as a best-effort first-ordinary-request
check; the CLI checks at most daily and installs a newer `briefcase-cli` for the
next invocation. Contract negotiation and short-lived IAM credential commands
defer maintenance so those credentials are sent immediately. Both behaviors
have documented config and environment opt-outs.

## The contract check

Both crates carry the complete ID, revision, HTTP method, and path of every
operation they call. Before its first real call the client reads
`GET /api/version`, verifies header/body negotiation agreement, and checks
those operations plus the service identity and negotiated major. Unknown
operation IDs are additive; duplicate IDs or a changed used operation fail at
startup, naming what moved, instead of halfway through a later call.
`--no-verify` on the CLI, or `Client::new_unchecked` in the package, skips it
deliberately.

## Working on these crates

From the repository root, first run `cd clients/rust`. This is a separate
Cargo workspace from the backend.

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
```

The package's wire behavior is covered against a mock server in
`crates/briefcase-client/tests/wire.rs`: what goes on the wire, what the
headers carry, and how a refusal reads.

## Publishing

The crates version together but publish separately, package first:

```bash
cargo publish -p briefcase-client
cargo publish -p briefcase-cli
```

`briefcase-cli` depends on `briefcase-client` by both path and version, so the
published CLI resolves the published package.

## Licence

Both crates are Apache-2.0. See [LICENSE](./LICENSE).

## The service

The API this speaks to lives in
[`silicon-briefcase`](https://github.com/teamofsilicons/silicon-briefcase);
`UNDERSTANDING.md` there is the product contract, `openapi.yaml` the wire
contract, and its `docs/` directory the canonical API/client/CLI integration
guides. The local pages above retain compatibility links to those guides.
