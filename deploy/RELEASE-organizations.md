# Organizations integration release

Prepared from the newest fetched Briefcase source, `5469a23` on
`origin/fix/durable-browser-refresh`, including the 4.1.0 Accounts rewrite and
session-recovery fixes. The live version endpoint reported 4.1.0 when inspected.
The deployed git SHA was not exposed. The original checkout was a dirty 2.1.0
checkout and was not modified.

Organizations now provides the shared `/v1/integrations/{app}` API in deployed
source `f004cb441653708aaeff743d063e56f7dae2d36a`, branch
`codex/app-session-integrations`. The existing `/v1/integrations/briefcase/*`
paths, response shapes and action bodies are compatible with this Briefcase
implementation. The earlier Briefcase-only companion commit `9ccd212` is
superseded and should not be deployed separately. No second Organizations
session is needed. This Briefcase branch remains local implementation work;
this document is not a Briefcase production deployment record.

## Deployment order

1. Confirm the target Organizations service supports the shared app-session
   integration. This prerequisite is met in production as of 2026-11-16. Keep
   its app-specific token checks and exact-account membership boundary intact.
2. Back up the Accounts-era Briefcase database. Apply migration 0053 using the
   schema owner. Standard `briefcase_api` and `briefcase_worker` grants are in
   the migration; installations with custom runtime role names must apply the
   equivalent table/function grants. The schema owner must own the new narrow
   discovery functions and their definer read policy.
3. Set `BRIEFCASE_ORGANIZATIONS_URL` on the API. Deploy API and worker together,
   followed by the matching web and CLI/client builds. No new shared secret is
   needed: Organizations verifies the forwarded Briefcase session at Accounts.
4. Install `accounts-browser.nginx.conf`, run `nginx -t`, and reload. It routes
   only to the Accounts web on 4319, rejects IAM `slt` callbacks and returns 410
   for the retired browser/assets interface. Preserve file data and historical
   mapping tools. Legacy IAM runtime services can then be decommissioned after
   confirming no other application depends on them; do not erase their storage.
5. Verify personal/work switching; owner, admin and ordinary-member views;
   admin and team management; shared-folder inheritance; search/download access
   after membership removal; expiry; existing direct shares and public links.
   Use authorized test accounts and files, not production membership changes.

Keep a copy of the prior images and Nginx configuration for rollback. Do not
roll back Organizations to a release without the shared integration while the
new API depends on it.
Migration 0053 does not delete historical rows. Rolling the API back removes
the new controls; retire or suspend new group grants deliberately before doing
so because the database permission view also changed.

## Local verification

Use an isolated PostgreSQL cluster with the roles in
`deploy/postgres/001_runtime_roles.sql`:

```sh
BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:PORT/DATABASE \
  cargo test --locked --lib --tests -- --test-threads=1
cargo test --locked --manifest-path clients/rust/Cargo.toml --workspace
pnpm --dir web typecheck
pnpm --dir web lint
pnpm --dir web test
pnpm --dir web build
pnpm --dir web test:organizations
python3 deploy/base-tier/test-cohosting.py --nginx /path/to/nginx
```

The new browser suite uses an isolated loopback API fixture to check UI
contracts, role controls, recipient selection and responsive layout. It does
not replace real Accounts hosted-login E2E. That existing suite needs
`APP_SECRET` or `TEST_STACK_JSON` and the local Accounts/API/storage stack.

Organizations tests use a separate `ORGANIZATIONS_TEST_DATABASE_URL`. Its bridge
test exercises token audience/account binding, direct membership, team-admin
limits, action idempotency/audit, membership removal and current app exclusion.

### Verified locally, 2026-11-16

- Briefcase Rust suite: 236 reported passing tests, with the database suites
  enabled against isolated PostgreSQL 16. The optional S3 test returns early
  without S3 configuration; this run does not validate hosted object storage.
- Rust CLI/client: 148 passed; two external-service integration tests ignored.
- Organizations: 30 passed against a separate isolated database; one live TLS
  notification test ignored.
- Strict Clippy checks passed for Briefcase, its CLI/client and Organizations.
- Web typecheck, lint, 57 unit tests and production build passed. All four
  organization browser scenarios passed, including desktop/mobile layouts,
  account switching links, role controls and named team/organization recipients.
- All 20 Accounts-only Nginx routing checks passed using a locally built Nginx.
- CLI guide synchronization and whitespace checks passed.

The existing real hosted-login E2E command was attempted but could not start:
this checkout has no `APP_SECRET` or `TEST_STACK_JSON` for a local Accounts
stack. Production sign-in, mail/notification delivery and deployment have not
been exercised by this change.

### Shared Organizations API deployed, 2026-11-16

The [live integration guide](https://organizations.teamofsilicons.com/docs/app-integrations)
and deployed source were checked against Briefcase's gateway. An independent
read confirmed the Briefcase route exists and rejects anonymous requests with
401. No runtime change is required in Briefcase: it already forwards its
Accounts bearer token to the compatible routes and handles errors by HTTP status.

Organizations' [release evidence](https://github.com/teamofsilicons/silicon-organizations/blob/a462193/deploy/verification-app-sessions-2026-11-16.json)
records 16 successful read-only production checks with real Write and Accounts
sessions, audience isolation, healthy services and a verified backup restore.
Those sessions had no organization memberships. Work-account management was
tested locally with PostgreSQL and an Accounts stand-in; production roles and
memberships were not changed. Briefcase's authenticated production workflow
and its own deployment remain unverified.
