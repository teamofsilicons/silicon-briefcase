# Briefcase browser app

The browser app uses Silicon IAM for sign-in and the official Briefcase Rust
client for file operations through the `briefcase-web` gateway.

See the [browser user guide](../docs/browser.md) for file management, links,
sharing, and notifications.

## Sign-in

1. Select **Continue with IAM**. No organisation ID is required.
2. Briefcase redirects to `https://auth.iam.teamofsilicons.com/login`, passing
   `app_id=tos>briefcase` and a server-generated `redirect_uri`. An existing
   organisation file link supplies `org_id` automatically; ordinary sign-in
   remains unscoped.
   Production uses `https://briefcase.teamofsilicons.com/auth/callback`;
   local development uses the same path on the configured local origin.
   The callback URL includes a random, short-lived `state` query value matched
   against the browser's HttpOnly login cookie and pending server-side flow.
3. IAM authenticates you and returns a short-lived token to the callback.
4. The gateway validates the browser-bound callback, exchanges the token through
   Briefcase, and redirects to the file manager with the token removed from the URL.

When sign-in starts from a file link, the gateway retains its validated same-origin
`/org/` return path in the pending flow. Absolute URLs, other origins, query strings,
and fragments are not accepted as return destinations.

IAM credentials are entered only on IAM. Access and refresh tokens stay on the
gateway; the browser receives an HttpOnly session cookie, not those tokens.
The callback is generated from the configured website origin, never a request
header. IAM does not require callback registration. See the
[official IAM login guide](https://github.com/teamofsilicons/silicon-iam/blob/main/docs/client/login.html).

## Local development

From this directory:

```sh
npm ci
npm run dev -- --host 127.0.0.1 --port 4317
```

In another terminal, from `clients/rust`:

```sh
BRIEFCASE_WEB_ORIGIN=http://localhost:4317 cargo run -p briefcase-web --locked
```

Open `http://localhost:4317`. The development server proxies `/browser` and `/auth/callback` requests
to the gateway at `127.0.0.1:4318`. Set `BRIEFCASE_API_URL` on the gateway to use
a different Briefcase API, for example `http://127.0.0.1:18180/api/v1/`.
Do not put application secrets or IAM tokens in frontend environment variables.

## Gateway configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `BRIEFCASE_WEB_ORIGIN` | `http://localhost:4317` | Exact browser origin used for callbacks and request validation |
| `BRIEFCASE_WEB_LISTEN` | `127.0.0.1:4318` | Gateway listen address |
| `BRIEFCASE_API_URL` | `https://backend.briefcase.teamofsilicons.com/api/v1/` | Briefcase API |
| `BRIEFCASE_WEB_ASSETS` | `../../web/dist/client` | Built assets, relative to the gateway working directory |
| `BRIEFCASE_WEB_UPLOAD_DIRECTORY` | OS temporary directory | Existing parent directory for a private, process-specific staging directory |
| `BRIEFCASE_WEB_UPLOAD_BUDGET_BYTES` | `21990232555520` (20 TiB) | Aggregate staging reservation ceiling across the gateway's four upload slots |
| `BRIEFCASE_WEB_UPLOAD_FREE_RESERVE_BYTES` | `1073741824` (1 GiB) | Filesystem free space to leave outside upload reservations |
| `BRIEFCASE_WEB_UPLOAD_DEADLINE_SECONDS` | `86400` (24 hours) | Maximum browser-to-gateway staging duration; values must be positive |

The staging budget is an admission ceiling, not allocated disk space or an
organisation quota. Known-length uploads reserve their size before reading the
body; unknown-length uploads reserve each chunk before writing. Both check
filesystem availability and outstanding unwritten reservations. Reservations
remain charged until forwarding finishes and the temporary file is removed.
An incoming body idle for 60 seconds times out. The maximum file size remains
5 TiB; provision staging capacity and configure deadlines for the intended load.

Accounting is local to one gateway process. Give each replica a dedicated
staging volume or a filesystem quota; do not rely on this counter to coordinate
multiple processes or unrelated disk writers. Private staging files are removed
on normal completion, cancellation, and shutdown. A process crash can leave its
private staging directory behind; handle such orphan cleanup in deployment
operations after confirming the owning process is no longer running.

Run `npm run build` to produce `dist/client`, then build the gateway with
`cargo build -p briefcase-web --release --locked` from `clients/rust`.
For a standalone local preview of the built assets, run `npm start` from `web`
and open `http://localhost:4318`. This builds and runs the Rust gateway; a Rust
toolchain is required. It defaults the website origin to that same local address
unless `BRIEFCASE_WEB_ORIGIN` is already configured.
Serve the assets and `/browser` routes on the same browser origin. For HTTPS
deployment, build the assets before starting the gateway and configure the
canonical HTTPS origin. HTTP origins are accepted only for loopback development.

Browser sessions last at most eight hours and are held in gateway memory.
Restarting the gateway signs users out. HTTPS cookies use `Secure`, `HttpOnly`,
and the `__Host-` prefix. Pending sign-ins expire after ten minutes.
Exclude callback query strings and authentication headers from proxy access logs.

## Viewing test files

Production environment managers can select **View as testing environment** and
enter a short-lived token for the paired test IAM application. The gateway
authorizes environment access and root-key retrieval, then exchanges the token
through the SDK using that environment. Root keys and session tokens are never
returned by the view endpoint.

The production cookie identifies a parent session. Test identities are child
sessions bound to that parent, selected per tab by a public UUID in sessionStorage.
API, preview, download, and upload URLs carry `test_environment=<uuid>`; this is
a selector, never a credential. Missing child sessions, invalid selectors, or an
expired parent fail closed. The production tab retains its original identity.
Test logout removes the child only; **Return to production** clears the tab
selection. Cached reentry checks environment version, root-key generation, and
live test authentication before reuse.

Gateway regression checks:

```sh
cargo test --locked --manifest-path clients/rust/Cargo.toml -p briefcase-web
```

Run from the repository root. Tests cover parent binding, production/test
isolation, expiry, malformed selectors, and independent test logout.
