BEGIN;
SET LOCAL standard_conforming_strings=on;
SET LOCAL lock_timeout='15s';
SET LOCAL statement_timeout='5min';
CREATE TEMP TABLE identifier_apps AS SELECT * FROM jsonb_to_recordset(__APPLICATION_MAP__) AS m(legacy_id text,public_id text);
CREATE TEMP TABLE identifier_actors AS SELECT * FROM jsonb_to_recordset(__IDENTITY_MAP__) AS m(environment_id uuid,identity_kind text,legacy_id text,public_id text);
CREATE UNIQUE INDEX ON identifier_apps(legacy_id);
CREATE UNIQUE INDEX ON identifier_apps(public_id);
CREATE UNIQUE INDEX ON identifier_actors(environment_id,identity_kind,legacy_id);
CREATE UNIQUE INDEX ON identifier_actors(environment_id,identity_kind,public_id);
-- Exclusive locks and transactional restoration keep relaxed RLS invisible.
CREATE TEMP TABLE identifier_rls AS SELECT oid,oid::regclass AS rel,relrowsecurity,relforcerowsecurity FROM pg_class WHERE relnamespace='briefcase'::regnamespace AND relkind='r';
DO $$ DECLARE r record; BEGIN
 FOR r IN SELECT * FROM identifier_rls ORDER BY oid LOOP
 EXECUTE format('LOCK TABLE %s IN ACCESS EXCLUSIVE MODE',r.rel);
 EXECUTE format('ALTER TABLE %s DISABLE ROW LEVEL SECURITY',r.rel);
 END LOOP;
 IF NOT EXISTS(SELECT 1 FROM pg_trigger WHERE tgrelid='briefcase.entry_versions'::regclass AND tgname='entry_versions_are_immutable' AND tgenabled='O') THEN RAISE EXCEPTION 'Immutable version protection must be enabled before migration'; END IF;
 IF to_regclass('briefcase.identifier_path_aliases') IS NULL THEN RAISE EXCEPTION 'Apply Briefcase migration 0046 first'; END IF;
 IF EXISTS(SELECT 1 FROM briefcase.idempotency_records WHERE status='in_progress')
 OR EXISTS(SELECT 1 FROM briefcase.testing_environment_idempotency WHERE status='in_progress')
 OR EXISTS(SELECT 1 FROM briefcase.delegated_uploads WHERE status IN ('reserved','receiving','staged','cleanup_pending'))
 OR EXISTS(SELECT 1 FROM briefcase.multipart_uploads WHERE status IN ('initiated','uploading','completing'))
 OR EXISTS(SELECT 1 FROM briefcase.outbox_events WHERE status IN ('pending','processing'))
 THEN RAISE EXCEPTION 'Drain in-flight writes, uploads, cleanup and notifications before migration'; END IF;
 IF EXISTS(SELECT 1 FROM briefcase.iam_identity_bindings b
 WHERE b.public_id NOT SIMILAR TO '(c|si):%'
 AND NOT EXISTS(SELECT 1 FROM identifier_actors m WHERE m.environment_id=b.environment_id AND m.identity_kind=b.identity_kind AND m.legacy_id=b.public_id))
 THEN RAISE EXCEPTION 'Retained UUID identity binding missing from trusted IAM mapping'; END IF;
END $$;
CREATE TEMP TABLE identifier_bindings_before AS SELECT environment_id,identity_kind,local_id FROM briefcase.iam_identity_bindings;
CREATE TEMP TABLE identifier_paths_before AS SELECT org_id,entry_id,path FROM briefcase.entries;
-- Defer only member-reference FKs, restore their exact original timing below.
CREATE TEMP TABLE identifier_constraints AS SELECT conrelid::regclass AS rel,conname,condeferrable,condeferred FROM pg_constraint WHERE contype='f' AND confrelid='briefcase.organization_members'::regclass;
DO $$ DECLARE r record; BEGIN
 FOR r IN SELECT * FROM identifier_constraints LOOP
 EXECUTE format('ALTER TABLE %s ALTER CONSTRAINT %I DEFERRABLE INITIALLY DEFERRED',r.rel,r.conname);
 END LOOP;
