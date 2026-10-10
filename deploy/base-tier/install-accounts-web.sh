#!/usr/bin/env bash
# Stage the independent Accounts web. This does not change public ingress.
# Keep the IAM web on 4318 for legacy links; switch Nginx only after coordinated GO.
set -Eeuo pipefail
image=${1:?Pass the digest-pinned Briefcase web image}
secret_id=${2:-silicon-briefcase/accounts-production}
if [[ ! "$image" =~ ^234951665042\.dkr\.ecr\.us-east-1\.amazonaws\.com/silicon-briefcase-production@sha256:[0-9a-f]{64}$ ]]; then
  echo 'Expected a digest-pinned Briefcase ECR image' >&2
  exit 2
fi
aws ecr get-login-password --region us-east-1 | docker login --username AWS --password-stdin 234951665042.dkr.ecr.us-east-1.amazonaws.com >/dev/null
docker pull "$image"
umask 077
backup=/var/lib/silicon-briefcase/releases/accounts-web-$(date -u +%Y%m%dT%H%M%SZ)
install -d -m 0700 "$backup" /etc/silicon-briefcase /etc/systemd/system/silicon-briefcase-accounts-web.service.d
if [ -f /etc/silicon-briefcase/accounts-web.env ]; then cp -p /etc/silicon-briefcase/accounts-web.env "$backup/web.env"; fi
if [ -f /etc/systemd/system/silicon-briefcase-accounts-web.service ]; then cp -p /etc/systemd/system/silicon-briefcase-accounts-web.service "$backup/unit"; fi
aws secretsmanager get-secret-value --region us-east-1 --secret-id "$secret_id" --query SecretString --output text > "$backup/runtime-secret.json"
python3 - "$backup/runtime-secret.json" <<'PY'
import json, os, secrets, sys
from pathlib import Path
source_path = Path(sys.argv[1])
try:
    source = json.loads(source_path.read_text())
finally:
    source_path.unlink()
secret = source.get('BRIEFCASE_APP_SECRET')
if not isinstance(secret, str) or not secret or any(c in secret for c in '\r\n\0'):
    raise SystemExit('The runtime secret needs BRIEFCASE_APP_SECRET before installing the Accounts web.')
path = Path('/etc/silicon-briefcase/accounts-web.env')
old = dict(line.split('=', 1) for line in path.read_text().splitlines() if '=' in line and not line.startswith('#')) if path.exists() else {}
values = {
    'APP_ID': 'briefcase',
    'APP_SECRET': secret,
    'ACCOUNTS_URL': source.get('ACCOUNTS_URL') or 'https://accounts.teamofsilicons.com',
    'APP_API_URL': 'http://127.0.0.1:8180/api',
    'PUBLIC_URL': 'https://briefcase.teamofsilicons.com',
    'SESSION_SECRET': old.get('SESSION_SECRET') or secrets.token_urlsafe(48),
    'PORT': '4319',
    'HOSTNAME': '127.0.0.1',
    'NODE_ENV': 'production',
}
for key in ['EXTRA_IMG_ORIGINS', 'EXTRA_ORIGINS']:
    if old.get(key):
        values[key] = old[key]
if any(any(c in str(value) for c in '\r\n\0') for value in values.values()):
    raise SystemExit('A web setting contains an invalid line break.')
path.write_text(''.join(f'{key}={value}\n' for key, value in sorted(values.items())))
os.chmod(path, 0o600)
print('Prepared Accounts web configuration; credential values omitted.')
PY
cat > /etc/systemd/system/silicon-briefcase-accounts-web.service <<UNIT
[Unit]
Description=Briefcase Accounts website
Requires=docker.service
After=docker.service network-online.target
Wants=network-online.target
[Service]
Restart=always
RestartSec=5
ExecStartPre=-/usr/bin/docker rm -f silicon-briefcase-accounts-web
ExecStart=/usr/bin/docker run --name silicon-briefcase-accounts-web --network host --read-only --cap-drop ALL --security-opt no-new-privileges --tmpfs /tmp:size=32m,mode=1777 --tmpfs /app/.next/cache:size=64m,mode=1777 --memory 384m --env-file /etc/silicon-briefcase/accounts-web.env --log-driver json-file --log-opt max-size=10m --log-opt max-file=3 $image
ExecStop=/usr/bin/docker stop -t 20 silicon-briefcase-accounts-web
TimeoutStopSec=30
[Install]
WantedBy=multi-user.target
UNIT
rollback() {
  systemctl stop silicon-briefcase-accounts-web || true
  if [ -f "$backup/web.env" ]; then cp -p "$backup/web.env" /etc/silicon-briefcase/accounts-web.env; fi
  if [ -f "$backup/unit" ]; then
    cp -p "$backup/unit" /etc/systemd/system/silicon-briefcase-accounts-web.service
    systemctl daemon-reload
    systemctl restart silicon-briefcase-accounts-web
  fi
  echo "Accounts web startup failed; legacy web and ingress were untouched. Backup: $backup" >&2
}
systemctl daemon-reload
systemctl enable silicon-briefcase-accounts-web
if ! systemctl restart silicon-briefcase-accounts-web; then rollback; exit 1; fi
for attempt in {1..30}; do
  if curl --fail --silent --output /dev/null http://127.0.0.1:4319/; then
    echo "Accounts web ready privately on 4319; legacy web remains on 4318. Backup: $backup"
    exit 0
  fi
  sleep 1
done
rollback
exit 1
