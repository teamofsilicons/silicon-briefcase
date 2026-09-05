# Silicon Briefcase backend

Silicon Briefcase is the organization-scoped file service used by Carbons,
Silicons, and IAM-authorized applications. The backend is written in Rust and
keeps filesystem metadata and authorization state in PostgreSQL while storing
file bytes in S3-compatible object storage.

The product contract lives in [UNDERSTANDING.md](./UNDERSTANDING.md) and the
public API in [openapi.yaml](./openapi.yaml). Start with the segregated
[API, Rust-client, CLI, and testing guides](./docs/README.md).
UNDERSTANDING.md is the product source of truth; the guides describe the API,
client, CLI, and deployment behavior.

## Process layout

- `briefcase-api` serves health checks, the `/api/v1` contract, and the IAM
  webhook receiver.
- `briefcase-worker` performs outbox delivery, search indexing and text
  extraction, multipart cleanup, retention, and reconciliation.
- `briefcase-migrate` is the only process that applies forward SQL migrations.

The three binaries share the `silicon_briefcase` library. API replicas never run
migrations automatically.

Each process uses a distinct PostgreSQL principal. `briefcase-api` requires a
non-superuser role without `BYPASSRLS`; `briefcase-worker` requires the separate
`BRIEFCASE_WORKER_DATABASE_URL` role with `BYPASSRLS`; and
`briefcase-migrate` uses `BRIEFCASE_MIGRATOR_DATABASE_URL`. Both long-running
processes fail startup when the effective role has the wrong capability.

The worker also loads the same `BRIEFCASE_S3_*` transport settings and AWS
credential chain as the API. Its identity must be able to delete platform
objects and assume every retained organization-storage role. Briefcase derives
the organization-specific STS External ID itself; it is never loaded from a job
or accepted from a client.

## What the service exposes

- **Clean permanent URLs.** Every entry carries a materialized
  organization-relative path, so `GET /org/{org_id}/{path}` resolves in one
  indexed lookup and shows the folder structure exactly as the contract does.
  Anything the caller cannot read answers `404`.
- **Proxied, sandboxed content.** File bytes are relayed by Briefcase, never by
  a signed provider URL, so a permanent URL is never a bearer capability. Reads
  are sandboxed by Content-Security-Policy, never sniffed, never cached, and
  support byte ranges for media playback.
- **One upload endpoint.** `POST /uploads` takes a whole file of any supported
  size and decides internally whether the bytes travel as a single provider
  request or as a durable multipart transfer, so a client never drives parts.
  Uploading over an existing file name is how a file is updated: the bytes
  become that file's next version and the history keeps the previous fifty.
- **Bounded volume, reported in bytes.** Every organization may upload 100 GiB
  per UTC day and store 1 PiB, both configurable per organization in the
  database. The daily counter is charged in the same transaction that publishes
  a file, and stored bytes are maintained by a trigger on the version rows
  themselves, so retention, purges, and cascading deletes all account for
  storage without knowing the counter exists. `GET /usage` reports exact byte
  counts rather than percentages.
- **Independent access rights.** A grant conveys any set of `read`, `write`,
  `update`, and `delete`. Update never implies deletion, write never implies
  update — write on a folder adds files to it and never replaces one already
  there — and `POST /permissions/effective` answers what the caller may
  actually do on up to a hundred named targets at once. Owning a folder always
  shows what is inside it, and conveys nothing over another member's file there
  beyond reading it. Tag members may read entries and add children to folders
  inside their tag tree, while mutating a peer's existing entry requires an
  explicit grant; organization owners and admins hold every operation everywhere.
  A member holding only a clean permanent URL can request access with
  `POST /access-requests` and its organization-relative path; hidden metadata
  remains undisclosed until a grant is approved.
- **A central inbox.** Grants, revocations, access requests, and decisions each
  write a notification in the same transaction as the change, so the inbox
  cannot disagree with the permissions it describes.
- **A filter language.** Folder contents page a hundred newest-first — pages
  are refilled after permission filtering rather than answered short — and a
  filter expression combines every documented predicate with `and`, `or`,
  `not`, and grouping. It is parsed in the domain, compiled to parameterized
  SQL, and its `permissions:` predicate is decided by domain policy.
- **One application endpoint.** `POST /obo/files` creates a file for a
  represented member, taking its destination from IAM-bound proof metadata and
  defaulting to `private/{actor}/apps/{app_id}`.

## Clients

