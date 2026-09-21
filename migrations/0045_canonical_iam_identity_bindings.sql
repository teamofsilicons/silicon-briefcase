-- Canonical IAM IDs authenticate requests; retained UUIDs become Briefcase's
-- own storage keys. The environment is part of every binding, including ones
-- used by pending delegated uploads after a test-world reset.
-- The production migration owners do not bypass forced RLS. Hold exclusive
-- locks while reading all retained rows with RLS temporarily disabled. SQLx
-- runs this file in one transaction: other sessions never observe relaxed RLS,
-- and any error restores the original flags along with all data changes.
LOCK TABLE briefcase.organizations,briefcase.organization_members,briefcase.delegated_uploads IN ACCESS EXCLUSIVE MODE;
ALTER TABLE briefcase.organizations DISABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.organization_members DISABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.delegated_uploads DISABLE ROW LEVEL SECURITY;

CREATE TABLE briefcase.iam_identity_bindings (
 environment_id uuid NOT NULL,
 identity_kind text NOT NULL CHECK(identity_kind IN ('carbon','silicon','membership')),
 public_id text NOT NULL CHECK(length(public_id) BETWEEN 1 AND 255),
 local_id uuid NOT NULL CHECK(local_id<>'00000000-0000-0000-0000-000000000000'),
 PRIMARY KEY(environment_id,identity_kind,public_id),
 UNIQUE(environment_id,identity_kind,local_id)
);
CREATE TABLE briefcase.iam_identity_backfill (
 environment_id uuid PRIMARY KEY,
 verified boolean NOT NULL DEFAULT false
);

-- Historical directory snapshots and upload reservations already contain both
-- canonical public IDs and UUID keys. Conflicts abort rather than change owners.
WITH identities AS (
 SELECT COALESCE(o.testing_environment_id,'00000000-0000-0000-0000-000000000000') AS environment_id,
 m.actor_type,m.actor_id,m.principal_id,m.membership_id,
 CASE WHEN o.testing_environment_id IS NULL THEN o.org_id ELSE split_part(o.org_id,':',2) END AS public_org_id
 FROM briefcase.organization_members m JOIN briefcase.organizations o USING(org_id)
 UNION
 SELECT COALESCE(testing_environment_id,'00000000-0000-0000-0000-000000000000'),
 actor_type,actor_id,iam_principal_id,iam_membership_id,
 CASE WHEN testing_environment_id IS NULL THEN org_id ELSE split_part(org_id,':',2) END
 FROM briefcase.delegated_uploads
)
INSERT INTO briefcase.iam_identity_bindings
 SELECT DISTINCT environment_id,actor_type,actor_id,principal_id FROM identities WHERE principal_id IS NOT NULL
 UNION
 SELECT DISTINCT environment_id,'membership',actor_id||'['||public_org_id||']',membership_id FROM identities WHERE membership_id IS NOT NULL;
INSERT INTO briefcase.iam_identity_backfill(environment_id)
 SELECT DISTINCT environment_id FROM briefcase.iam_identity_bindings;

ALTER TABLE briefcase.organizations ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.organization_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.delegated_uploads ENABLE ROW LEVEL SECURITY;

ALTER TABLE briefcase.iam_identity_bindings ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.iam_identity_bindings FORCE ROW LEVEL SECURITY;
CREATE POLICY identity_environment ON briefcase.iam_identity_bindings
 USING(environment_id=COALESCE(NULLIF(current_setting('briefcase.testing_environment_id',true),'')::uuid,'00000000-0000-0000-0000-000000000000'))
 WITH CHECK(environment_id=COALESCE(NULLIF(current_setting('briefcase.testing_environment_id',true),'')::uuid,'00000000-0000-0000-0000-000000000000'));
ALTER TABLE briefcase.iam_identity_backfill ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.iam_identity_backfill FORCE ROW LEVEL SECURITY;
CREATE POLICY identity_environment ON briefcase.iam_identity_backfill
 USING(environment_id=COALESCE(NULLIF(current_setting('briefcase.testing_environment_id',true),'')::uuid,'00000000-0000-0000-0000-000000000000'))
 WITH CHECK(environment_id=COALESCE(NULLIF(current_setting('briefcase.testing_environment_id',true),'')::uuid,'00000000-0000-0000-0000-000000000000'));

CREATE FUNCTION briefcase.resolve_iam_identity_key(p_environment uuid,p_kind text,p_public_id text,p_candidate uuid)
RETURNS uuid LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
DECLARE result uuid;
BEGIN
 IF p_environment IS DISTINCT FROM COALESCE(NULLIF(current_setting('briefcase.testing_environment_id',true),'')::uuid,'00000000-0000-0000-0000-000000000000')
 OR p_kind NOT IN ('carbon','silicon','membership') OR p_public_id IS NULL OR length(p_public_id) NOT BETWEEN 1 AND 255
 OR p_candidate IS NULL OR p_candidate='00000000-0000-0000-0000-000000000000' THEN
 RAISE EXCEPTION 'invalid canonical identity context' USING ERRCODE='42501'; END IF;
 SELECT local_id INTO result FROM briefcase.iam_identity_bindings
 WHERE environment_id=p_environment AND identity_kind=p_kind AND public_id=p_public_id;
 IF FOUND THEN RETURN result; END IF;
 IF EXISTS(SELECT 1 FROM briefcase.iam_identity_backfill WHERE environment_id=p_environment AND NOT verified) THEN
 RAISE EXCEPTION 'canonical identity backfill incomplete' USING ERRCODE='55000'; END IF;
 INSERT INTO briefcase.iam_identity_bindings VALUES(p_environment,p_kind,p_public_id,p_candidate)
 ON CONFLICT(environment_id,identity_kind,public_id) DO NOTHING;
 SELECT local_id INTO STRICT result FROM briefcase.iam_identity_bindings
 WHERE environment_id=p_environment AND identity_kind=p_kind AND public_id=p_public_id;
 RETURN result;
END $$;
REVOKE ALL ON FUNCTION briefcase.resolve_iam_identity_key(uuid,text,text,uuid) FROM PUBLIC;
DO $$ BEGIN IF to_regrole('briefcase_api') IS NOT NULL THEN
 GRANT EXECUTE ON FUNCTION briefcase.resolve_iam_identity_key(uuid,text,text,uuid) TO briefcase_api;
END IF; END $$;
COMMENT ON COLUMN briefcase.organization_members.principal_id IS 'Briefcase-owned identity key bound to canonical IAM public_id.';
COMMENT ON COLUMN briefcase.organization_members.membership_id IS 'Briefcase-owned membership key bound to canonical IAM membership ID.';
