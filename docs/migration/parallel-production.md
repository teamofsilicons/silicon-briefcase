# Parallel Accounts production release

The user requested that the apps move to Accounts and Apps while existing IAM
Silicons remain unchanged. This plan supersedes the full-data adoption and
replacement instructions in `cutover.md`. No IAM identity, custody, file owner,
resource key, ciphertext, or old account is copied or rewritten.

| Plane | Legacy IAM | New Accounts |
| --- | --- | --- |
| API | `backend.briefcase.teamofsilicons.com`, port 8080 | `api.briefcase.teamofsilicons.com`, loopback 8180 |
| Database | `briefcase` | `briefcase_accounts` |
| Runtime roles | existing API/worker roles | `briefcase_accounts_api`, `briefcase_accounts_worker` |
| S3 prefix | existing `organizations/` | `accounts/` in the same bucket |
| Secret | `silicon-briefcase/production` | `silicon-briefcase/accounts-production` |
| Website | IAM gateway 4318, retained for old links | Next.js 4319 |

The new API and worker have independent units, env files, staging and telemetry
directories. The new database restricts CONNECT; runtime object grants are copied
inside the new database only, without granting membership in a legacy role.
The API role cannot bypass RLS. The worker keeps its required BYPASSRLS capability
but has object grants only in the new store. The shared host role reads each exact
secret ARN; credentials never appear in repository files or command output.

## Staging and opening

1. Back up both legacy RDS stores, object bytes/metadata, existing env files and
   units privately. Verify a restore locally. Leave all old writers running.
2. Create the fresh database/roles, apply the current 52 migrations, and apply
   runtime grants. Stage a fresh Accounts app webhook secret without enabling its
   URL. Configure the new S3 prefix and exact production app credentials.
3. Stage ARM64 images by digest; start the new API/worker privately. Check database
   readiness. Create the separate API TLS vhost with its gate returning 503.
4. Wait for coordinated Accounts/Apps/Browser UUID completion. Only then open the
   new API and register `https://api.briefcase.teamofsilicons.com/webhook/` with
   Accounts. Verify anonymous requests fail and app auth uses canonical UUIDs.
5. Stage the Next.js image with `deploy/base-tier/install-accounts-web.sh`. After
   the website GO, install `accounts-browser.nginx.conf`, validate `nginx -t`, and
   reload. Test Accounts hosted login and a canonical private link. Test an old
   IAM private link, login, and download through the preserved gateway.
6. Publish the new Apps CLI assets. Old Honeycomb assets and service origins stay
   intact; existing Silicons are not reenrolled or updated by this operation.

The web proxy sends verified legacy org slugs, `/browser/*`, and `/assets/*` to
4318. Canonical UUID `/org/` paths and ordinary pages use 4319. At `/auth/callback`,
`code` selects Accounts, `slt` selects IAM, and a mixture or absence is rejected.
Cookie names are distinct. A new legacy organization requires an explicit proxy
route until a separately authorized adoption; no ambiguous slug becomes an
Accounts identity. This routing does not bypass backend file authorization.

New Accounts drives start empty. Legacy files remain usable through existing IAM
clients and old permanent links; they do not appear in a new Accounts drive.
Rollback gates the new API and restores the prior website config. It does not
change either database or delete new objects. Keep new images and all old
services available until explicit approval for any later retirement.