END $$;
SET CONSTRAINTS ALL DEFERRED;
-- This one trigger protects immutable object versions. Only typed author text
-- is translated; storage descriptors, UUIDs and bytes remain untouched.
ALTER TABLE briefcase.entry_versions DISABLE TRIGGER entry_versions_are_immutable;
DO $$ DECLARE r record; missing boolean; BEGIN
 FOR r IN SELECT table_name,column_name FROM information_schema.columns WHERE table_schema='briefcase' AND table_name IN (SELECT relname FROM pg_class WHERE relnamespace='briefcase'::regnamespace AND relkind='r') AND data_type='text' AND column_name IN ('app_id','origin_app_id','iam_app_id') ORDER BY table_name,column_name LOOP
 EXECUTE format('SELECT EXISTS(SELECT 1 FROM briefcase.%I t WHERE position(''>'' in t.%I)>0 AND NOT EXISTS(SELECT 1 FROM identifier_apps m WHERE m.legacy_id=t.%I))',r.table_name,r.column_name,r.column_name) INTO missing;
 IF missing THEN RAISE EXCEPTION 'Application reference missing from trusted IAM mapping'; END IF;
 EXECUTE format('UPDATE briefcase.%I t SET %I=m.public_id FROM identifier_apps m WHERE t.%I=m.legacy_id AND m.public_id<>m.legacy_id',r.table_name,r.column_name,r.column_name);
 END LOOP;
 FOR r IN SELECT c.table_name,c.column_name,replace(c.column_name,'_id','_type') AS kind_column
 FROM information_schema.columns c WHERE c.table_schema='briefcase' AND c.table_name IN (SELECT relname FROM pg_class WHERE relnamespace='briefcase'::regnamespace AND relkind='r') AND c.data_type='text' AND c.column_name LIKE '%\_id' ESCAPE '\'
 AND EXISTS(SELECT 1 FROM information_schema.columns k WHERE k.table_schema=c.table_schema AND k.table_name=c.table_name AND k.column_name=replace(c.column_name,'_id','_type'))
 AND EXISTS(SELECT 1 FROM information_schema.columns o WHERE o.table_schema=c.table_schema AND o.table_name=c.table_name AND o.column_name='org_id')
 ORDER BY c.table_name,c.column_name LOOP
 EXECUTE format('SELECT EXISTS(SELECT 1 FROM briefcase.%I t JOIN briefcase.organizations o USING(org_id) WHERE t.%I IN (''carbon'',''silicon'') AND t.%I IS NOT NULL AND t.%I NOT SIMILAR TO ''(c|si):%%'' AND NOT EXISTS(SELECT 1 FROM identifier_actors m WHERE m.environment_id=COALESCE(o.testing_environment_id,''00000000-0000-0000-0000-000000000000'') AND m.identity_kind=t.%I AND m.legacy_id=t.%I))',r.table_name,r.kind_column,r.column_name,r.column_name,r.kind_column,r.column_name) INTO missing;
 IF missing THEN RAISE EXCEPTION 'Actor reference missing from trusted IAM mapping'; END IF;
 EXECUTE format('UPDATE briefcase.%I t SET %I=m.public_id FROM identifier_actors m,briefcase.organizations o WHERE o.org_id=t.org_id AND m.environment_id=COALESCE(o.testing_environment_id,''00000000-0000-0000-0000-000000000000'') AND m.identity_kind=t.%I AND m.legacy_id=t.%I AND m.public_id<>m.legacy_id',r.table_name,r.column_name,r.kind_column,r.column_name);
 END LOOP;

