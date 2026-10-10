#!/usr/bin/env bash
# Stage independent Accounts services; never changes legacy units or public ingress.
set -Eeuo pipefail
image=${1:?Pass the digest-pinned Briefcase backend image}
secret_id=${2:-silicon-briefcase/accounts-production}
[[ "$image" =~ ^234951665042\.dkr\.ecr\.us-east-1\.amazonaws\.com/silicon-briefcase-production@sha256:[0-9a-f]{64}$ ]] || exit 2
aws ecr get-login-password --region us-east-1 | docker login --username AWS --password-stdin 234951665042.dkr.ecr.us-east-1.amazonaws.com >/dev/null
docker pull "$image"
umask 077
install -d -m 0700 /etc/silicon-briefcase/accounts
backup=/var/lib/silicon-briefcase/releases/accounts-runtime-$(date -u +%Y%m%dT%H%M%SZ)
install -d -m 0700 "$backup"
cp -a /etc/silicon-briefcase/accounts "$backup/config"
aws secretsmanager get-secret-value --region us-east-1 --secret-id "$secret_id" --query SecretString --output text > /etc/silicon-briefcase/accounts/secret.json
python3 - <<'PY'
import json,os
from pathlib import Path
base=Path('/etc/silicon-briefcase/accounts');p=base/'secret.json'
try: values=json.loads(p.read_text())
finally: p.unlink()
required=['BRIEFCASE_DATABASE_URL','BRIEFCASE_WORKER_DATABASE_URL','BRIEFCASE_APP_SECRET','BRIEFCASE_ACCOUNTS_WEBHOOK_SECRET']
if any(not values.get(k) for k in required):raise SystemExit('Required Accounts runtime secret key is absent')
if values.get('BRIEFCASE_S3_KEY_PREFIX')!='accounts':raise SystemExit('Accounts object prefix is required')
if values.get('BRIEFCASE_BIND_ADDR')!='127.0.0.1:8180':raise SystemExit('Private Accounts API binding is required')
for k in required[:2]:
 from urllib.parse import urlparse
 if urlparse(values[k]).path!='/briefcase_accounts':raise SystemExit('Refusing a legacy database')
worker={k:v for k,v in values.items() if k.startswith(('BRIEFCASE_S3_','BRIEFCASE_WORKER_','BRIEFCASE_DATABASE_','BRIEFCASE_TELEMETRY')) or k in ['AWS_REGION','BRIEFCASE_ENVIRONMENT','BRIEFCASE_TEMPORARY_DIRECTORY','BRIEFCASE_LOG_FILTER','BRIEFCASE_POSTMARK_SERVER_TOKEN','BRIEFCASE_PUBLIC_SITE_BASE_URL']}
worker.pop('BRIEFCASE_DATABASE_URL',None);worker['BRIEFCASE_DATABASE_MAX_CONNECTIONS']='2'
api={k:v for k,v in values.items() if not k.startswith('BRIEFCASE_WORKER_')}
for name,settings in [('api',api),('worker',worker)]:
 if any(any(c in str(v) for c in '\r\n\0') for v in settings.values()):raise SystemExit('Invalid env value')
 p=base/(name+'.env');p.write_text(''.join(k+'='+str(v)+'\n' for k,v in sorted(settings.items())));p.chmod(0o600)
print('Independent Accounts env files prepared; secret values omitted.')
PY
install -d -m 0700 -o 10001 -g 10001 /var/lib/silicon-briefcase/accounts-staging /var/lib/silicon-briefcase/accounts-telemetry-api /var/lib/silicon-briefcase/accounts-telemetry-worker
for component in api worker; do
  memory=256m; [ "$component" = worker ] && memory=128m
  unit=silicon-briefcase-accounts-$component
  if [ -f "/etc/systemd/system/$unit.service" ]; then cp -p "/etc/systemd/system/$unit.service" "$backup/"; fi
  cat > "/etc/systemd/system/$unit.service" <<UNIT
[Unit]
Description=Briefcase Accounts $component (isolated from IAM)
Requires=docker.service
After=docker.service network-online.target
Wants=network-online.target
[Service]
Restart=always
RestartSec=5
ExecStartPre=-/usr/bin/docker rm -f $unit
ExecStart=/usr/bin/docker run --name $unit --network host --read-only --cap-drop ALL --security-opt no-new-privileges --tmpfs /tmp:size=32m,mode=1777 --memory $memory --env-file /etc/silicon-briefcase/accounts/$component.env --mount type=bind,src=/opt/silicon-briefcase/aws-rds-global-bundle.pem,dst=/opt/silicon-briefcase/aws-rds-global-bundle.pem,readonly --mount type=bind,src=/var/lib/silicon-briefcase/accounts-staging,dst=/var/lib/silicon-briefcase/staging --mount type=bind,src=/var/lib/silicon-briefcase/accounts-telemetry-$component,dst=/var/lib/silicon-briefcase/telemetry --log-opt max-size=10m --log-opt max-file=3 $image briefcase-$component
ExecStop=/usr/bin/docker stop -t 30 $unit
TimeoutStopSec=40
[Install]
WantedBy=multi-user.target
UNIT
done
systemctl daemon-reload
systemctl enable --now silicon-briefcase-accounts-api silicon-briefcase-accounts-worker
for attempt in {1..30}; do
  if curl --fail --silent --output /dev/null http://127.0.0.1:8180/readyz; then
    echo "Accounts API ready privately on 8180; legacy services and public ingress unchanged. Backup: $backup"
    exit 0
  fi
  sleep 1
done
echo 'Accounts readiness failed; inspect only the new units. Legacy services remain running.' >&2
exit 1
