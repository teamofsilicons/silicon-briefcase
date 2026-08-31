CREATE TABLE briefcase.idempotency_records (
    org_id text NOT NULL,
    actor_type text NOT NULL CHECK (actor_type IN ('carbon', 'silicon')),
    actor_id text NOT NULL,
    origin_app_id text NOT NULL DEFAULT '',
    operation text NOT NULL,
    idempotency_key text NOT NULL,
    request_hash bytea NOT NULL CHECK (octet_length(request_hash) = 32),
    status text NOT NULL DEFAULT 'in_progress'
        CHECK (status IN ('in_progress', 'completed')),
    response_status smallint CHECK (response_status BETWEEN 100 AND 599),
    response_headers jsonb,
    response_body jsonb,
    resource_id uuid,
    locked_until timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (
        org_id,
        actor_type,
        actor_id,
        origin_app_id,
        operation,
        idempotency_key
    ),
    FOREIGN KEY (org_id, actor_type, actor_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id),
    CHECK (origin_app_id = btrim(origin_app_id)),
    CHECK (octet_length(origin_app_id) <= 255),
    CHECK (operation = btrim(operation) AND octet_length(operation) BETWEEN 1 AND 255),
    CHECK (octet_length(idempotency_key) BETWEEN 8 AND 255),
    CHECK (jsonb_typeof(response_headers) = 'object' OR response_headers IS NULL),
    CHECK (
        (status = 'in_progress'
            AND response_status IS NULL
            AND response_headers IS NULL
            AND response_body IS NULL)
        OR (status = 'completed' AND response_status IS NOT NULL)
    ),
    CHECK (expires_at > created_at)
);

CREATE INDEX idempotency_records_expiry_idx
    ON briefcase.idempotency_records (expires_at, org_id)
    WHERE status = 'in_progress';

CREATE TRIGGER idempotency_records_set_updated_at
BEFORE UPDATE ON briefcase.idempotency_records
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

CREATE TABLE briefcase.audit_events (
    org_id text NOT NULL,
    audit_id uuid NOT NULL,
    entry_id uuid,
    actor_type text NOT NULL CHECK (actor_type IN ('carbon', 'silicon')),
    actor_id text NOT NULL,
    origin_app_id text,
    action text NOT NULL,
    request_id text NOT NULL,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    occurred_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, audit_id),
    FOREIGN KEY (org_id)
        REFERENCES briefcase.organizations (org_id)
        ON DELETE CASCADE,
    FOREIGN KEY (org_id, actor_type, actor_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id),
    CHECK (action = btrim(action) AND octet_length(action) BETWEEN 1 AND 255),
    CHECK (request_id = btrim(request_id) AND octet_length(request_id) BETWEEN 1 AND 255),
    CHECK (jsonb_typeof(metadata) = 'object')
);

CREATE INDEX audit_events_entry_recent_idx
    ON briefcase.audit_events (org_id, entry_id, occurred_at DESC, audit_id DESC)
    WHERE entry_id IS NOT NULL;

CREATE INDEX audit_events_actor_recent_idx
    ON briefcase.audit_events (
        org_id,
        actor_type,
        actor_id,
        occurred_at DESC,
        audit_id DESC
    );

CREATE FUNCTION briefcase.retain_latest_entry_audits()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
BEGIN
    IF NEW.entry_id IS NULL THEN
        RETURN NULL;
    END IF;

    PERFORM pg_advisory_xact_lock(
        hashtextextended(NEW.org_id || ':' || NEW.entry_id::text, 0)
    );

    DELETE FROM briefcase.audit_events AS audit
     WHERE audit.org_id = NEW.org_id
       AND audit.audit_id IN (
            SELECT stale.audit_id
              FROM briefcase.audit_events AS stale
             WHERE stale.org_id = NEW.org_id
               AND stale.entry_id = NEW.entry_id
             ORDER BY stale.occurred_at DESC, stale.audit_id DESC
             OFFSET 100
       );

    RETURN NULL;
END;
$$;

CREATE TRIGGER audit_events_retain_latest_entry_events
AFTER INSERT ON briefcase.audit_events
FOR EACH ROW
EXECUTE FUNCTION briefcase.retain_latest_entry_audits();

CREATE TABLE briefcase.outbox_events (
    org_id text NOT NULL,
    event_id uuid NOT NULL,
    topic text NOT NULL,
    aggregate_type text NOT NULL,
    aggregate_id text NOT NULL,
    aggregate_version bigint CHECK (aggregate_version IS NULL OR aggregate_version >= 0),
    payload jsonb NOT NULL,
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'processing', 'delivered', 'dead_letter')),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    lease_token uuid,
    lease_expires_at timestamptz,
    last_error text,
    delivered_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, event_id),
    FOREIGN KEY (org_id)
        REFERENCES briefcase.organizations (org_id)
        ON DELETE CASCADE,
    CHECK (topic = btrim(topic) AND octet_length(topic) BETWEEN 1 AND 255),
    CHECK (
        aggregate_type = btrim(aggregate_type)
        AND octet_length(aggregate_type) BETWEEN 1 AND 255
    ),
    CHECK (aggregate_id = btrim(aggregate_id) AND octet_length(aggregate_id) BETWEEN 1 AND 255),
    CHECK (jsonb_typeof(payload) = 'object'),
    CHECK ((lease_token IS NULL) = (lease_expires_at IS NULL)),
    CHECK (
        (status = 'processing' AND lease_token IS NOT NULL)
        OR (status <> 'processing' AND lease_token IS NULL)
    ),
    CHECK ((status = 'delivered') = (delivered_at IS NOT NULL)),
    CHECK (last_error IS NULL OR octet_length(last_error) <= 4000)
);

