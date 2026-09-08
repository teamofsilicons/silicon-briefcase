# Briefcase AWS hosting — 2026-09-05

Status: hosted API and worker healthy; public HTTPS operational.

## Deployment

- HTTPS host: `https://backend.briefcase.teamofsilicons.com`.
- IAM webhook: `https://backend.briefcase.teamofsilicons.com/webhook/`.
- AWS account/region: `234951665042` / `us-east-1`.
- CloudFormation stack: `silicon-briefcase-production`.
- Dedicated private ARM64 `t4g.small` EC2 `i-0f1af1080a00f4be3`, managed through SSM; no public IP.
- Existing IAM ALB reused through a Briefcase-only HTTPS host rule, priority 30.
- Production PostgreSQL 17.9 RDS: `silicon-briefcase-production`, `db.t4g.small`, database `briefcase`.
- Separate testing PostgreSQL 17.9 RDS: `silicon-briefcase-testing-production`, `db.t4g.micro`, database `briefcase_test`.
- Both databases are private and encrypted. Production backups are retained seven days; testing backups one day. Deletion protection and snapshot retention are enabled.
- Private encrypted S3 bucket: `silicon-briefcase-production-234951665042-us-east-1`.
- Existing DNS and issued ACM certificate reused; IAM's host routing and databases were not changed.

The test database is dedicated; the API and worker serve both production and
isolated testing environments from the same Briefcase EC2, matching IAM's model.
Application-level testing environments are created separately through the API/CLI.

## Credentials

The provided IAM application credentials and webhook secret are stored in AWS
Secrets Manager under `silicon-briefcase/production`. A local backup is outside
the repository, in a mode-0700 directory with mode-0600 files. No credential
values are included in this report or the build context.

IAM's canonical public application ID is `tos>briefcase`; the supplied UUID is
its internal application identifier. The official IAM CLI accepted the supplied
application secret against hosted IAM. Briefcase uses the official registry
`silicon-iam-client` version `1.2.0`.

## Startup correction

Live SQL inspection found the two independently provisioned RDS instances share
the same PostgreSQL system identifier and database OID, consistent with a shared
RDS base image. The original isolation guard therefore rejected valid configuration.
The identity comparison now additionally includes the server-reported database
name. Connections to the same actual database still compare equal, regardless
of URL spelling or credentials. No existing migration was changed.

`cargo check --all-targets --locked` and the ARM64 release image build are used
for compilation verification. No automated test suite is run for this deployment.

## Source traceability

The deployment preserves the existing dirty working tree; no changes were
committed or discarded. The source archive excludes Git metadata, build output,
environment files, deployment credentials and logs.

- Base commit: `2412214` plus the current working-tree changes.
- Corrected archive SHA-256: `e783d3c2ac396dadcbd75186eb562c4ca6ee94f49e2c9db53894ed046ac2b15e`.
- Corrected ECR tag: `workspace-20260905-e783d3c2`.
- Deployed image digest: `sha256:ac60b07a41635376e32a93f64cc54334e29cf6d8845e8ef69cf4052e7af7db58`.
- Credential scan of the 139-file archived source: no production secret matches.

## Hands-on operational verification

- CloudFormation reports `UPDATE_COMPLETE`; the replacement uses launch template version 2 and the corrected image.
- Public HTTPS `/healthz` and `/readyz`: HTTP 200. Readiness checks both production and testing PostgreSQL pools.
- Public HTTPS `/api/version`: service `silicon-briefcase`, API `v1`, contract `0.3.0`, build `0.1.0`, 42 operations.
- Unsigned POST to `/webhook/`: HTTP 401 `unauthenticated`.
- Briefcase ALB target: healthy. IAM's original target and HTTPS readiness remain healthy.
- API and worker remain active with zero service restarts, running as UID/GID 10001. CloudWatch receives worker maintenance logs for both database planes and testing-environment lifecycle maintenance.
- Live SQL confirms separate database names and server addresses. The API role is neither superuser nor RLS-bypassing on both databases; the worker role is non-superuser with the required RLS bypass on both.
- EC2 role can access the Briefcase S3 bucket. Runtime credential files are root-owned mode 0600.
- A one-off invocation of the deployed API image with both pools intentionally pointed at the same actual database, using different connection-option spellings, exits with the expected isolation-guard error before serving. Existing services and data were not modified. An initial probe first hit the production loopback-bind restriction; the corrected probe reached and verified the intended database guard.

## Follow-up

At the final check (09:50 UTC), the healthy replacement was serving traffic while
AWS was retiring the failed first instance `i-045d6c9b4c624edac`. The target group's
configured 900-second connection-drain period kept the instance refresh at 50%
until that retirement finishes. This is infrastructure cleanup, not a failure of
the live replacement. The drain setting was preserved to protect future uploads;
the refresh must not yet be represented as `Successful`.

At inspection, IAM's webhook registration was pending review, with no active
delivery URL; its pending URL matches the endpoint above. The application OBO
endpoint catalog was empty. Hosting does not approve IAM's registration review.
These need to be addressed before claiming working hosted webhook/OBO flows.

The requested next round of hands-on end-to-end CLI/client/API testing remains
separate from this hosting rollout and its operational health checks.
