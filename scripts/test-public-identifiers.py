"""Full PostgreSQL migration verification in a dedicated disposable database.
Set IDENTIFIER_TEST_POSTGRES to an administrative local PostgreSQL URL to run.
"""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import unittest
from urllib.parse import urlsplit, urlunsplit
import uuid

spec=importlib.util.spec_from_file_location('migration',Path(__file__).with_name('migrate-public-identifiers.py'))
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
ROOT=Path(__file__).resolve().parents[1]
MAP={'applications':[{'legacy_id':'tos>example','app_id':'example','org_id':'tos'}],
'identities':[{'kind':'carbon','legacy_id':'saket','public_id':'c:saket'},
 {'kind':'silicon','legacy_id':'cos:tos','public_id':'si:cos'},
 {'kind':'membership','legacy_id':'saket[tos]','public_id':'c:saket[tos]'},
 {'kind':'membership','legacy_id':'cos:tos[tos]','public_id':'si:cos[tos]'}]}

@unittest.skipUnless(os.environ.get('IDENTIFIER_TEST_POSTGRES'),'set IDENTIFIER_TEST_POSTGRES to a disposable test cluster')
class MigrationTest(unittest.TestCase):
 def setUp(self):
  self.admin=os.environ['IDENTIFIER_TEST_POSTGRES'];self.name='briefcase_ids_'+uuid.uuid4().hex
  self.run_sql('CREATE DATABASE '+self.name,self.admin)
  url=urlsplit(self.admin);self.url=urlunsplit((url.scheme,url.netloc,'/'+self.name,url.query,url.fragment))
  self.addCleanup(lambda:self.run_sql('DROP DATABASE '+self.name+' WITH (FORCE)',self.admin))
  schema='\n'.join(p.read_text() for p in sorted((ROOT/'migrations').glob('*.sql')) if not p.name.startswith('0046_'))
  self.run_sql('BEGIN;'+schema+'COMMIT;')
  self.run_sql("""
   BEGIN; SET CONSTRAINTS ALL DEFERRED;
   INSERT INTO briefcase.organizations(org_id) VALUES('tos'),('other');
   INSERT INTO briefcase.organization_members(org_id,actor_type,actor_id,principal_id,membership_id) VALUES
   ('tos','carbon','saket','11111111-1111-4111-8111-111111111111','22222222-2222-4222-8222-222222222222'),
   ('tos','silicon','cos:tos','33333333-3333-4333-8333-333333333333','44444444-4444-4444-8444-444444444444');
   INSERT INTO briefcase.iam_identity_bindings VALUES
   ('00000000-0000-0000-0000-000000000000','carbon','saket','11111111-1111-4111-8111-111111111111'),
   ('00000000-0000-0000-0000-000000000000','silicon','cos:tos','33333333-3333-4333-8333-333333333333'),
   ('00000000-0000-0000-0000-000000000000','membership','saket[tos]','22222222-2222-4222-8222-222222222222'),
   ('00000000-0000-0000-0000-000000000000','membership','cos:tos[tos]','44444444-4444-4444-8444-444444444444');
   INSERT INTO briefcase.iam_identity_backfill VALUES('00000000-0000-0000-0000-000000000000',true);
   INSERT INTO briefcase.entries(org_id,entry_id,parent_id,entry_type,name,root_type,system_kind,owner_type,owner_id,origin_app_id,created_by_type,created_by_id,updated_by_type,updated_by_id) VALUES
   ('tos','00000000-0000-4000-8000-000000000001',null,'folder','apps','private','apps_root','carbon','saket',null,'carbon','saket','carbon','saket'),
   ('tos','00000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000001','folder','tos>example','private','app_root','carbon','saket','tos>example','carbon','saket','carbon','saket'),
   ('tos','00000000-0000-4000-8000-000000000003','00000000-0000-4000-8000-000000000002','folder','private','private','app_private','carbon','saket','tos>example','carbon','saket','carbon','saket'),
   ('tos','00000000-0000-4000-8000-000000000004','00000000-0000-4000-8000-000000000003','folder','cos:tos','private','app_actor','silicon','cos:tos','tos>example','carbon','saket','carbon','saket'),
   ('tos','00000000-0000-4000-8000-000000000005','00000000-0000-4000-8000-000000000004','folder','saket-tos>example.txt','private',null,'silicon','cos:tos','tos>example','carbon','saket','carbon','saket');
   INSERT INTO briefcase.audit_events(org_id,audit_id,entry_id,actor_type,actor_id,origin_app_id,action,request_id,metadata)
   VALUES('tos',gen_random_uuid(),null,'carbon','saket','tos>example','fixture',gen_random_uuid(),'{"opaque":"saket tos>example"}');
   INSERT INTO briefcase.idempotency_records(org_id,actor_type,actor_id,origin_app_id,operation,idempotency_key,request_hash,status,response_status,response_body,locked_until,expires_at)
   VALUES('tos','carbon','saket','tos>example','fixture','key-123456',decode(repeat('ab',32),'hex'),'completed',200,'{"opaque":"saket tos>example"}',now(),now()+interval '1 day');
   INSERT INTO briefcase.entries(org_id,entry_id,parent_id,entry_type,name,root_type,owner_type,owner_id,origin_app_id,content_type,size_bytes,current_version_id,created_by_type,created_by_id,updated_by_type,updated_by_id) VALUES
   ('tos','00000000-0000-4000-8000-000000000006','00000000-0000-4000-8000-000000000004','file','content.txt','private','silicon','cos:tos','tos>example','text/plain',5,'00000000-0000-4000-8000-000000000007','carbon','saket','carbon','saket');
   INSERT INTO briefcase.entry_versions(org_id,entry_id,version_id,version_number,source,storage_backend,bucket_name,storage_region,storage_prefix,storage_encryption_mode,object_key,checksum_algorithm,checksum_type,checksum_value,size_bytes,content_type,created_by_type,created_by_id) VALUES
   ('tos','00000000-0000-4000-8000-000000000006','00000000-0000-4000-8000-000000000007',1,'upload','platform','fixture-bucket','test-region','opaque/saket','sse_s3','opaque/tos>example/saket','sha256','full_object','preserved-checksum',5,'text/plain','carbon','saket');
   INSERT INTO briefcase.testing_environments(org_id,environment_id,name,created_by_type,created_by_id,iam_environment_id,iam_app_id,iam_environment_key_digest,iam_environment_key_ciphertext,iam_environment_key_nonce,iam_app_secret_ciphertext,iam_app_secret_nonce,status,deleted_at,purge_after)
   VALUES('tos','88888888-8888-4888-8888-888888888888','Fixture','carbon','saket','88888888-8888-4888-8888-888888888888','tos>example',decode(repeat('ab',32),'hex'),decode('abcd','hex'),decode(repeat('12',12),'hex'),decode('cdef','hex'),decode(repeat('34',12),'hex'),'deleted',now(),now()+interval '1 day');
   COMMIT;
  """)
  self.run_sql('BEGIN;'+(ROOT/'migrations/0046_public_identifier_schema.sql').read_text()+'COMMIT;')
 def run_sql(self,sql,url=None,ok=True):
  p=subprocess.run(['psql','-XAt','--set','ON_ERROR_STOP=1',url or self.url],input=sql,text=True,capture_output=True)
  if ok:self.assertEqual(p.returncode,0,p.stderr)
  else:self.assertNotEqual(p.returncode,0)
  return p.stdout.strip()
 def state(self):
  value=json.loads(self.run_sql("SELECT json_build_object('members',(SELECT json_agg(row(actor_type,actor_id,principal_id,membership_id)) FROM briefcase.organization_members),'bindings',(SELECT json_agg(row(environment_id,identity_kind,public_id,local_id)) FROM briefcase.iam_identity_bindings),'paths',(SELECT json_agg(row(entry_id,path)) FROM briefcase.entries),'rls',(SELECT json_agg(row(relname,relrowsecurity,relforcerowsecurity)) FROM pg_class WHERE relnamespace='briefcase'::regnamespace AND relkind='r'),'fk',(SELECT json_agg(row(conname,condeferrable,condeferred)) FROM pg_constraint WHERE contype='f' AND connamespace='briefcase'::regnamespace));"))
  return json.dumps({k:sorted(v or [],key=lambda x:json.dumps(x,sort_keys=True)) for k,v in value.items()},sort_keys=True)
 def test_preview_and_apply_preserve_bindings_content_links_and_fences(self):
  before=json.loads(self.state())
  self.run_sql("SELECT briefcase.resolve_iam_identity_key('00000000-0000-0000-0000-000000000000','carbon','c:saket',gen_random_uuid())",ok=False)
  self.run_sql(m.statement(MAP))
  self.assertEqual(json.loads(self.state()),before)
  self.run_sql(m.statement(MAP,True))
  after=json.loads(self.state())
  self.assertEqual(after['rls'],before['rls']);self.assertEqual(after['fk'],before['fk'])
  self.assertEqual(sorted(x['f4'] for x in after['bindings']),sorted(x['f4'] for x in before['bindings']))
  self.assertEqual(self.run_sql("SELECT actor_id FROM briefcase.organization_members ORDER BY actor_type"),'c:saket\nsi:cos')
  self.assertEqual(self.run_sql("SELECT path FROM briefcase.entries WHERE entry_id='00000000-0000-4000-8000-000000000005'"),'apps/example/private/si:cos/saket-tos>example.txt')
  self.assertEqual(self.run_sql("SET briefcase.org_id='tos'; SELECT briefcase.resolve_identifier_path('apps/tos>example/private/cos:tos/saket-tos>example.txt');").splitlines()[-1],'apps/example/private/si:cos/saket-tos>example.txt')
  self.assertEqual(self.run_sql("SET briefcase.org_id='other'; SELECT briefcase.resolve_identifier_path('apps/tos>example/private/cos:tos');").splitlines()[-1],'apps/tos>example/private/cos:tos')
  self.assertEqual(self.run_sql("SELECT object_key||'|'||checksum_value||'|'||created_by_id FROM briefcase.entry_versions"),'opaque/tos>example/saket|preserved-checksum|c:saket')
  self.assertEqual(self.run_sql("SELECT iam_app_id||'|'||encode(iam_app_secret_ciphertext,'hex')||'|'||encode(iam_app_secret_nonce,'hex') FROM briefcase.testing_environments"),'example|cdef|'+'34'*12)
  self.run_sql("UPDATE briefcase.entry_versions SET object_key='changed'",ok=False)
  self.assertEqual(self.run_sql("SELECT metadata->>'opaque' FROM briefcase.audit_events LIMIT 1"),'saket tos>example')
  self.assertEqual(self.run_sql("SELECT encode(request_hash,'hex') FROM briefcase.idempotency_records"),'ab'*32)
  self.assertEqual(self.run_sql("SELECT response_body->>'opaque' FROM briefcase.idempotency_records"),'saket tos>example')
  self.run_sql(m.statement(MAP,True))
  self.assertEqual(json.loads(self.state()),after)
  self.run_sql("INSERT INTO briefcase.entries(org_id,entry_id,parent_id,entry_type,name,root_type,owner_type,owner_id,created_by_type,created_by_id,updated_by_type,updated_by_id) VALUES('tos',gen_random_uuid(),'00000000-0000-4000-8000-000000000001','folder','tos>example','private','carbon','c:saket','carbon','c:saket','carbon','c:saket')",ok=False)
  self.run_sql("INSERT INTO briefcase.entries(org_id,entry_id,parent_id,entry_type,name,root_type,owner_type,owner_id,created_by_type,created_by_id,updated_by_type,updated_by_id) VALUES('tos','00000000-0000-4000-8000-000000000099','00000000-0000-4000-8000-000000000001','folder','unused','private','carbon','c:saket','carbon','c:saket','carbon','c:saket')")
  self.run_sql("UPDATE briefcase.entries SET name='tos>example' WHERE entry_id='00000000-0000-4000-8000-000000000099'",ok=False)
 def test_testing_plane_mapping_is_explicit_and_keeps_its_uuid(self):
  environment='99999999-9999-4999-8999-999999999999'
  self.run_sql("SET briefcase.testing_environment_id='"+environment+"'; INSERT INTO briefcase.organizations(org_id) VALUES('"+environment+":tos'); INSERT INTO briefcase.organization_members(org_id,actor_type,actor_id,principal_id,membership_id) VALUES('"+environment+":tos','carbon','saket','55555555-5555-4555-8555-555555555555','66666666-6666-4666-8666-666666666666'); INSERT INTO briefcase.iam_identity_bindings VALUES('"+environment+"','carbon','saket','55555555-5555-4555-8555-555555555555'),('"+environment+"','membership','saket[tos]','66666666-6666-4666-8666-666666666666');")
  self.run_sql(m.statement(MAP,True),ok=False)
  mapping={**MAP,'identities':MAP['identities']+[{'kind':'carbon','legacy_id':'saket','public_id':'c:tester','testing_environment_id':environment},{'kind':'membership','legacy_id':'saket[tos]','public_id':'c:tester[tos]','testing_environment_id':environment}]}
  self.run_sql(m.statement(mapping,True))
  self.assertEqual(self.run_sql("SELECT actor_id FROM briefcase.organization_members WHERE org_id='tos' AND actor_type='carbon'"),'c:saket')
  self.assertEqual(self.run_sql("SELECT actor_id FROM briefcase.organization_members WHERE org_id='"+environment+":tos'"),'c:tester')
  self.assertEqual(self.run_sql("SET briefcase.testing_environment_id='"+environment+"'; SELECT briefcase.resolve_iam_identity_key('"+environment+"','carbon','c:tester',gen_random_uuid())").splitlines()[-1],'55555555-5555-4555-8555-555555555555')
 def test_signed_webhook_aggregate_uuid_and_receipt_remain_exact(self):
  self.run_sql("INSERT INTO briefcase.webhook_receipts(source,event_id,org_id,event_type,aggregate_type,aggregate_id,aggregate_version,signature_timestamp,payload_sha256,status,processed_at) VALUES('silicon-iam','historical-uuid-event','tos','silicon.updated','silicon','33333333-3333-4333-8333-333333333333',1,now(),decode(repeat('ab',32),'hex'),'processed',now())")
  before=self.run_sql("SELECT to_jsonb(r)::text FROM briefcase.webhook_receipts r")
  self.run_sql(m.statement(MAP,True))
  self.assertEqual(self.run_sql("SELECT to_jsonb(r)::text FROM briefcase.webhook_receipts r"),before)
 def test_missing_map_and_path_collision_fail_atomically(self):
  before=json.loads(self.state())
  missing={**MAP,'identities':MAP['identities'][:1]}
  self.run_sql(m.statement(missing,True),ok=False)
  self.assertEqual(json.loads(self.state()),before)
  self.run_sql("INSERT INTO briefcase.entries(org_id,entry_id,parent_id,entry_type,name,root_type,owner_type,owner_id,created_by_type,created_by_id,updated_by_type,updated_by_id) VALUES('tos',gen_random_uuid(),'00000000-0000-4000-8000-000000000001','folder','example','private','carbon','saket','carbon','saket','carbon','saket')")
  before=json.loads(self.state());self.run_sql(m.statement(MAP,True),ok=False)
  self.assertEqual(json.loads(self.state()),before)

class MappingTest(unittest.TestCase):
 def test_collision_kind_and_owner_checks(self):
  for data in [{**MAP,'applications':MAP['applications']+[{'legacy_id':'other>example','app_id':'example','org_id':'other'}]},
               {**MAP,'applications':[{'legacy_id':'other>example','app_id':'example','org_id':'tos'}]},
               {**MAP,'identities':[{'kind':'silicon','legacy_id':'cos:tos','public_id':'c:cos'}]},
               {**MAP,'identities':[{'kind':'carbon','legacy_id':'saket','public_id':'c:ab'}]},
               {**MAP,'identities':[{'kind':'membership','legacy_id':'saket[tos]','public_id':'c:saket[other]'}]},
               {**MAP,'identities':[{'kind':'membership','legacy_id':'cos:tos[tos]','public_id':'c:cos[tos]'}]},
               {**MAP,'identities':[MAP['identities'][0],{'kind':'membership','legacy_id':'saket[tos]','public_id':'c:other[tos]'}]}]:
   with self.assertRaises(ValueError):m.statement(data)

if __name__=='__main__':unittest.main()
