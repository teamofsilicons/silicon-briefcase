# Deploying Silicon Briefcase

For the current Accounts release, follow [the parallel production plan](migration/parallel-production.md). Existing IAM runtimes and user data remain live; the historical replacement steps below do not authorize identity adoption or service shutdown.

Run shell commands from the backend repository root unless stated otherwise.
This is the operator guide; start with the [documentation index](README.md)
for API, package, and CLI usage.

## Accounts migration candidate

The 4.0 frontend is a Next.js standalone BFF built by `Dockerfile.browser`, listening on port3000 inside the image. Its runtime environment is separate from the Rust API: APP_ID, APP_SECRET, ACCOUNTS_URL, optional ACCOUNTS_API_URL, APP_API_URL (including `/api`), PUBLIC_URL and SESSION_SECRET. See [web setup](../web/README.md). The old Rust gateway and its static assets remain on port 4318 for legacy links; the independent Next.js unit uses 4319.

The nginx candidate forwards `/api/v1/uploads` with request buffering disabled and 15-minute timeouts. Register the exact web `/auth/callback` at Accounts. Configure Accounts signed events on the API, use a fresh Accounts database and the separate `accounts/` object prefix. Follow the [parallel release plan](migration/parallel-production.md). No local migration check alone authorizes production changes.

## Previously deployed base tier

Briefcase runs on one ARM64 EC2 instance with direct Nginx TLS ingress, RDS for
metadata, and S3 for file bytes. API, worker, and browser containers are managed
by systemd. There is no load balancer or autoscaling group in the current
deployment path. Follow the [base-tier runbook](../deploy/base-tier/README.md)
for image updates, both database migrations, streaming upload ingress, private
browser staging, and rollback.

## Historical load-balancer deployment

The procedure below documents the former autoscaling/load-balancer topology.
Do not run its stack or instance-refresh commands against the current base-tier
deployment. Reintroducing that topology is a separate infrastructure decision.

```
Namecheap ──DNS──▶ shared ALB ──host header──▶ target group ──▶ EC2 (private)
                                                                  │
                                                    briefcase-api ├──▶ RDS PostgreSQL
                                                 briefcase-worker ┘    S3 bucket
```

| Path | What it is |
| --- | --- |
| `deploy/deploy.sh` | Build, push, update the stack, replace the instance |
| `deploy/dns.sh` | Point the host at the load balancer, at Namecheap |
| `deploy/aws/production.yaml` | Everything AWS runs, as one CloudFormation stack |
| `deploy/aws/production.env.example` | The shared identifiers a deploy needs |
| `deploy/postgres/001_runtime_roles.sql` | The two runtime roles, for local Compose |

## Before the first deploy

**1. Fill in the configuration.**

```bash
cp deploy/aws/production.env.example deploy/aws/production.env
$EDITOR deploy/aws/production.env
```

It names the VPC, the two private subnets, the shared ALB's security group and
HTTPS listener, an ACM certificate covering `backend.briefcase.teamofsilicons.com`,
and the Secrets Manager secret below. It is gitignored: it names real
infrastructure and may carry the Namecheap key.

**2. Create the application secret.** The instance reads it from Secrets
Manager. Keep any local provisioning copy outside the repository in a private
directory with file permissions `0600`; never put credentials in an image or
CloudFormation parameters.

Prepare a private JSON file using a secure editor/secret manager. Its schema is:

```json
{
  "BRIEFCASE_APP_SECRET": "<briefcase-app-secret-from-silicon-apps>",
  "BRIEFCASE_ACCOUNTS_WEBHOOK_SECRET": "<whsec-secret-returned-when-the-webhook-was-set>",
  "BRIEFCASE_PROOF_ISSUERS": "<scope=app list; see below>",
  "BRIEFCASE_POSTMARK_SERVER_TOKEN": "<postmark-server-token>",
  "BRIEFCASE_TELEMETRY_TABLE_KEY": "<siliconbriefcase-table-key>",
  "BRIEFCASE_API_DATABASE_PASSWORD": "<strong-random-runtime-password>",
  "BRIEFCASE_WORKER_DATABASE_PASSWORD": "<different-strong-random-runtime-password>"
}
```

