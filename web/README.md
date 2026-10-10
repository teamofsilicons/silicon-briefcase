# Silicon Briefcase web

Next.js 16 App Router, React 19, TypeScript, TanStack Query and the Arc UI components/styles from Silicon's Accounts/Apps web kit. Node 24+, pnpm 10.33.0. Product configuration is in `lib/app.config.ts`; credentials are server-only.

## Run locally

Copy `.env.example` to `.env.local` and fill the app secret and random session secret. Register `http://127.0.0.1:4100/auth/callback` (or the exact PUBLIC_URL) for app `briefcase` in Accounts. Start the Briefcase API with a migrated PostgreSQL database and S3-compatible storage before running the web app.

```sh
pnpm install --frozen-lockfile
pnpm dev
```

Open http://127.0.0.1:4100. Accounts owns email/phone/provider verification. The local E2E setup uses the Accounts stack's mock email sender; production needs a working delivery provider.

| Setting | Purpose |
| --- | --- |
| `APP_ID` | `briefcase` |
| `APP_SECRET` | `sa_app_…`, held only on the server |
| `ACCOUNTS_URL` | Public Accounts origin and token issuer |
| `ACCOUNTS_API_URL` | Optional private Accounts API base |
| `APP_API_URL` | Briefcase service base **including `/api`**, e.g. `http://127.0.0.1:4101/api` |
| `SESSION_SECRET` | At least 32 random bytes, e.g. `openssl rand -base64 48` |
| `PUBLIC_URL` | Web origin; callback is `{PUBLIC_URL}/auth/callback` |
| `EXTRA_IMG_ORIGINS` | Optional comma-separated profile image origins |
| `EXTRA_ORIGINS` | Optional additional origins permitted to send state-changing requests |

Settings are read and validated at runtime. Non-loopback public production origins require HTTPS. Never use a `NEXT_PUBLIC_` variable for credentials. Keep `.env.local` outside Git and Docker build contexts.

## Sessions and files

Hosted PKCE sign-in exchanges the code on the server. Tokens live in an authenticated, encrypted httpOnly cookie; secure deployments use a `__Host-` prefix. Refresh is single-flight per process, with a short memo for concurrent old-cookie requests. Deployment currently assumes one web process per session cohort; multi-instance deployments need shared refresh coordination before scaling. Sign-out revokes at Accounts, clears the cookie, and tells other tabs.

`/api/*` checks same-origin mutations and forwards only allowlisted headers to APP_API_URL with the sealed session's bearer. Bodies and responses stream; uploads keep an idempotency key across retries. The proxy timeout is 15 minutes. Public `/public-api/*` reaches only the anonymous file route and forwards no credentials. Range and Content-Disposition survive both routes. Uploaded document responses have a restrictive sandbox CSP.

File pages cover own/custodial drives, explicit shares, folder navigation, search, filters, list/grid views, uploads, folder creation, download, sharing, public links, activity, versions, rename/move, self-destruction, bin restore, notifications, usage and S3 setup. Preview readers are bounded: 256 KiB text, 10 MiB Office input and 12 MiB expanded XML. Office rendering is a text/table preview; originals preserve the complete formatting and media.

The frontend uses no file-content indexing service; search and authorization belong to Briefcase. Browser previews do not execute uploaded scripts or external document relationships.

## Verify

```sh
pnpm typecheck
pnpm lint
pnpm test
pnpm build
pnpm test:e2e
pnpm screens
```

Browser tests require real local Accounts (public9590/API9589/mock sender9592), Briefcase API4101 and storage. They start Next on4100 in `.next-e2e`, create fresh identities, and exercise uploaded bytes, sharing, public reads, session refresh/revocation, CSRF and accessibility. There is no stub file API. E2E state under `test-results/.auth` contains local secrets and is ignored. Screenshots go to `screens/` for visual review in four light/dark desktop/phone combinations.

`next build` produces `.next/standalone` by default. `Dockerfile.browser` copies that server, public files and static assets, runs unprivileged, and accepts runtime settings. For direct self-hosting, copy public and `.next/static` beside the standalone server before starting it. The nginx candidate routes streaming uploads through `/api/v1/uploads` without request buffering.

This worktree is a migration candidate. Build success and local compatibility checks do not assert a production cutover or real AWS role integration. The migration log records the evidence and remaining release gates.
