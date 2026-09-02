-- Independent read, write, update, and delete rights.
--
-- The product contract is explicit that update authority does not imply
-- deletion and that write authority does not imply update, so a grant carries
-- a set of rights rather than one escalating level. The set is stored as a
-- bitmask so a grant, a request, and a decision all share one representation
-- and no query needs a subquery to test a single right.

CREATE FUNCTION briefcase.access_bit(access_right text)
RETURNS integer
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
SET search_path = pg_catalog
AS $$
    SELECT CASE access_right
        WHEN 'read' THEN 1
        WHEN 'write' THEN 2
        WHEN 'update' THEN 4
        WHEN 'delete' THEN 8
    END;
$$;

-- Every valid mask includes read: nobody can act on an entry they cannot see.
CREATE FUNCTION briefcase.access_mask_is_valid(access_mask smallint)
RETURNS boolean
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
SET search_path = pg_catalog, briefcase
AS $$
    SELECT access_mask BETWEEN 1 AND 15
       AND (access_mask & briefcase.access_bit('read')) <> 0;
$$;

ALTER TABLE briefcase.permission_grants NO FORCE ROW LEVEL SECURITY;
ALTER TABLE briefcase.access_requests NO FORCE ROW LEVEL SECURITY;

ALTER TABLE briefcase.permission_grants ADD COLUMN access_mask smallint;

-- The retired `write` level conveyed every mutation right at once.
UPDATE briefcase.permission_grants
   SET access_mask = CASE access_level
        WHEN 'read' THEN briefcase.access_bit('read')
        ELSE briefcase.access_bit('read')
             + briefcase.access_bit('write')
             + briefcase.access_bit('update')
             + briefcase.access_bit('delete')
       END;

-- Dropping the column also drops its check constraint and the index that
-- carried it, so the principal lookup index is rebuilt over the new mask.
ALTER TABLE briefcase.permission_grants
    ALTER COLUMN access_mask SET NOT NULL,
    ADD CONSTRAINT permission_grants_access_mask_valid
        CHECK (briefcase.access_mask_is_valid(access_mask)),
    DROP COLUMN access_level;

DROP INDEX IF EXISTS briefcase.permission_grants_principal_idx;

CREATE INDEX permission_grants_principal_idx
    ON briefcase.permission_grants (
        org_id,
        principal_type,
        principal_id,
        entry_id,
        access_mask
    )
    WHERE revoked_at IS NULL;

ALTER TABLE briefcase.access_requests
    ADD COLUMN requested_access_mask smallint,
    ADD COLUMN granted_access_mask smallint;

UPDATE briefcase.access_requests
   SET requested_access_mask = CASE requested_access
            WHEN 'read' THEN briefcase.access_bit('read')
            ELSE briefcase.access_bit('read')
                 + briefcase.access_bit('write')
                 + briefcase.access_bit('update')
                 + briefcase.access_bit('delete')
           END,
       granted_access_mask = CASE granted_access
            WHEN 'read' THEN briefcase.access_bit('read')
            WHEN 'write' THEN briefcase.access_bit('read')
                 + briefcase.access_bit('write')
                 + briefcase.access_bit('update')
                 + briefcase.access_bit('delete')
            ELSE NULL
           END;

-- The retired level columns take their own check constraints with them,
-- including the composite decision check that referenced them.
ALTER TABLE briefcase.access_requests
    ALTER COLUMN requested_access_mask SET NOT NULL,
    ADD CONSTRAINT access_requests_requested_access_mask_valid
        CHECK (briefcase.access_mask_is_valid(requested_access_mask)),
    ADD CONSTRAINT access_requests_granted_access_mask_valid
        CHECK (
            granted_access_mask IS NULL
            OR briefcase.access_mask_is_valid(granted_access_mask)
        ),
    ADD CONSTRAINT access_requests_decision_is_complete
        CHECK (
            (status = 'pending'
                AND granted_access_mask IS NULL
                AND decided_by_type IS NULL
                AND decided_by_id IS NULL
                AND decided_at IS NULL
                AND permission_grant_id IS NULL)
            OR (status = 'denied'
                AND granted_access_mask IS NULL
                AND decided_by_type IS NOT NULL
                AND decided_by_id IS NOT NULL
                AND decided_at IS NOT NULL
                AND permission_grant_id IS NULL)
            OR (status = 'approved'
                AND granted_access_mask IS NOT NULL
                AND decided_by_type IS NOT NULL
                AND decided_by_id IS NOT NULL
                AND decided_at IS NOT NULL
                AND permission_grant_id IS NOT NULL)
        ),
    DROP COLUMN requested_access,
    DROP COLUMN granted_access;

ALTER TABLE briefcase.permission_grants FORCE ROW LEVEL SECURITY;
ALTER TABLE briefcase.access_requests FORCE ROW LEVEL SECURITY;