END $$;
SET CONSTRAINTS ALL IMMEDIATE;
ALTER TABLE briefcase.entry_versions ENABLE TRIGGER entry_versions_are_immutable;
UPDATE briefcase.iam_identity_bindings b SET public_id=m.public_id FROM identifier_actors m
WHERE b.environment_id=m.environment_id AND b.identity_kind=m.identity_kind AND b.public_id=m.legacy_id AND m.public_id<>m.legacy_id;
-- Only structural names change; ordinary user filenames never undergo search/replace.
UPDATE briefcase.entries SET name=origin_app_id WHERE system_kind IN ('app_root','app_container') AND origin_app_id IS NOT NULL AND name<>origin_app_id;
UPDATE briefcase.entries SET name=owner_id WHERE system_kind IN ('actor_root','app_actor') AND name<>owner_id;
INSERT INTO briefcase.identifier_path_aliases(org_id,legacy_path,entry_id)
SELECT old.org_id,old.path,old.entry_id FROM identifier_paths_before old JOIN briefcase.entries e USING(org_id,entry_id)
WHERE old.path<>e.path ON CONFLICT(org_id,legacy_path) DO NOTHING;
DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM identifier_paths_before old JOIN briefcase.entries e USING(org_id,entry_id)
 JOIN briefcase.identifier_path_aliases a ON a.org_id=old.org_id AND a.legacy_path=old.path WHERE old.path<>e.path AND a.entry_id<>old.entry_id)
 THEN RAISE EXCEPTION 'Permanent path alias collision'; END IF;
 IF EXISTS(SELECT 1 FROM briefcase.identifier_path_aliases a JOIN briefcase.entries e ON e.org_id=a.org_id AND e.path=a.legacy_path WHERE e.entry_id<>a.entry_id AND e.deleted_at IS NULL)
 THEN RAISE EXCEPTION 'Permanent path alias conflicts with an existing entry'; END IF;
 IF EXISTS(SELECT * FROM identifier_bindings_before EXCEPT SELECT environment_id,identity_kind,local_id FROM briefcase.iam_identity_bindings)
 OR EXISTS(SELECT environment_id,identity_kind,local_id FROM briefcase.iam_identity_bindings EXCEPT SELECT * FROM identifier_bindings_before)
 THEN RAISE EXCEPTION 'Immutable UUID binding changed'; END IF;
END $$;
-- Keep request hashes/AEAD receipts verbatim but retire their old-name replay window.
UPDATE briefcase.idempotency_records SET expires_at=greatest(created_at+interval '1 microsecond',clock_timestamp()) WHERE expires_at>clock_timestamp();
UPDATE briefcase.testing_environment_idempotency SET expires_at=greatest(created_at+interval '1 microsecond',clock_timestamp()) WHERE expires_at>clock_timestamp();
ALTER TABLE briefcase.testing_environments VALIDATE CONSTRAINT testing_environments_iam_app_id_check;
SET CONSTRAINTS ALL IMMEDIATE;
DO $$ DECLARE r record; BEGIN
 FOR r IN SELECT * FROM identifier_constraints LOOP
 EXECUTE format('ALTER TABLE %s ALTER CONSTRAINT %I %s INITIALLY %s',r.rel,r.conname,CASE WHEN r.condeferrable THEN 'DEFERRABLE' ELSE 'NOT DEFERRABLE' END,CASE WHEN r.condeferred THEN 'DEFERRED' ELSE 'IMMEDIATE' END);
 END LOOP;
 FOR r IN SELECT * FROM identifier_rls ORDER BY oid LOOP
 EXECUTE format('ALTER TABLE %s %s ROW LEVEL SECURITY',r.rel,CASE WHEN r.relrowsecurity THEN 'ENABLE' ELSE 'DISABLE' END);
 EXECUTE format('ALTER TABLE %s %s ROW LEVEL SECURITY',r.rel,CASE WHEN r.relforcerowsecurity THEN 'FORCE' ELSE 'NO FORCE' END);
 END LOOP;
END $$;
__FINISH__;
