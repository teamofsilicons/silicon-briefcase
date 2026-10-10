-- Identity links: the cutover from organizations to personal drives.
--
-- The operator command `briefcase-migrate link-identities --file mapping.csv`
-- re-keys what pre-migration organizations stored (IAM public ids such as
-- `c:saket`) to Silicon Accounts uuids and moves each organization's files
-- into personal drives. These tables record every run, the mapping it applied
-- and enough of what it changed to undo it, so a corrected mapping can be
-- applied before the cutover by simply running the command again. Nothing
-- here changes existing rows.

CREATE TABLE briefcase.identity_link_runs (
    run_id uuid PRIMARY KEY,
    -- `running` only inside the run's own transaction.
    status text NOT NULL CHECK (status IN ('running', 'applied', 'reverted')),
    -- SHA-256 of the mapping file the operator supplied (hex).
    mapping_sha256 text NOT NULL CHECK (mapping_sha256 ~ '^[0-9a-f]{64}$'),
    -- The mapping rows as parsed, and the report the command printed.
    mapping jsonb NOT NULL,
    report jsonb NOT NULL DEFAULT '{}'::jsonb,
    started_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    finished_at timestamptz,
    reverted_at timestamptz,
    reverted_by_run uuid REFERENCES briefcase.identity_link_runs (run_id),
    CHECK ((status = 'reverted') = (reverted_at IS NOT NULL))
);

CREATE UNIQUE INDEX identity_link_runs_one_applied_uidx
    ON briefcase.identity_link_runs ((true))
    WHERE status = 'applied';