These are placeholders, not values to deploy. Generate credentials using a
cryptographically secure generator and place the real values only in that
private file. Set `BRIEFCASE_SECRET_FILE` to its absolute path. Verify its
parent is private and the file is mode 0600. Upload the file without expanding
its secret contents into a command-line argument:

```bash
aws --profile silicon-production --region us-east-1 secretsmanager create-secret \
  --name silicon-briefcase/production \
  --secret-string "file://$BRIEFCASE_SECRET_FILE"
```

This is first-time provisioning only. The hosted secret already exists; use
`put-secret-value` for an intentional update, preserving existing database
passwords and unrelated fields. Updating Secrets Manager does not itself reload
running processes: deploy/restart in a planned sequence.

Plain (non-secret) settings: `ACCOUNTS_URL=https://accounts.teamofsilicons.com`
(the public Silicon Accounts origin, which is also the `iss` of every access
token), `BRIEFCASE_APP_ID=briefcase`, and optionally `ACCOUNTS_API_URL` when
Briefcase should reach Silicon Accounts at another origin than the public one.
Plain http is refused except for this machine (`localhost`, `127.0.0.1`, `::1`),
and production requires https.

`BRIEFCASE_APP_SECRET` (`sa_app_…`) is Briefcase's app secret from Silicon
Apps; it authenticates introspection, account lookups and proof verification
and never leaves the server. `BRIEFCASE_ACCOUNTS_WEBHOOK_SECRET` (`whsec_…`) is
returned once when the webhook URL is set. During a rotation, put the old value
in `BRIEFCASE_ACCOUNTS_WEBHOOK_PREVIOUS_SECRET` until deliveries carry the new
signature.

`BRIEFCASE_PROOF_ISSUERS` lists, per scope, the apps that may act for an
account through `/api/v1/obo/*` with a Silicon Accounts User verification
proof: entries `scope=app` or `scope=app1|app2`, separated by commas or spaces.
Unset means no app may (the API logs a warning at start). The production value
is recorded in `docs/migration/decisions.md`. An app acting this way reaches
only its own folder, `apps/{app}/`, in the account's own drive, and what it
creates records the app.

`BRIEFCASE_PROOF_FIRST_PARTY_ISSUERS` (plain, optional) names the apps among
those that act with everything the account can reach: its own files, what was
shared with it and the drives of the Silicons it looks after, still only
through the `/api/v1/obo/*` routes their scopes allow. Production:
`interface`. Unset, every app is confined to its folder.

**Moving organization data at the cutover.** After `briefcase-migrate` applied
migration 0050, and with the API and worker stopped, run
`briefcase-migrate link-identities --file mapping.csv --dry-run`, review the
report, then run it without `--dry-run` (same environment as the migrator). The
mapping and the procedure are in `docs/migration/cutover.md`.

**3. Set Briefcase's sign-in setup and webhook at Silicon Accounts.** With the
app's credentials (never from the Briefcase CLI): the sign-in setup allows the
web callback `https://briefcase.teamofsilicons.com/auth/callback`, the device
flow and public clients (the CLI signs in without a secret), and asks for
`email` as an optional field (invitation mail goes only to accounts that shared
it). The webhook is `PUT /v1/apps/briefcase/webhook` with the URL
`https://backend.briefcase.teamofsilicons.com/webhook/`; store the returned
secret as above.

Briefcase verifies every delivery's `X-Accounts-Signature` over the raw body
(5-minute timestamp tolerance), applies each `event_id` once, and handles
`account.id_changed`, `account.updated`, `silicon.custodian_changed`,
`membership.signed_out`, `membership.access_removed` and `account.deleted`.
Other types are acknowledged and logged. A bad signature answers 401 and a
signed body that is not an event answers 400, so Silicon Accounts retries only
what can succeed.

## Deploying

```bash
./deploy/deploy.sh            # build, push, update the stack, replace the instance
./deploy/deploy.sh plan       # show what a stack update would change, and stop
./deploy/deploy.sh image      # build and push only
./deploy/deploy.sh stack      # update the stack with the current image tag
./deploy/deploy.sh refresh    # replace the running instance
./deploy/deploy.sh status     # what is deployed, and whether it is healthy
```

