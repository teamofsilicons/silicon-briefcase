# Work-account ID fix — 17 November 2026

Briefcase's API accepted work-account IDs such as `c:ada.example`, but the
account cache's database checks accepted only personal IDs. Affected requests
failed with HTTP 500 before reaching Organizations, drives, or files.

Migration 0054 was applied to the production Accounts database on 2026-11-17.
It widens the checks on current IDs and ID history to the same format already
accepted by the API. It does not rewrite existing accounts, files, or sessions.
The running API, worker, and website were not restarted or replaced.

- Tested migration source: `f60566514c5e07506d22277fdffbac5e2795d066`.
- One-shot migrator image: `234951665042.dkr.ecr.us-east-1.amazonaws.com/silicon-briefcase-production@sha256:4341cbb458febdbc29289542b18dd92d3736935e610a1984d181402c17bac3f2`.
- Pre-change RDS snapshot: `briefcase-work-ids-pre-20261117113337`.
- Successful migration and rollback-only probe: SSM command `56b39660-e98e-4fd1-933c-3356a0a2f83f`.
- Production recorded migration 54 with no unsuccessful migrations. Both
  work-account ID types and their history entries passed an insert probe under
  the API role, and the probe transaction was rolled back.
- The API remained ready, all three services remained active, and the shared
  agent's Organizations and drive requests succeeded. The affected user
  confirmed that the workspace opened after retrying.
- The first SSM attempt stopped before making changes because the instance role
  could not describe snapshots; the snapshot was verified with the deployment
  account before the successful retry.

The tested backend library suite passed 217 tests against a fresh PostgreSQL
database, and the identity migration suite passed eight tests, including an
upgrade using a non-superuser schema owner. Formatting, strict Clippy, and all
four [PR checks](https://github.com/teamofsilicons/silicon-briefcase/pull/42)
passed for the tested source. The [ARM64 image build](https://github.com/teamofsilicons/silicon-briefcase/actions/runs/44075017895)
passed and its source revision and archive checksum were verified.

Keep migration 0054 in future migration images. Do not reimpose the old checks
on a database containing work-account IDs; the prior API is compatible with the
widened checks, but its older migration runner does not contain migration 0054.
