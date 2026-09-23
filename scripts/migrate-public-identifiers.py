#!/usr/bin/env python3
"""Offline Briefcase public-ID migration. Preview rolls back; --apply backs up first.

DATABASE_URL selects one database. Supply every retained production/testing actor
and application mapping exported by IAM. Stop API/workers for the whole operation.
Connection secrets and mapping contents are never printed. See docs/identifier-migration.md.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import time
import uuid

spec = importlib.util.spec_from_file_location('backfill', Path(__file__).with_name('backfill-iam-identities.py'))
backfill = importlib.util.module_from_spec(spec)
spec.loader.exec_module(backfill)


def validated(document):
    apps, actors = [], []
    seen, destinations = set(), set()
    for row in document['applications']:
        old, new, org = row['legacy_id'], row['app_id'], row['org_id']
        if not re.fullmatch(r'[a-z][a-z0-9_-]{0,79}', new) or not re.fullmatch(r'[a-z0-9_-]{3,50}', org):
            raise ValueError('Invalid application mapping')
        if old != new and (old.count('>') != 1 or old.split('>')[0] != org):
            raise ValueError('Legacy app owning organization mismatch')
        if old in seen or new in destinations:
            raise ValueError('Application mapping collision')
        seen.add(old); destinations.add(new)
        apps.append(dict(legacy_id=old, public_id=new))
    seen, destinations = set(), set()
    for row in document['identities']:
        kind, old, new = row['kind'], row['legacy_id'], row['public_id']
        env = str(uuid.UUID(row.get('testing_environment_id') or str(uuid.UUID(int=0))))
        if kind not in ('carbon', 'silicon', 'membership') or not re.fullmatch(r'(?:c:[a-z0-9_-]{3,30}|si:[a-z0-9_-]{3,50})(?:\[[a-z0-9_-]{3,50}\])?', new):
            raise ValueError('Invalid typed identity mapping')
        if (kind == 'membership') != ('[' in new):
            raise ValueError('Membership IDs require an organization suffix')
        if kind == 'membership':
            legacy_membership = re.fullmatch(r'(.+)\[([a-z0-9_-]{3,50})\]', old)
            if not legacy_membership or legacy_membership.group(2) != new.rsplit('[', 1)[1][:-1]:
                raise ValueError('Membership owning organization mismatch')
            old_actor = legacy_membership.group(1)
            old_kind = 'carbon' if old_actor.startswith('c:') or ':' not in old_actor else 'silicon'
            if not new.startswith('c:' if old_kind == 'carbon' else 'si:'):
                raise ValueError('Membership actor kind mismatch')
        if kind == 'carbon' and not new.startswith('c:') or kind == 'silicon' and not new.startswith('si:'):
            raise ValueError('Identity kind mismatch')
        if (env, kind, old) in seen or (env, kind, new) in destinations:
            raise ValueError('Identity mapping collision')
        seen.add((env, kind, old)); destinations.add((env, kind, new))
        actors.append(dict(environment_id=env, identity_kind=kind, legacy_id=old, public_id=new))
    actor_destinations = {(row['environment_id'], row['identity_kind'], row['legacy_id']): row['public_id'] for row in actors if row['identity_kind'] != 'membership'}
    for row in actors:
        if row['identity_kind'] != 'membership':
            continue
        old_actor, suffix = row['legacy_id'].rsplit('[', 1)
        kind = 'carbon' if old_actor.startswith('c:') or ':' not in old_actor else 'silicon'
        actor = actor_destinations.get((row['environment_id'], kind, old_actor))
        if actor is not None and row['public_id'] != actor + '[' + suffix:
            raise ValueError('Membership and actor mappings disagree')
    return apps, actors


def statement(document, apply=False):
    apps, actors = validated(document)
    def literal(value): return "'" + json.dumps(value, separators=(',', ':')).replace("'", "''") + "'::jsonb"
    template = Path(__file__).with_name('public-identifier-migration.sql').read_text()
    return template.replace('__APPLICATION_MAP__', literal(apps)).replace('__IDENTITY_MAP__', literal(actors)).replace('__FINISH__', 'COMMIT' if apply else 'ROLLBACK')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mapping', type=Path)
    parser.add_argument('--apply', action='store_true')
    parser.add_argument('--backup-dir', type=Path)
    args = parser.parse_args()
    if args.apply and not args.backup_dir:
        parser.error('--apply requires --backup-dir')
    document = json.loads(args.mapping.read_text())
    sql = statement(document)
    env = backfill.database_environment()
    def execute(sql):
        result = subprocess.run(['psql', '-X', '--set', 'ON_ERROR_STOP=1', '--quiet'], input=sql, text=True, env=env, capture_output=True)
        if result.returncode:
            # psql includes SQL data in CONTEXT/DETAIL. Return the bounded, static
            # exception message only; never dump the source map or credentials.
            errors = [line.split('ERROR:', 1)[1].strip() for line in result.stderr.splitlines() if 'ERROR:' in line]
            raise RuntimeError('Migration rolled back: ' + (errors[0] if errors else 'SQL validation failed'))
    execute(sql)
    if args.apply:
        os.umask(0o077)
        args.backup_dir.mkdir(parents=True, exist_ok=True)
        backup = args.backup_dir / (time.strftime('%Y%m%dT%H%M%SZ', time.gmtime()) + '-' + str(uuid.uuid4()) + '.dump')
        result = subprocess.run(['pg_dump', '--format=custom', '--file', str(backup)], env=env, capture_output=True)
        if result.returncode or not backup.is_file() or not backup.stat().st_size:
            raise RuntimeError('Database backup failed; no migration applied')
        if subprocess.run(['pg_restore', '--list', str(backup)], capture_output=True).returncode:
            raise RuntimeError('Database backup verification failed; no migration applied')
        execute(statement(document, True))
    print('Applied identifier migration.' if args.apply else 'Validated identifier migration; rolled back preview.')

if __name__ == '__main__': main()
