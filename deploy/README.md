# Deploying Silicon Briefcase

Briefcase runs the way Silicon IAM does: one private EC2 instance in an auto
scaling group behind the shared Team of Silicons load balancer, with RDS for
metadata and S3 for file bytes. The image is immutable and tagged with the
commit it was built from; putting a new build in service means replacing the
instance, not restarting a process on it.

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

**2. Create the application secret.** The instance reads it itself; nothing on
a laptop ever holds it.

```bash
aws secretsmanager create-secret \
  --name silicon-briefcase/production \
  --secret-string "$(jq -n \
    --arg api "$(openssl rand -hex 32)" \
    --arg worker "$(openssl rand -hex 32)" \
    '{
      BRIEFCASE_IAM_APP_ID: "silicon-briefcase",
      BRIEFCASE_IAM_AUDIENCE: "silicon-briefcase",
      BRIEFCASE_IAM_APP_SECRET: "REPLACE_IN_AWS_CONSOLE",
      BRIEFCASE_IAM_WEBHOOK_SIGNING_SECRET: "REPLACE_IN_AWS_CONSOLE",
      BRIEFCASE_API_DATABASE_PASSWORD: $api,
      BRIEFCASE_WORKER_DATABASE_PASSWORD: $worker
    }')"
```

The two `REPLACE_IN_AWS_CONSOLE` values come from IAM when Briefcase is
registered there as an application: the HTTP Basic secret it authenticates
introspection and OBO verification with, and the secret IAM signs webhook
deliveries with. The instance refuses to start while either is still the
placeholder, which is deliberate — a Briefcase that cannot verify a caller
should not accept one.

**3. Register the webhook and endpoint with IAM.** Briefcase receives
membership and tag events at `POST https://backend.briefcase.teamofsilicons.com/webhook/`,
and applications reach it through the endpoint `briefcase.files.create` at the
registered path `/api/v1/obo/files`, with metadata keys `path`, `name`, and
`content_type`.

## Deploying

```bash
./deploy/deploy.sh            # build, push, update the stack, replace the instance
./deploy/deploy.sh plan       # show what a stack update would change, and stop
./deploy/deploy.sh image      # build and push only
./deploy/deploy.sh stack      # update the stack with the current image tag
./deploy/deploy.sh refresh    # replace the running instance
./deploy/deploy.sh status     # what is deployed, and whether it is healthy
```

The image tag is the commit, and a dirty working tree is refused: an image
nobody can rebuild from source does not belong in production. The build targets
`linux/arm64` because the instances are Graviton.

A first deploy takes a while — RDS has to be created before the instance can
migrate against it, and the instance's health check grace period is twenty
minutes to allow for that.

## DNS

The load balancer is shared, so Briefcase needs one CNAME. `deploy.sh` prints
the exact record; `dns.sh` writes it at Namecheap:

```bash
./deploy/dns.sh --value <alb-dns-name>            # show the change, send nothing
./deploy/dns.sh --value <alb-dns-name> --apply    # write it
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
`/silicon-briefcase/production/worker`. The instance has no inbound access at
all; reach it with SSM:

```bash
aws ssm start-session --target <instance-id>
sudo journalctl -u silicon-briefcase-api -f
sudo cat /var/log/cloud-init-output.log     # what the first boot did
```

## Rolling back

Deploy the previous commit: the image for it is still in ECR, so this is a
stack update plus an instance replacement, with no rebuild.

```bash
git checkout <previous-commit>
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

**Organization-owned buckets are not wired up yet.** The instance role can
reach the platform bucket and nothing else, so `PUT /storage/configuration`
will report a failed probe until the role is given `sts:AssumeRole` for the
role an organization registers. That grant is deliberately absent until the
first organization needs it.

**One instance, no redundancy.** The auto scaling group is fixed at one, which
means a deploy has a gap while the replacement boots and a lost instance is an
outage until it is replaced. Growing past that means more than raising the
count: uploads stage on instance-local disk, so a second instance needs its own
staging volume, and the multipart sessions in flight belong to whichever
instance started them.
