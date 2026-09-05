-- IAM OAuth introspection identifies a subject by immutable principal and
-- membership UUIDs. Public Carbon/Silicon handles, roles, and tags arrive in
-- signed Application webhook snapshots, so retain both identities and require
-- an exact cross-binding before constructing request authority.

ALTER TABLE briefcase.organization_members
    ADD COLUMN principal_id uuid,
    ADD COLUMN membership_id uuid,
    ADD COLUMN authorization_epoch bigint,
    ADD CONSTRAINT organization_members_authorization_epoch_check
        CHECK (authorization_epoch IS NULL OR authorization_epoch > 0);

ALTER TABLE briefcase.organizations
    ADD COLUMN iam_organization_id uuid;

CREATE UNIQUE INDEX organizations_iam_organization_uidx
    ON briefcase.organizations (iam_organization_id)
    WHERE iam_organization_id IS NOT NULL;

CREATE UNIQUE INDEX organization_members_principal_uidx
    ON briefcase.organization_members (org_id, principal_id)
    WHERE principal_id IS NOT NULL;

CREATE UNIQUE INDEX organization_members_membership_uidx
    ON briefcase.organization_members (org_id, membership_id)
    WHERE membership_id IS NOT NULL;

COMMENT ON COLUMN briefcase.organization_members.principal_id IS
    'Immutable IAM principal UUID used to cross-bind OAuth introspection.';
COMMENT ON COLUMN briefcase.organization_members.membership_id IS
    'Immutable IAM organization-membership UUID used to cross-bind OAuth introspection.';
COMMENT ON COLUMN briefcase.organization_members.authorization_epoch IS
    'IAM membership authorization epoch cross-bound to online introspection.';
COMMENT ON COLUMN briefcase.organizations.iam_organization_id IS
    'Internal IAM organization UUID used only to route scoped webhook tombstones.';

CREATE OR REPLACE FUNCTION briefcase.resolve_iam_organization_id(target uuid)
RETURNS text
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $function$
    SELECT organization.org_id
    FROM briefcase.organizations AS organization
    WHERE organization.iam_organization_id = target
$function$;

REVOKE ALL ON FUNCTION briefcase.resolve_iam_organization_id(uuid) FROM PUBLIC;

DO $block$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.resolve_iam_organization_id(uuid) TO briefcase_api';
    END IF;
END
$block$;
