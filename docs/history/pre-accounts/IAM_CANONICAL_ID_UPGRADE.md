# Canonical IAM identity upgrade

Briefcase authenticates canonical Carbon and Silicon IDs from live IAM 3 authorization snapshots. The official client is pinned to `silicon-iam-client =3.0.0`. Older IAM may omit the top-level public ID; its authorized snapshot must still disclose it. Private IAM UUIDs do not establish identity. Actor kind, public ID, canonical membership ID, audience, organization, and testing environment are cross-checked before local keys are resolved.

Existing Briefcase UUIDs remain local persistence keys for directory projections, membership bindings, and delegated uploads. Ordinary actor and permission IDs remain canonical strings. The public Briefcase login/session contract keeps its local UUID fields, so existing Briefcase clients do not need to interpret IAM's internal wire schema.

Migration0045 seeds `iam_identity_bindings` from retained organization members and delegated uploads. Each binding includes the testing environment, with the nil UUID reserved for production. The live migrator roles do not bypass forced RLS, so the migration locks the source tables and temporarily disables/re-enables their RLS inside the same transaction while seeding. Other sessions cannot observe relaxed policies. Conflicting historical mappings abort migration. An existing environment with retained UUID keys blocks allocation of unknown canonical keys until its trusted export is verified; existing known mappings preserve their original keys. Empty, newly created worlds may allocate new local keys. Runtime access is limited to the resolver function, which requires its environment argument to match the transaction context.

Release sequence:

1. Stop Briefcase API/worker writers and retain a database backup and previous image.
2. Apply migrations through0045 in both production and the separate shared testing database using the normal migration role. Do not apply unrelated working-tree changes as part of this upgrade without reviewing them.
3. Obtain the complete private IAM identity export for the selected environment. Keep it outside the repository. Set `DATABASE_URL` using a database owner connection and ensure `psql` is available.
4. Run `python3 scripts/backfill-iam-identities.py /private/identity-mapping.json --production`. It validates the selected environment, all retained bindings, and the export, then rolls back. Repeat with `--apply` to commit.
5. For each retained testing environment, point `DATABASE_URL` at the shared testing database and use `--testing-environment-id UUID` instead of `--production`, first checking and then applying. No production fallback exists. Environments with no retained keys have no backfill-state row and require no import.
6. Start the updated services. Verify existing file access, a pending delegated upload, a test-world request, and signed membership lifecycle delivery before IAM cutover.

The import rejects missing historical mappings and conflicts instead of overwriting a local key. It is a cutover operation while writers are paused; it is not a later reconciliation tool for replacing keys allocated for new actors.

New signed webhook tombstones use `resource.membership_id` to identify the canonical membership. Retained notifications can still locate an existing member through its resource UUID. Profile aggregate keys accept bounded canonical identity strings; organization, event, and membership-resource UUIDs remain unchanged. No webhook cache replaces live IAM authorization.

Validation includes the IAM network/authority suite, a fresh PostgreSQL upgrade under the restricted runtime role, production/test-world key isolation, canonical and retained tombstone projection, and check/apply/recheck plus conflict rollback for the backfill command. Database integration tests use `BRIEFCASE_CANONICAL_TEST_DATABASE_URL` and require a fresh disposable database; do not point them at an existing development or live database.

The importer uses inline JSON recordsets and does not require database `TEMP` privileges. Imported public and local keys must each be unique within their kind.