The normal image tag is the commit, and the script refuses a dirty working
tree by default. Keep image tags immutable and traceable to their source.
Do not reuse a static `-dirty` tag for different source snapshots. The build targets
`linux/arm64` because the instances are Graviton.

A first deploy takes a while — RDS has to be created before the instance can
migrate against it, and the instance's health check grace period is twenty
minutes to allow for that.

## DNS

The load balancer is shared, so Briefcase needs one CNAME. `deploy.sh` prints
the exact record; `dns.sh` writes it at Namecheap:

```bash
./deploy/dns.sh --value "$ALB_DNS_NAME"            # show the change, send nothing
./deploy/dns.sh --value "$ALB_DNS_NAME" --apply    # write it
```

Namecheap's `setHosts` replaces every record on the domain, so the script reads
the zone first, changes exactly one record, prints the whole set it will send,
and refuses outright if the zone comes back empty. Read that list before
passing `--apply`.

The machine running it must have its public address on the API allow list at
**Profile → Tools → Namecheap API Access**; the script prints the address it
used, which is the address to allow.

The ACM certificate for the host is validated by a DNS record too — add that
one the same way before the certificate will issue.

## After a deploy

```bash
./deploy/deploy.sh status
curl https://backend.briefcase.teamofsilicons.com/api/version
```

`/api/version` names every operation's contract revision, which is what a
client checks before its first call. A client built against an older revision
will refuse to connect after a deploy that bumps one — that is the intended
behavior, and the fix is to upgrade the client, not the server.

Logs are in CloudWatch under `/silicon-briefcase/production/api` and
`/silicon-briefcase/production/worker`. The instance has no public IP or SSH ingress; only the ALB security group can
reach its API port. Use SSM for administration:

```bash
aws ssm start-session --target "$INSTANCE_ID"
sudo journalctl -u silicon-briefcase-api -f
sudo cat /var/log/cloud-init-output.log     # what the first boot did
```

## Rolling back

Deploy the previous commit: the image for it is still in ECR, so this is a
stack update plus an instance replacement, with no rebuild.

```bash
git checkout "$PREVIOUS_COMMIT"
./deploy/deploy.sh stack && ./deploy/deploy.sh refresh
```

Migrations only go forward. A rollback across one has to be thought through
rather than run — check what the migration did before assuming an older binary
can read the schema.

## Things worth knowing

**The worker needs `BYPASSRLS`.** Every table forces row-level security, and
the worker is the one process that legitimately works across tenants; it
refuses to start without it, and the API refuses to start *with* it. The first
boot creates both roles accordingly. If RDS declines to grant `BYPASSRLS` to
the master principal, that is the one thing to check before anything else:

```sql
SELECT rolname, rolbypassrls FROM pg_roles WHERE rolname LIKE 'briefcase%';
-- briefcase_api    | f
-- briefcase_worker | t
```

**Uploads stage on local disk.** A whole file arrives in one request and is
written to `/var/lib/silicon-briefcase/staging` before it goes to S3, so the
instance volume — not a tmpfs — has to hold the largest file anyone uploads.
`StagingVolumeSizeGiB` defaults to 100 GiB. The target group's deregistration
delay is fifteen minutes for the same reason: a draining instance is allowed to
finish an upload rather than have it cut off.

**Migrations run on every boot**, as the RDS master principal, before the API
and worker start. They are idempotent, and each one grants the runtime roles
whatever it created. Migration `0012` rewrites `search_documents` to reindex
filenames, which takes an exclusive lock on that table for the duration of the
rewrite — brief while the table is small, worth a maintenance window once it
is not.

**Organization-owned buckets require role access.** Before configuring one,
grant the instance role `sts:AssumeRole` for the exact organization-provided
role and configure its trust policy. `PUT /storage/configuration` activates
the bucket only after its create/read/update/delete probe succeeds.

**One instance, no redundancy.** The auto scaling group is fixed at one, which
means a deploy has a gap while the replacement boots and a lost instance is an
outage until it is replaced. Growing past that means more than raising the
count: uploads stage on instance-local disk, so a second instance needs its own
staging volume, and the multipart sessions in flight belong to whichever
instance started them.

