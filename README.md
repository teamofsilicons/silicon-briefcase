# Silicon Briefcase

File storage for personal and work accounts, Silicons, and apps acting with Silicon Accounts approval. Each account owns a drive. Accounts, organization and team shares, and custodian authority govern access. Briefcase 5.0 includes [Organizations management and sharing](docs/organizations.md), using Organizations' shared app-session API. The API, website, CLI and Rust client are released; see the [production release record](deploy/RELEASE-organizations.md).

- Rust API and worker: Accounts JWT/JWKS verification, live access-token introspection on every request, signed account events, per-drive row security, S3 streaming, immutable versions, share expiry, a 45-day bin, and delegated uploads.
- Native CLI and Rust client: public device/short-lived-token sign-in, locked token refresh, precise paths and sharing, streaming operations, and Silicon Apps packaging.
- Next.js 16 frontend: the Accounts/Apps Arc UI design system, hosted PKCE sign-in with sealed httpOnly sessions, same-origin streaming proxy, file management, document previews and public links.

For full frontend integrations, start with the [interface API guide](docs/interfaces.md).

Start with [documentation](docs/README.md), [web setup](web/README.md), [CLI](docs/cli/README.md), [client](docs/client/README.md), or [OpenAPI](openapi.yaml). Operator setup is in [deployment](docs/deployment.md). Identity mapping and rollout constraints are in [cutover](docs/migration/cutover.md); verified local work is recorded in [progress](docs/migration/progress.md).

## Local checks

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features -- --test-threads=1
cargo test --locked --manifest-path clients/rust/Cargo.toml --workspace --all-targets --all-features
pnpm --dir web install --frozen-lockfile
pnpm --dir web typecheck
pnpm --dir web lint
pnpm --dir web test
pnpm --dir web build
```

Database tests require `BRIEFCASE_TEST_DATABASE_URL`; S3 tests require `BRIEFCASE_TEST_S3_BUCKET`, `BRIEFCASE_S3_ENDPOINT_URL` and AWS-compatible credentials. A local S3-compatible service proves compatibility with that implementation, not an AWS production deployment. Browser checks require a configured Accounts stack and running Briefcase API/storage; see web/README.md.

Build `briefcase-api`, `briefcase-worker` and `briefcase-migrate` with the root Cargo workspace. Build the CLI from `clients/rust`. Package a native target with `scripts/package-apps.py` and validate it with `silicon-apps`; no package helper publishes automatically. `Dockerfile.browser` builds the standalone Next server. Secrets are supplied at runtime.

Human-owned [UNDERSTANDING.md](UNDERSTANDING.md) remains unchanged. [The migration proposal](docs/migration/understanding-proposal.md) records requirement wording that needs human review.
