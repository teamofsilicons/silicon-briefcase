# Accounts UUID backfill

`briefcase-migrate rekey-accounts` consumes the immutable export from Accounts.
The exact CSV header is `old_uuid,new_uuid,kind`; targets must be canonical
lowercase UUIDv4 values and kind must be `carbon` or `silicon`. Use the same export
for every app and every retained Briefcase database. Never generate an app-local
mapping. All legacy cached accounts, including deleted accounts, must be present.
Already canonical accounts may remain unchanged; exported accounts with no local
references are recorded as tombstones too.

## Maintenance sequence

1. Complete and verify `link-identities` if pre-Accounts organization data still
   needs to be attached to personal accounts. Its historical ledger becomes
   permanently sealed once UUID backfill is applied.
2. Stop API writes and capture a custom-format database backup. Let the normal
   worker finish or honestly fail its pending work, then stop the worker. The
   command refuses any in-progress general/test-environment idempotency record,
   reserved/receiving/staged/cleanup-pending delegated upload, initiated/uploading/
   completing multipart upload, or pending/processing outbox event. Resolve those
   rows through the normal lifecycle; do not claim they were delivered. Confirm
   external cleanup requirements separately before a production cutover.
3. Apply pending schema migrations, including 0051 and 0052, as the existing
   migrator role. Keep `BRIEFCASE_MIGRATOR_DATABASE_URL` out of shell history and
   logs. Use the same non-superuser schema owner that created the RLS functions.
4. Run `briefcase-migrate rekey-accounts --file accounts-uuids.csv --dry-run --json`.
   Save its report, review reference counts and unmapped/merge refusals. This runs
   every update and constraint check inside a transaction, then rolls it back.
5. Apply with the identical command without `--dry-run`. Save the report and its
   SHA-256. Repeat the same command once: expect `already_applied`, zero new rows,
   and an empty `changed_references`. A conflicting, chained, swapped, ambiguous,
   duplicate, wrong-kind or merging mapping is refused atomically.
6. Restart the rebuilt service only after Accounts and all consumers have applied
   the coordinated mapping. Sign in again. Verify previous files, shares,
   custodian drives, permissions, public links, public-ID aliases, notification
   history and configured storage. Verify an old token/proof is rejected and a
   fresh token reads the same bytes. Do not expose the API during partial cutover.

Rollback is a coordinated restore of the pre-cutover databases and app versions,
not an inverse mapping or another `link-identities` run. Restore after quiescing
writes; a restore after new writes otherwise loses those writes. Keep object
storage untouched and retain the backup until acceptance.

## Preserved data and identity references

The transaction rekeys account cache/history and webhook receipts, custodian and
Silicon sharing allowances, account references in organization memberships,
entries, versions, grants, access requests, audits, notifications, uploads,
idempotency and test-environment rows. Personal-drive tenant keys change with
these references; structural identity fields in stored JSON also change.
The migration fails closed if a new paired actor column or known identity UUID
column appears without an audited migration rule.

Briefcase resource UUIDv7 values, IAM private principal/membership UUIDs, public
IDs, names, free text, paths, S3 buckets/prefixes/object keys/provider versions,
BYOS role External IDs, KMS configuration, checksums and ciphertext do not change.
Current storage uses provider-side encryption, so no account-derived application
AAD is rewritten. Legacy encrypted environment rows retain their bytes. The drive
API reads a persisted storage External ID instead of deriving a new trust identity
from the new account UUID.

Historical `identity_link_runs` and `identity_link_ledger` stay unchanged. Outbox
payloads also stay unchanged: migration seals their dispatch state and database
triggers prevent a later pending/processing replay or removal of the seal. New
post-cutover events remain deliverable. Incoming valid signed events naming retired
accounts or custodians are ignored; their receipt remains deduplicated. Old bearer
and proof subjects are denied even if their token/proof verification is cached.
Old identifiers are tombstones, never login aliases, and cannot recreate accounts.

## Local evidence

`tests/postgres_link_identities.rs` uses disposable databases owned by a
non-superuser without BYPASSRLS. It compares every table and trigger/RLS/foreign-key
setting around dry-run and replay, verifies resource/storage/legacy ciphertext
preservation, checks deleted accounts/custody/shares/JSON, rejects incomplete or
conflicting maps, rejects target collisions, and exercises historical replay fences.
`api::accounts_tests::uuid_backfill_rejects_old_credentials_and_preserves_file_access`
uses real PostgreSQL, signed test Accounts tokens and an object-store stand-in to
prove old bearer/proof rejection, late-event fencing, identical file bytes,
retained storage trust IDs and outsider denial. These local tests do not prove
real AWS AssumeRole or KMS interoperability.
