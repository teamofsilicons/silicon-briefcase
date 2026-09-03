-- Optional actor references must stay optional.
--
-- `revoked_by` and `decided_by` are recorded only once someone revokes a grant
-- or decides a request. Both foreign keys were declared MATCH FULL over
-- `(org_id, actor_type, actor_id)`, and because `org_id` is never null, MATCH
-- FULL demanded the actor columns as well: no grant could be created
-- unrevoked, and no access request could be created pending.
--
-- Default (MATCH SIMPLE) matching is what these columns need — a row with a
-- null actor is exempt from the reference — while the existing check
-- constraints continue to require the actor exactly when the timestamp is set.

ALTER TABLE briefcase.permission_grants
    DROP CONSTRAINT permission_grants_org_id_revoked_by_type_revoked_by_id_fkey,
    ADD CONSTRAINT permission_grants_revoked_by_fkey
        FOREIGN KEY (org_id, revoked_by_type, revoked_by_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id);

ALTER TABLE briefcase.access_requests
    DROP CONSTRAINT access_requests_org_id_decided_by_type_decided_by_id_fkey,
    ADD CONSTRAINT access_requests_decided_by_fkey
        FOREIGN KEY (org_id, decided_by_type, decided_by_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id);
