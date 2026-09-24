# Public identifier migration

Briefcase accepts Carbon IDs such as `c:saket`, Silicon IDs such as `si:cos`, and bare application IDs such as `briefcase`. Organization handles and internal UUIDs retain their existing meaning. The migration changes public references while preserving the local principal and membership UUID bindings introduced in migration 0045.

Stop Briefcase API, workers, upload clients and lifecycle coordinators before applying the data migration. Complete or explicitly cancel active metadata mutations, ordinary/delegated uploads, provider cleanup and outbox deliveries. The migration refuses in-flight records. Do not turn pending operations into completed records manually.

Apply schema migration 0046 with the normal `briefcase-migrate` workflow. It creates permanent-path aliases, adjusts the application-ID constraint, and blocks identity-key allocation while retained old-format bindings still exist. Existing data requires the offline mapping step before serving requests. Resolve duplicate Silicon/app handles in IAM first; an identity rename must never choose a different owner or UUID because its handle happens to match.

Prepare a private map from the approved IAM inventory. Include every retained actor, membership and referenced application, including external app origins. Include each testing environment explicitly. The file uses this structure:

```json
{
  "applications": [
    {"legacy_id":"tos>briefcase","app_id":"briefcase","org_id":"tos"}
  ],
  "identities": [
    {"kind":"carbon","legacy_id":"saket","public_id":"c:saket"},
    {"kind":"silicon","legacy_id":"cos:tos","public_id":"si:cos"},
    {"kind":"membership","legacy_id":"saket[tos]","public_id":"c:saket[tos]"},
    {"kind":"membership","legacy_id":"cos:tos[tos]","public_id":"si:cos[tos]"},
    {"kind":"carbon","legacy_id":"tester","public_id":"c:tester","testing_environment_id":"11111111-1111-4111-8111-111111111111"}
  ]
}
```

`DATABASE_URL` must select the database being migrated. Use its migration owner, with `psql`, `pg_dump` and `pg_restore` on PATH. Connection strings and mapping values are not logged. Preview runs the full migration and rolls it back:

```sh
python3 scripts/migrate-public-identifiers.py iam-public-id-map.json
```

After the preview succeeds, apply with a database backup:

```sh
python3 scripts/migrate-public-identifiers.py iam-public-id-map.json --apply --backup-dir /secure/backups/identifier-cutover
```

Repeat independently for production and the shared testing database. The shared testing database map must include each retained environment; the script refuses an unmapped old identity even when production contains the same old handle. Never use production principal/membership UUIDs as replacements for testing-plane keys.

The transaction locks Briefcase tables, temporarily disables RLS, defers the exact foreign keys that reference actor names, and restores their original flags before commit. It updates only typed actor/application columns and IAM public bindings. It briefly disables the immutable-version trigger solely to translate its author identifier, then restores the trigger after checking all foreign keys. A missing map, duplicate identity, path collision, ownership inconsistency or failed check rolls back the entire transaction, including RLS and constraint changes.

Application roots and actor roots receive canonical names. Existing entries retain UUIDs, permissions, version UUIDs, object keys, checksums, provider upload descriptors and file content. Structural renames update materialized descendant paths through the existing path triggers. Every changed old path is retained as an alias to the same entry UUID. Public and authenticated path lookup resolve that alias and then apply the ordinary current-entry authorization checks. Aliases never grant access. Another entry cannot reuse a retained alias path.

Ordinary user filenames, historical audit metadata, notification descriptions, request hashes and encrypted receipts are unchanged. Completed idempotency replay windows are expired because their immutable response bodies may contain old identifiers; clients must start new operations with fresh idempotency keys. UUID-derived AEAD associated data and ciphertext are preserved. Existing signed/provider receipts are not edited as text. Keep source records and backups for audit and rollback.

Update Briefcase/IAM/Honeycomb configuration and consumers in the same coordinated cutover. Reauthenticate under IAM's session migration rules. Before reopening writes, verify an existing user's private folder and permissions, old and new permanent links, an existing version download, app-owned roots, test-environment isolation, canonical identity-key resolution and fresh delegated upload authorization. Restore coordinated backups and old binaries if verification fails before writes reopen.

The PostgreSQL test harness creates and drops its own random database on the explicitly supplied test cluster:

```sh
IDENTIFIER_TEST_POSTGRES=postgresql://localhost/postgres python3 scripts/test-public-identifiers.py -v
```