- **Rust:** [`briefcase-client`](../briefcase-client-rust) is the official
  client crate. It negotiates the API version and verifies every operation's
  revision against this build before its first call, so an incompatible pairing
  fails at startup rather than mid-request.
- **Command line:** `briefcase`, in the same repository, is built on that crate
  and exposes the same operations to a shell. It has no capability the crate
  lacks; what it adds is a saved profile, a token stored under `~/.briefcase/`,
  `--json` output, and exit codes a script can branch on.

Any client can do the same by reading `GET /api/version`, which names the
served API majors and every operation with the revision of its request and
response shape.

## Local development

Prerequisites are Rust 1.98, PostgreSQL 16 or newer, and an S3-compatible service
such as MinIO. Start the local dependencies, copy the example environment, run
migrations, and then start the API and worker:

```bash
docker compose up -d postgres postgres-test minio minio-init
cp .env.example .env
set -a && source .env && set +a
cargo run --bin briefcase-migrate
BRIEFCASE_MIGRATOR_DATABASE_URL=postgres://briefcase:briefcase-local-only@127.0.0.1:5434/briefcase_test \
  cargo run --bin briefcase-migrate
cargo run --bin briefcase-api
cargo run --bin briefcase-worker
```

Run the API and worker in separate terminals after both migrations complete.
The API is ready to receive traffic only when both checks succeed:

```bash
curl --fail http://127.0.0.1:8080/healthz
curl --fail http://127.0.0.1:8080/readyz
```

The Compose MinIO service carries an obviously-local `MINIO_KMS_SECRET_KEY`
because Briefcase always stores objects encrypted, and MinIO answers
`501 NotImplemented` to an SSE-S3 upload when no key is configured. A MinIO
volume created before that setting existed will keep refusing uploads until the
service is recreated.

`deploy/postgres/001_runtime_roles.sql` creates the two local runtime roles on
a new Compose volume. If the volume predates those role definitions, create a
fresh development database volume or provision equivalent roles before running
the API and worker. Production role names may differ from the local canonical
names, but operators must grant equivalent schema/data-plane privileges after
migration. For `object_cleanup_jobs`, that specifically means tenant-scoped
`INSERT` for the API's abort enqueue and `SELECT`, `INSERT`, `UPDATE`, and
`DELETE` for the `BYPASSRLS` worker.

Local IAM fixtures are not accepted by the production profile. Integration
tests use an explicit mock IAM server and isolated credentials.

## Deploying

Production runs on private EC2 behind the shared Team of Silicons load
balancer, with RDS and S3, defined as one CloudFormation stack and shipped by
one script:

```bash
cp deploy/aws/production.env.example deploy/aws/production.env   # once
./deploy/deploy.sh                                               # build, push, deploy, replace
./deploy/dns.sh --value "$ALB_DNS_NAME" --apply                 # point Namecheap at it
```

[docs/deployment.md](./docs/deployment.md) has the runbook: the secret the instance
reads, what a first deploy needs, how to roll back, and the two things that
make Briefcase different from its neighbours — the worker's `BYPASSRLS` role
and the local disk uploads are staged on.

## Object cleanup and retention

Run migrations before deploying either runtime process. Multipart aborts are
snapshotted into `object_cleanup_jobs` in the same transaction that marks the
session aborted or expired. Cleanup jobs preserve the exact bucket, region,
prefix, role, encryption descriptor, object key, provider object version, or
provider upload ID that was originally persisted; later storage-configuration
rotation cannot redirect a destructive request.

The worker claims each cleanup job with `FOR UPDATE SKIP LOCKED`, makes the S3
call after committing the lease transaction, and deletes metadata only after
the provider confirms deletion. Each process admits at most
`BRIEFCASE_WORKER_CLEANUP_CONCURRENCY` provider calls concurrently and never
processes more than `BRIEFCASE_WORKER_BATCH_SIZE` jobs in a pass. Failures keep
the job with deterministic capped backoff and a redacted error code; cleanup is
never dead-lettered. A crash or expired lease can repeat a provider operation,
so exact-version deletes and multipart aborts must remain idempotent.

- `initiated` and `uploading` multipart sessions use their persisted fixed
  24-hour `expires_at` deadline. `completing` and `completed` sessions are never
  provider-aborted by retention.
- Active files retain their newest 50 versions. The current version is excluded
  from cleanup independently of its rank.
- Deleted subtrees remain recoverable until their persisted 45-day
  `purge_after`. Permanent metadata deletion occurs only after every version in
  the deletion batch has an `object_deleted` cleanup result.

