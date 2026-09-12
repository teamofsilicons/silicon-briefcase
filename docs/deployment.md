# Deploying Silicon Briefcase

Run shell commands from the backend repository root unless stated otherwise.
This is the operator guide; start with the [documentation index](README.md)
for API, package, and CLI usage.

## Current production: base tier

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
  "BRIEFCASE_IAM_APP_ID": "tos>briefcase",
  "BRIEFCASE_IAM_APP_SECRET": "<existing-IAM-application-secret>",
  "BRIEFCASE_IAM_WEBHOOK_SIGNING_SECRET": "<configured-webhook-secret>",
  "BRIEFCASE_IAM_WEBHOOK_KEY_VERSION": 1,
  "BRIEFCASE_POSTMARK_SERVER_TOKEN": "<postmark-server-token>",
  "BRIEFCASE_TEST_ENVIRONMENT_ENCRYPTION_KEY": "<base64-encoded-random-32-bytes>",
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
passwords, encryption key, and unrelated fields. Updating Secrets Manager does
not itself reload running processes: deploy/restart in a planned sequence.

The canonical Application ID is `tos>briefcase`, not its internal UUID. The
Application secret comes from IAM; the webhook signing secret is the shared
value supplied to IAM and configured here. Both are server-side secrets, not
CLI login inputs. During webhook rotation, retain prior key versions using
`BRIEFCASE_IAM_WEBHOOK_PREVIOUS_KEYS_JSON`, a JSON-object string such as
`{"1":"<prior-secret>"}`. Never log that configuration or persist a raw signed
test wrapper.

Runtime Docker env files are not shell scripts: do not `source` them. An
unquoted canonical Application ID contains `>`, which a shell interprets as
redirection. Inspect only specific non-secret configuration fields when needed.

The generated testing-environment encryption key protects retrievable IAM and
Briefcase sandbox credentials at rest. Keep it stable across deployments and
rotate it only with a data migration. First boot creates and migrates a
separate `briefcase_test` database on a dedicated testing RDS instance, matching
IAM's topology. Every sandbox shares that test database but is namespaced by
its environment UUID, while control rows remain in production `briefcase`.

**3. Register the webhook and endpoints with IAM.** Follow the
[separate IAM approval/OBO runbook](iam-integration.md). Production
webhook approval uses IAM's dedicated `app approve-webhook` operation. It
requires a direct Carbon session as the owning organization's owner/admin or
an IAM `applications.review` reviewer, plus verified-channel step-up for
`application.webhook.approve`. An Application secret cannot approve the pending
destination. Do not expose this operator workflow in the Briefcase CLI/client.

Briefcase receives the IAM
Application current-state events needed for organizations, memberships, roles,
and tags at `POST https://backend.briefcase.teamofsilicons.com/webhook/`.
Register every delegated control endpoint listed in the [IAM guide](iam-integration.md)
on the appropriate production or test Application. The one-shot endpoint
`briefcase.files.create` uses `/api/v1/obo/files` and metadata keys `path`,
`name`, and `content_type`. The JSON folder/list/read/trash and upload-control
endpoints use their own fixed paths and empty metadata schemas. The
[capability-only byte-transfer route](api/delegated-uploads.md) is not an IAM
endpoint registration; its ingress must allow the documented PUT request body.

Verify that the Briefcase Application discloses `profile`, `self.organizations.read`,
`self.identity.read`, and `self.membership.read`; without those scopes IAM correctly omits
fields that Briefcase needs to cross-bind an introspected bearer. The production
webhook URL accepts both production events and IAM's signed wrapped test
events. Test deliveries are routed by a constant-time root-key match into the
separate shared test database; neither the testing key nor the outer envelope
is persisted.

Deploy IAM's current authorization contract (backend migration 0067 and test
migration 9003) before this Briefcase version. Briefcase uses the official
`silicon-iam-client` 1.4.0, with dependency auto-updates disabled. Complete
online snapshots populate immutable membership bindings on first use; webhook
delivery is no longer a prerequisite for first login or post-clean bootstrap.
Keep webhooks enabled for other members, resource lifecycle and directory
reconciliation. Snapshot conflicts fail closed rather than replacing newer
signed versions. The SDK uses the complete request timeout; the old separate
`BRIEFCASE_IAM_CONNECT_TIMEOUT_MS` setting is no longer used.

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

Deploy backend **1.0.0** with official client, CLI and browser gateway **1.0.1**.
Development 0.x consumers are unsupported. Apply all forward migrations to
both configured databases before starting the new binaries. Keep the role grants
for the API and worker separate. The first release introduces app namespace
roots, immutable version hashes, public links, dynamic tag grants, verified
contacts, invitation outbox delivery, year-long logs and contract lifecycle state.

Set `BRIEFCASE_POSTMARK_SERVER_TOKEN` on the worker and verify
`briefcase@teamofsilicons.com` in Postmark. The public site base must be
`https://briefcase.teamofsilicons.com/` so invitation links point to the browser.
Monitor invitation outbox retries/dead letters; missing IAM-verified contacts
are explicit delivery failures. Testing email never leaves the test plane.

Configure IAM 1.7 scopes and the two critical sharing endpoints from the
[IAM guide](iam-integration.md). Test credentials now select the paired plane
using `X-Briefcase-App-Secret`. Replace a rotated IAM pairing atomically.

Build the documentation with `npm ci && npm run build && npm run check` in
`docs-site/`. Upload only `docs-site/dist/` to the static host for
**docs.briefcase.teamofsilicons.com** and configure TLS, directory indexes and
`404.html`. No wildcard application proxy or API credentials are required.
Canonical URLs, navigation, search, sitemap and robots are generated for that
hostname. See [the site build instructions](../docs-site/README.md).

This change prepares release artifacts and documentation. Publishing packages,
changing DNS and deploying services/docs are separate operator actions.
