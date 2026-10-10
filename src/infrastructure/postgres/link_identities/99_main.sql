-- briefcase-migrate link-identities, the program: undo the run in place if
-- the mapping changed, re-key, move, share, account for, verify, report.

DO $$
DECLARE
    previous_run uuid;
    previous_sha text;
    previous_report jsonb;
    this_run uuid := pg_temp.link_run_id();
    legacy text[];
    touched text[];
    drive_key text;
    run_report jsonb;
BEGIN
    PERFORM pg_temp.link_check_actor_columns();
    PERFORM pg_temp.link_maintenance_on();
    IF EXISTS (SELECT 1 FROM briefcase.account_uuid_mappings) THEN
        RAISE EXCEPTION 'Accounts UUIDs were already migrated; historical identity-link ledgers are sealed and cannot be replayed or reverted';
    END IF;

    SELECT run.run_id, run.mapping_sha256, run.report
      INTO previous_run, previous_sha, previous_report
      FROM briefcase.identity_link_runs AS run
     WHERE run.status = 'applied';
    IF previous_run IS NOT NULL
        AND previous_sha = (SELECT mapping_sha256 FROM link_settings) THEN
        -- The same mapping is already in place: nothing to do.
        PERFORM pg_temp.link_maintenance_off();
        INSERT INTO link_result
        VALUES (jsonb_set(COALESCE(previous_report, '{}'::jsonb), '{status}', '"already_applied"')
                || jsonb_build_object('applied_run', previous_run));
        RETURN;
    END IF;

    INSERT INTO briefcase.identity_link_runs (run_id, status, mapping_sha256, mapping)
    SELECT run_id, 'running', mapping_sha256, mapping FROM link_settings;
    IF previous_run IS NOT NULL THEN
        INSERT INTO link_state
        VALUES ('reverted', jsonb_build_array(pg_temp.link_revert(previous_run)));
    END IF;

    PERFORM pg_temp.link_resolve();
    legacy := ARRAY(SELECT org FROM link_legacy_orgs ORDER BY org);
    PERFORM pg_temp.link_rekey(legacy, NULL, false);
    PERFORM pg_temp.link_owners();
    PERFORM pg_temp.link_move_organizations();
    INSERT INTO link_state VALUES ('notifications', jsonb_build_array(pg_temp.link_move_notifications()));
    PERFORM pg_temp.link_storage_configs();
    INSERT INTO link_state VALUES ('tag_grants', jsonb_build_array(pg_temp.link_expand_tag_grants()));
    INSERT INTO link_state VALUES ('snapshot_grants', jsonb_build_array(pg_temp.link_snapshot_grants()));
    FOR drive_key IN
        SELECT DISTINCT ledger.org_id FROM briefcase.identity_link_ledger AS ledger
         WHERE ledger.run_id = this_run AND NOT (ledger.org_id = ANY (legacy))
         ORDER BY ledger.org_id
    LOOP
        PERFORM pg_temp.link_participants(drive_key);
    END LOOP;
    PERFORM pg_temp.link_usage();
    INSERT INTO link_state VALUES ('aliases', jsonb_build_array(pg_temp.link_aliases()));
    PERFORM pg_temp.link_accounts();

    touched := legacy || ARRAY(
        SELECT DISTINCT ledger.org_id FROM briefcase.identity_link_ledger AS ledger
         WHERE ledger.run_id = this_run AND NOT (ledger.org_id = ANY (legacy)));
    PERFORM pg_temp.link_verify(touched);
    -- Described while row security is still off, so every count is complete.
    run_report := pg_temp.link_report('applied', previous_run);
    PERFORM pg_temp.link_maintenance_off();

    UPDATE briefcase.identity_link_runs
       SET status = 'applied', finished_at = clock_timestamp(), report = run_report
     WHERE run_id = this_run;
    INSERT INTO link_result VALUES (run_report);
END;
$$;