An unresolved `completing` multipart session that still references a deleted
subtree intentionally blocks permanent purge. Its provider outcome is ambiguous
and requires completion reconciliation; operators should alert on cleanup queue
age and deletion batches that remain past `purge_after`.

## Quality checks

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo deny check
```

`tests/postgres_metadata.rs` exercises the SQL the repository actually builds —
root reconciliation, path resolution, the compiled filter language, the
notification inbox, the membership projection, and the application folder — and
skips itself unless a database is named:

```bash
docker compose up -d postgres postgres-test
cargo run --bin briefcase-migrate
BRIEFCASE_MIGRATOR_DATABASE_URL=postgres://briefcase:briefcase-local-only@127.0.0.1:5434/briefcase_test \
  cargo run --bin briefcase-migrate
BRIEFCASE_TEST_DATABASE_URL=postgres://briefcase:briefcase-local-only@127.0.0.1:5433/briefcase \
  cargo test --test postgres_metadata
```

Each run uses a fresh organization identifier, so it is repeatable without
deleting anything.

When the testing plane is enabled, API and worker startup compare PostgreSQL's
cluster system identifier, database OID, and server-reported database name for
the production and testing pools. Including the name avoids a false collision
between independently provisioned RDS databases inheriting the same IDs.
Startup fails if both connection strings resolve to the same database,
including through aliases or different runtime credentials.

`tests/s3_object_store.rs` does the same for object delivery — a stored object
streams back whole, and one exact range comes back as the bytes a media player
asked for:

```bash
docker compose up -d minio minio-init
BRIEFCASE_TEST_S3_BUCKET=briefcase-local cargo test --test s3_object_store
```

## Security model

Every protected request is bound to a current IAM membership and an
`X-Org-ID`. The contracted API is a bearer surface; an application acts only
through `POST /obo/files`, where IAM binds the proof to the exact method,
registered path, and body digest, consumes it once, and is never retried.
Entry IDs and permanent URLs are not capabilities: authorization is reevaluated
for browsing, filtering, direct reads, downloads, search, versions, and bin
access, and an entry the caller may not read is reported as missing.

Briefcase imports official `silicon-iam-client = "=1.2.0"` from crates.io;
all IAM calls use its typed APIs, with runtime auto-updates disabled. It
negotiates IAM API `v1` at startup. Bearer and OBO authorization come from
current online snapshots, cross-bound to identity, membership, audience and
environment. Full role/tag disclosure is required; webhooks do not grant
request authority. Fresh environments bootstrap without webhook delivery.
Snapshots cannot roll back newer projected membership versions or epochs.

IAM must have backend migration 0067 and testing migration 9003. The SDK owns
HTTP transport, redirects, version headers and its 4 MiB response limit.
`BRIEFCASE_IAM_REQUEST_TIMEOUT_MS` bounds the complete request;
`BRIEFCASE_IAM_MAX_RESPONSE_BYTES` additionally bounds decoded IAM models.
The old separate `BRIEFCASE_IAM_CONNECT_TIMEOUT_MS` setting is removed.

IAM webhook delivery is treated as an at-least-once notification stream, not
as request authorization. Briefcase verifies the exact raw body against the
secret selected by its signing-key version, retains prior key versions during
rotation, orders updates by each projected resource's own version, and safely
ignores authenticated event types it does not yet understand.

Every connection pins `search_path` to `public`. PostgreSQL's default path
starts with a schema named after the connecting role, and one runtime role is
named `briefcase` — the same name as the application schema — so an unqualified
name would otherwise resolve differently depending on who connected.

Do not place access tokens, OBO proofs, IAM application secrets, webhook
secrets, filenames, or object keys in logs or metrics.

Frontend note: the API supplies what the contract's interface notes need. An
entry response carries `render` so a client knows which renderer to open (and
when to say "unsupported file type"), and a folder the caller can navigate but
not fully read is omitted from listings rather than half-described — which is
the signal to show the "you might not be seeing all the contents of this
folder" note on a permission-based folder.

Historical-version restore is synchronous in the v1 API. It has an independent
`BRIEFCASE_RESTORE_TIMEOUT_SECONDS` deadline and
`BRIEFCASE_MAX_CONCURRENT_RESTORES` admission limit so multi-terabyte copies do
not consume the ordinary request pool without a bound. Individual S3 calls use
`BRIEFCASE_S3_OPERATION_TIMEOUT_SECONDS`; operators must size all three values
for the deployment's object size, bandwidth, and temporary-disk capacity.
