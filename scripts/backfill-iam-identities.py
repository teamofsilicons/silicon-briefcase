#!/usr/bin/env python3
"""Validate/import trusted canonical IAM bindings while Briefcase writers are paused.

DATABASE_URL must select production for --production or the shared testing database
for --testing-environment-id. Default rolls back; --apply commits verified bindings.
Identity export contents and connection secrets are never logged.
"""
import argparse
import csv
import io
import json
import os
import subprocess
import uuid
from pathlib import Path
from urllib.parse import urlsplit, unquote, parse_qsl

def mapping_rows(document, environment):
    rows = set()
    for actor in document["testing" if environment else "production"]:
        if actor.get("testing_environment_id") != environment:
            continue
        if actor["kind"] not in ("carbon", "silicon"):
            continue
        rows.add((actor["kind"], actor["public_id"], str(uuid.UUID(actor["legacy_id"]))))
        for member in actor["memberships"]:
            expected = f'{actor["public_id"]}[{member["org_id"]}]'
            if member["membership_public_id"] != expected:
                raise ValueError("inconsistent trusted membership mapping")
            rows.add(("membership", expected, str(uuid.UUID(member["membership_id"]))))
    if not rows:
        raise ValueError("no identities for the explicitly selected environment")
    return sorted(rows)

def database_environment():
    env = dict(os.environ)
    parsed = urlsplit(env["DATABASE_URL"])
    if parsed.scheme not in ("postgres", "postgresql") or not parsed.path.lstrip("/"):
        raise ValueError("DATABASE_URL must be a PostgreSQL connection URL")
    fields = {"PGHOST": parsed.hostname, "PGPORT": str(parsed.port or 5432),
        "PGUSER": unquote(parsed.username or ""), "PGPASSWORD": unquote(parsed.password or ""),
        "PGDATABASE": unquote(parsed.path.lstrip("/"))}
    parameters = {"sslmode": "PGSSLMODE", "sslrootcert": "PGSSLROOTCERT",
        "sslcert": "PGSSLCERT", "sslkey": "PGSSLKEY", "connect_timeout": "PGCONNECT_TIMEOUT",
        "application_name": "PGAPPNAME", "options": "PGOPTIONS", "host": "PGHOST"}
    for name, value in parse_qsl(parsed.query, keep_blank_values=True):
        if name not in parameters:
            raise ValueError("unsupported DATABASE_URL parameter")
        fields[parameters[name]] = value
    for name, value in fields.items():
        if value:
            env[name] = value
        else:
            env.pop(name, None)
    env.pop("PGSERVICE", None)
    env.pop("PGSERVICEFILE", None)
    return env


def statement(rows, environment, apply):
    selected = environment or str(uuid.UUID(int=0))
    data = io.StringIO()
    csv.writer(data, lineterminator="\n").writerows(rows)
    return f'''BEGIN;
SELECT set_config('briefcase.testing_environment_id','{environment or ""}',true);
LOCK TABLE briefcase.iam_identity_bindings IN EXCLUSIVE MODE;
DO $$ BEGIN
 IF NOT EXISTS(SELECT 1 FROM briefcase.iam_identity_backfill WHERE environment_id='{selected}') THEN
 RAISE EXCEPTION 'selected environment has no retained legacy bindings; verify database and environment'; END IF;
END $$;
CREATE TEMP TABLE identity_import(identity_kind text,public_id text,local_id uuid,
 PRIMARY KEY(identity_kind,public_id),UNIQUE(identity_kind,local_id)) ON COMMIT DROP;
COPY identity_import FROM STDIN WITH(FORMAT csv);
{data.getvalue()}\\.
DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM identity_import i JOIN briefcase.iam_identity_bindings b
 ON b.environment_id='{selected}' AND b.identity_kind=i.identity_kind
 AND (b.public_id=i.public_id OR b.local_id=i.local_id)
 WHERE b.public_id<>i.public_id OR b.local_id<>i.local_id) THEN
 RAISE EXCEPTION 'existing identity binding conflicts with trusted export'; END IF;
 IF EXISTS(SELECT 1 FROM briefcase.iam_identity_bindings b WHERE b.environment_id='{selected}'
 AND NOT EXISTS(SELECT 1 FROM identity_import i WHERE i.identity_kind=b.identity_kind
 AND i.public_id=b.public_id AND i.local_id=b.local_id)) THEN
 RAISE EXCEPTION 'retained identity references missing from trusted export'; END IF;
END $$;
INSERT INTO briefcase.iam_identity_bindings
 SELECT '{selected}',identity_kind,public_id,local_id FROM identity_import
 ON CONFLICT(environment_id,identity_kind,public_id) DO NOTHING;
UPDATE briefcase.iam_identity_backfill SET verified=true WHERE environment_id='{selected}';
{'COMMIT' if apply else 'ROLLBACK'};
'''


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mapping",type=Path)
    plane=parser.add_mutually_exclusive_group(required=True)
    plane.add_argument("--production",action="store_true")
    plane.add_argument("--testing-environment-id",type=uuid.UUID)
    parser.add_argument("--apply",action="store_true")
    args=parser.parse_args()
    environment=str(args.testing_environment_id) if args.testing_environment_id else None
    rows=mapping_rows(json.loads(args.mapping.read_text()),environment)
    result=subprocess.run(["psql","-X","--set","ON_ERROR_STOP=1","--quiet"],
        input=statement(rows,environment,args.apply),text=True,env=database_environment(),capture_output=True)
    if result.returncode:
        raise SystemExit("Identity binding validation failed; transaction rolled back. Check selected database/environment, export completeness, and binding conflicts.")
    print(f"{'Applied' if args.apply else 'Validated (rolled back)'} {len(rows)} bindings in {environment or 'production'}.")


if __name__=="__main__":
    main()
