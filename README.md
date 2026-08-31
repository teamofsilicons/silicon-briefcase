# Silicon Briefcase backend

Silicon Briefcase is the organization-scoped file service used by Carbons,
Silicons, and IAM-authorized applications. The backend is written in Rust and
keeps filesystem metadata and authorization state in PostgreSQL while storing
file bytes in S3-compatible object storage.

The product contract lives in [UNDERSTANDING.md](./UNDERSTANDING.md), the public
API in [openapi.yaml](./openapi.yaml), and implementation interpretations in
[decisions.md](./decisions.md). Contract gaps are deliberately documented rather
than filled by undocumented endpoints.

## Process layout

- `briefcase-api` serves health checks, the `/api/v1` contract, and the IAM
  webhook receiver.
- `briefcase-worker` performs outbox delivery, indexing, multipart cleanup,
  retention, and reconciliation.
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

## Local development

Prerequisites are Rust 1.98, PostgreSQL 16 or newer, and an S3-compatible service
such as MinIO. Start the local dependencies, copy the example environment, run
migrations, and then start the API and worker:

```bash
docker compose up -d postgres minio minio-init
cp .env.example .env
set -a && source .env && set +a
cargo run --bin briefcase-migrate
cargo run --bin briefcase-api
cargo run --bin briefcase-worker
```

The PostgreSQL initialization directory creates the two local runtime roles on
a new Compose volume. If the volume predates those role definitions, create a
fresh development database volume or provision equivalent roles before running
the API and worker. Production role names may differ from the local canonical
names, but operators must grant equivalent schema/data-plane privileges after
migration. For `object_cleanup_jobs`, that specifically means tenant-scoped
`INSERT` for the API's abort enqueue and `SELECT`, `INSERT`, `UPDATE`, and
`DELETE` for the `BYPASSRLS` worker.

Local IAM fixtures are not accepted by the production profile. Integration
tests use an explicit mock IAM server and isolated credentials.

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

## Security model

Every protected request is bound to a current IAM membership and an
`X-Org-ID`. OBO requests are additionally bound to their verified application,
action, resource, and represented actor. Entry IDs and permanent URLs are not
capabilities. Authorization is reevaluated for browsing, direct reads, search,
versions, bin access, and temporary URL generation.

Do not place access tokens, OBO proofs, IAM application secrets, webhook
secrets, signed URLs, filenames, or object keys in logs or metrics.

Historical-version restore is synchronous in the v1 API. It has an independent
`BRIEFCASE_RESTORE_TIMEOUT_SECONDS` deadline and
`BRIEFCASE_MAX_CONCURRENT_RESTORES` admission limit so multi-terabyte copies do
not consume the ordinary request pool without a bound. Individual S3 calls use
`BRIEFCASE_S3_OPERATION_TIMEOUT_SECONDS`; operators must size all three values
for the deployment's object size, bandwidth, and temporary-disk capacity.
