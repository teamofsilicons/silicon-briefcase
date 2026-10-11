# Silicon Briefcase Rust client and CLI

The workspace contains `briefcase-client` and `briefcase-cli` 4.1.0 for Silicon Accounts and Silicon Apps.

- [Rust client guide](../../docs/client/README.md)
- [CLI guide](../../docs/cli/README.md)

The default API is `https://api.briefcase.teamofsilicons.com/api/v1/`. The IAM service remains available at its existing `backend.briefcase.teamofsilicons.com` origin for existing clients; its data is separate. Installing this release does not import IAM files or enroll existing Silicons in Accounts.

```sh
silicon-apps install briefcase
briefcase login
briefcase put report.pdf notes
```

For an account already registered with Silicon Accounts:

```sh
silicon-accounts login --app briefcase -q | briefcase login --slt-stdin
```

From this directory, validate with `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --workspace`. Publish the client before the CLI only after the matching Accounts service is verified. Both crates are Apache-2.0.
