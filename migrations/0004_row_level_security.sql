CREATE FUNCTION briefcase.current_org_id()
RETURNS text
LANGUAGE sql
STABLE
PARALLEL SAFE
SET search_path = pg_catalog
AS $$
    SELECT NULLIF(current_setting('briefcase.org_id', true), '')
$$;

ALTER TABLE briefcase.organizations ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.organizations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.organizations
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.organization_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.organization_members FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.organization_members
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.organization_tags ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.organization_tags FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.organization_tags
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.organization_member_tags ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.organization_member_tags FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.organization_member_tags
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.entries ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.entries FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.entries
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.entry_closure ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.entry_closure FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.entry_closure
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.organization_storage_configs ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.organization_storage_configs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.organization_storage_configs
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.entry_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.entry_versions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.entry_versions
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.permission_grants ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.permission_grants FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.permission_grants
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.access_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.access_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.access_requests
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.multipart_uploads ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.multipart_uploads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.multipart_uploads
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.multipart_parts ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.multipart_parts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.multipart_parts
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.idempotency_records ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.idempotency_records FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.idempotency_records
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.audit_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.audit_events FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.audit_events
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.outbox_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.outbox_events FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.outbox_events
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.search_documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.search_documents FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.search_documents
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

ALTER TABLE briefcase.webhook_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.webhook_receipts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.webhook_receipts
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());