CREATE INDEX outbox_events_claim_idx
    ON briefcase.outbox_events (org_id, available_at, created_at, event_id)
    WHERE status = 'pending';

CREATE INDEX outbox_events_expired_lease_idx
    ON briefcase.outbox_events (org_id, lease_expires_at, event_id)
    WHERE status = 'processing';

CREATE INDEX outbox_events_aggregate_idx
    ON briefcase.outbox_events (
        org_id,
        aggregate_type,
        aggregate_id,
        aggregate_version
    );

CREATE TRIGGER outbox_events_set_updated_at
BEFORE UPDATE ON briefcase.outbox_events
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

CREATE TABLE briefcase.search_documents (
    org_id text NOT NULL,
    entry_id uuid NOT NULL,
    filename text NOT NULL,
    extracted_content text,
    extraction_status text NOT NULL DEFAULT 'pending' CHECK (
        extraction_status IN ('pending', 'indexed', 'unsupported', 'failed')
    ),
    extraction_error_code text,
    filename_search tsvector GENERATED ALWAYS AS (
        to_tsvector('simple'::regconfig, filename)
    ) STORED,
    content_search tsvector GENERATED ALWAYS AS (
        to_tsvector('simple'::regconfig, coalesce(extracted_content, ''))
    ) STORED,
    indexed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, entry_id),
    FOREIGN KEY (org_id, entry_id)
        REFERENCES briefcase.entries (org_id, entry_id)
        ON DELETE CASCADE,
    CHECK (filename = btrim(filename)),
    CHECK (octet_length(filename) BETWEEN 1 AND 255),
    CHECK (
        (extraction_status = 'failed' AND extraction_error_code IS NOT NULL)
        OR (extraction_status <> 'failed' AND extraction_error_code IS NULL)
    ),
    CHECK (
        (extraction_status = 'indexed' AND indexed_at IS NOT NULL)
        OR extraction_status <> 'indexed'
    )
);

CREATE INDEX search_documents_filename_gin_idx
    ON briefcase.search_documents USING gin (filename_search);

CREATE INDEX search_documents_content_gin_idx
    ON briefcase.search_documents USING gin (content_search);

CREATE INDEX search_documents_status_idx
    ON briefcase.search_documents (org_id, extraction_status, updated_at, entry_id);

CREATE TRIGGER search_documents_set_updated_at
BEFORE UPDATE ON briefcase.search_documents
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

CREATE TABLE briefcase.webhook_receipts (
    source text NOT NULL,
    event_id text NOT NULL,
    org_id text NOT NULL,
    event_type text NOT NULL,
    aggregate_type text NOT NULL,
    aggregate_id text NOT NULL,
    aggregate_version bigint NOT NULL CHECK (aggregate_version >= 0),
    signature_timestamp timestamptz NOT NULL,
    payload_sha256 bytea NOT NULL CHECK (octet_length(payload_sha256) = 32),
    status text NOT NULL DEFAULT 'received'
        CHECK (status IN ('received', 'processed', 'ignored', 'failed')),
    failure_code text,
    received_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    processed_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (source, event_id),
    FOREIGN KEY (org_id)
        REFERENCES briefcase.organizations (org_id)
        ON DELETE CASCADE,
    CHECK (source = btrim(source) AND octet_length(source) BETWEEN 1 AND 255),
    CHECK (event_id = btrim(event_id) AND octet_length(event_id) BETWEEN 1 AND 255),
    CHECK (event_type = btrim(event_type) AND octet_length(event_type) BETWEEN 1 AND 255),
    CHECK (
        aggregate_type = btrim(aggregate_type)
        AND octet_length(aggregate_type) BETWEEN 1 AND 255
    ),
    CHECK (aggregate_id = btrim(aggregate_id) AND octet_length(aggregate_id) BETWEEN 1 AND 255),
    CHECK (
        (status = 'failed' AND failure_code IS NOT NULL)
        OR (status <> 'failed' AND failure_code IS NULL)
    ),
    CHECK (
        (status IN ('processed', 'ignored', 'failed') AND processed_at IS NOT NULL)
        OR (status = 'received' AND processed_at IS NULL)
    )
);

CREATE INDEX webhook_receipts_org_version_idx
    ON briefcase.webhook_receipts (
        org_id,
        aggregate_type,
        aggregate_id,
        aggregate_version DESC
    );

CREATE INDEX webhook_receipts_pending_idx
    ON briefcase.webhook_receipts (org_id, received_at, source, event_id)
    WHERE status = 'received';

CREATE TRIGGER webhook_receipts_set_updated_at
BEFORE UPDATE ON briefcase.webhook_receipts
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();