-- Which legacy identity became which Silicon Accounts account. The legacy
-- columns elsewhere (organization_members.principal_id, iam_identity_bindings,
-- delegated_uploads.iam_principal_id) are kept; this table is the bridge.
CREATE TABLE briefcase.identity_links (
    run_id uuid NOT NULL REFERENCES briefcase.identity_link_runs (run_id),
    identity_kind text NOT NULL CHECK (identity_kind IN ('carbon', 'silicon')),
    -- The public id pre-migration rows store (`c:saket`, `si:scout`).
    iam_public_id text COLLATE "C" NOT NULL,
    -- The IAM principal UUID (or Briefcase's local key for it), when known.
    iam_principal_id uuid,
    accounts_uuid text COLLATE "C" NOT NULL,
    linked_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    -- Where the link came from, such as `mapping:12` (line 12 of the file).
    source text NOT NULL CHECK (octet_length(source) BETWEEN 1 AND 255),
    PRIMARY KEY (run_id, identity_kind, iam_public_id),
    UNIQUE (run_id, accounts_uuid),
    CHECK (accounts_uuid ~ '^[A-Za-z0-9]{3,32}$'),
    CHECK (iam_public_id ~ '^(c|si):[a-z0-9_-]{1,64}$')
);

CREATE INDEX identity_links_accounts_idx
    ON briefcase.identity_links (accounts_uuid, run_id);

-- What one run changed, in order, so a later run can undo it before applying
-- a corrected mapping. `item` names the kind of change, `org_id` the drive or
-- organization it happened in, `key` identifies the row, `before` holds the
-- values to restore (NULL when the run created the row).
CREATE TABLE briefcase.identity_link_ledger (
    run_id uuid NOT NULL REFERENCES briefcase.identity_link_runs (run_id),
    seq bigint NOT NULL,
    item text NOT NULL CHECK (item IN (
        'account', 'drive', 'participant', 'folder', 'tag', 'storage_config',
        'usage', 'move', 'entry', 'tag_grant', 'grant', 'notification', 'multipart'
    )),
    org_id text NOT NULL,
    key jsonb NOT NULL,
    before jsonb,
    PRIMARY KEY (run_id, seq)
);

-- Addresses from before personal drives (`/org/{org}/{path}`) that still
-- resolve: links in messages, todos and emails keep working after the move.
-- One row per moved entry (and per legacy container), naming the entry it
-- became; a link below a moved folder resolves through the longest prefix.
CREATE TABLE briefcase.legacy_path_aliases (
    legacy_org text NOT NULL,
    legacy_path text NOT NULL,
    drive text COLLATE "C" NOT NULL,
    entry_id uuid NOT NULL,
    run_id uuid NOT NULL REFERENCES briefcase.identity_link_runs (run_id),
    PRIMARY KEY (legacy_org, legacy_path),
    CHECK (octet_length(legacy_org) BETWEEN 1 AND 255),
    CHECK (octet_length(legacy_path) BETWEEN 1 AND 2048)
);

CREATE INDEX legacy_path_aliases_entry_idx
    ON briefcase.legacy_path_aliases (drive, entry_id);
CREATE INDEX legacy_path_aliases_run_idx
    ON briefcase.legacy_path_aliases (run_id);

REVOKE ALL ON briefcase.identity_link_runs FROM PUBLIC;
REVOKE ALL ON briefcase.identity_links FROM PUBLIC;
REVOKE ALL ON briefcase.identity_link_ledger FROM PUBLIC;
REVOKE ALL ON briefcase.legacy_path_aliases FROM PUBLIC;

-- A delegated byte transfer names only its upload; the reservation's drive is
-- found here so callers no longer have to send it.
CREATE POLICY drive_definer_lookup ON briefcase.delegated_uploads
    FOR SELECT TO CURRENT_USER USING (true);

-- The drive and current path a pre-migration address leads to: the moved
-- entry with the longest legacy path equal to, or a folder above, the
-- requested one. NULL when nothing matches. The caller still authorizes the
-- target inside its drive; an alias never grants access.
CREATE FUNCTION briefcase.resolve_legacy_location(p_org text, p_path text)
RETURNS TABLE (drive text, path text)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    WITH requested AS (
        SELECT btrim(p_path, '/') AS value
    ),
    alias AS (
        SELECT candidate.drive, candidate.entry_id, candidate.legacy_path
          FROM briefcase.legacy_path_aliases AS candidate, requested
         WHERE candidate.legacy_org = p_org
           AND requested.value <> ''
           AND (candidate.legacy_path = requested.value
                OR starts_with(requested.value, candidate.legacy_path || '/'))
         ORDER BY length(candidate.legacy_path) DESC
         LIMIT 1
    )
    SELECT alias.drive,
           CASE
               WHEN remainder.value = '' THEN target.path
               WHEN target.path = '.' THEN remainder.value
               ELSE target.path || '/' || remainder.value
           END
      FROM alias
      CROSS JOIN requested
      CROSS JOIN LATERAL (
          SELECT substr(requested.value, length(alias.legacy_path) + 2) AS value
      ) AS remainder
      JOIN briefcase.entries AS target
        ON target.org_id = alias.drive
       AND target.entry_id = alias.entry_id
      JOIN briefcase.organizations AS owner_drive
        ON owner_drive.org_id = alias.drive
       AND owner_drive.owner_account_uuid = alias.drive
$$;

-- The drive a delegated upload reservation belongs to. NULL when no drive
-- holds exactly one reservation with that id.
CREATE FUNCTION briefcase.delegated_upload_drive(p_upload_id uuid)
RETURNS text
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    SELECT CASE WHEN count(*) = 1 THEN min(upload.org_id) END
      FROM briefcase.delegated_uploads AS upload
      JOIN briefcase.organizations AS owner_drive
        ON owner_drive.org_id = upload.org_id
       AND owner_drive.owner_account_uuid = upload.org_id
     WHERE upload.upload_id = p_upload_id
$$;

REVOKE ALL ON FUNCTION briefcase.resolve_legacy_location(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.delegated_upload_drive(uuid) FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        GRANT EXECUTE ON FUNCTION briefcase.resolve_legacy_location(text, text) TO briefcase_api;
        GRANT EXECUTE ON FUNCTION briefcase.delegated_upload_drive(uuid) TO briefcase_api;
    END IF;
END;
$$;