## First official release deployment

Deploy backend **2.0.0** with official client, CLI and browser gateway **2.0.0**.
Development 0.x consumers are unsupported. Apply all forward migrations to
both configured databases before starting the new binaries. Keep the role grants
for the API and worker separate. The first release introduces app namespace
roots, immutable version hashes, public links, dynamic tag grants, verified
contacts, invitation outbox delivery, year-long logs and contract lifecycle state.

Set `BRIEFCASE_POSTMARK_SERVER_TOKEN` on the worker and verify
`briefcase@teamofsilicons.com` in Postmark. The public site base must be
`https://briefcase.teamofsilicons.com/` so invitation links point to the browser.
Monitor invitation outbox retries/dead letters; a recipient who did not share
an email with Briefcase is an explicit delivery failure, not a retry.

Build the documentation with `npm ci && npm run build && npm run check` in
`docs-site/`. Upload only `docs-site/dist/` to the static host for
**docs.briefcase.teamofsilicons.com** and configure TLS, directory indexes and
`404.html`. No wildcard application proxy or API credentials are required.
Canonical URLs, navigation, search, sitemap and robots are generated for that
hostname. See [the site build instructions](../docs-site/README.md).

This change prepares release artifacts and documentation. Publishing packages,
changing DNS and deploying services/docs are separate operator actions.

## Telemetry

Backend and worker use the official Space Station Rust client, pinned in
`Cargo.toml`. Browser analytics and explicit UI events use
`@teamofsilicons/space-station-web`. Everything lands in the private
[`tos/siliconbriefcase` table](https://spacestation.teamofsilicons.com/o/tos/tables/siliconbriefcase).
Bug report content stays in Briefcase PostgreSQL.

Provision `BRIEFCASE_TELEMETRY_TABLE_KEY` in the backend and worker secret
environments. Keep it out of client builds and source control. Local processes
also read `$HOME/.config/briefcase/telemetry.env`; use directory mode 0700 and file
mode 0600. `BRIEFCASE_TELEMETRY_URL` defaults to
`https://backend.spacestation.teamofsilicons.com`. `BRIEFCASE_TELEMETRY_HOME` selects a
persistent private spool directory (default `$HOME/.briefcase-telemetry`).
Each replica should have its own durable volume for this directory. The container
image uses `/var/lib/silicon-briefcase/telemetry`; the AWS template mounts separate
private API and worker spool directories and reads the telemetry key, URL and
optional on/off override from the existing application secret in Secrets Manager. Missing or
invalid configuration disables delivery; `telemetry::configured()` exposes only
whether a collector was initialized. Set `BRIEFCASE_TELEMETRY=off` to disable a
backend, worker, or gateway process. Restart after changing server settings.

HTTP middleware records operation, outcome, duration, and a UUID correlation ID.
Request-scoped tracing inherits opt-out and authenticated sandbox context.
Workers record named job stages and numeric progress. Browser analytics are
sanitized before leaving the browser; the gateway relays batches to
`POST /api/v1/telemetry`, and only the backend holds the table key. Both browser
analytics and custom request events use the same table. Browser preference is
available on sign-in and in Organisation settings → Telemetry, applies to that
browser, and is on by default. CLI and SDK settings are documented in their guides.

Each stored record includes event ID, source, operation, stage, sandbox context,
optional correlation/timing/status/count/progress, schema version, service,
backend build, receipt time, and `client_reported`. Space Station adds its own
host metadata for the collecting backend/worker, not the remote browser or CLI.
Remote source and sandbox labels are claims, never authorization facts. Raw
request paths, headers, error messages, typed text, filenames and file contents
are excluded. Test telemetry shares this operational table with `testing: true`;
it contains no sandbox application data and is independent of sandbox cleanup.

Collection is best effort: the official Rust client's bounded queue and durable
spool retry remote delivery, while client relays have short deadlines. Browser
batches retry within bounded queues; duplicate event IDs are suppressed for 60
seconds by each backend process. A 204 means intake, not Space Station persistence.
When the sink is unavailable, Briefcase operations continue normally. An already
accepted event can still drain after a client opts out; new opted-out requests
are suppressed. In-flight network traffic cannot be recalled.
