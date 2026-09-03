#!/usr/bin/env bash
# Point the Briefcase host at the load balancer, at Namecheap.
#
# Namecheap's setHosts call replaces every record on the domain, so this reads
# the current records first, changes exactly one, and writes the whole set
# back. It prints what it would send and changes nothing until --apply.
#
#   ./deploy/dns.sh --value dualstack.shared-alb-123.us-east-1.elb.amazonaws.com
#   ./deploy/dns.sh --value <alb-dns-name> --apply
#
# Credentials come from deploy/aws/production.env, or the environment:
#
#   NAMECHEAP_API_USER   Namecheap account the API key belongs to
#   NAMECHEAP_API_KEY    API key from Profile > Tools > Namecheap API Access
#   NAMECHEAP_USERNAME   account that owns the domain (defaults to the API user)
#   NAMECHEAP_DOMAIN     teamofsilicons.com
#
# The calling machine's public address must be on Namecheap's API allow list,
# which is why this prints the address it used when Namecheap refuses.

set -euo pipefail

readonly REPOSITORY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly CONFIG="${BRIEFCASE_DEPLOY_CONFIG:-$REPOSITORY_ROOT/deploy/aws/production.env}"
readonly ENDPOINT="${NAMECHEAP_API_ENDPOINT:-https://api.namecheap.com/xml.response}"

APPLY=false
VALUE=""
RECORD_TYPE="CNAME"
TTL="300"

fail() {
  printf 'dns: %s\n' "$1" >&2
  exit 1
}

usage() {
  sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
  case "$1" in
    --value) VALUE="${2:-}"; shift 2 ;;
    --host) BRIEFCASE_PUBLIC_HOST="${2:-}"; shift 2 ;;
    --type) RECORD_TYPE="${2:-}"; shift 2 ;;
    --ttl) TTL="${2:-}"; shift 2 ;;
    --apply) APPLY=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "unknown option: $1 (try --help)" ;;
  esac
done

[ -f "$CONFIG" ] && { set -a; . "$CONFIG"; set +a; }

command -v python3 >/dev/null 2>&1 || fail 'python3 is required to merge the record set'
[ -n "$VALUE" ] || fail 'pass --value with the load balancer DNS name'

: "${NAMECHEAP_API_USER:?set NAMECHEAP_API_USER}"
: "${NAMECHEAP_API_KEY:?set NAMECHEAP_API_KEY}"
NAMECHEAP_USERNAME="${NAMECHEAP_USERNAME:-$NAMECHEAP_API_USER}"
DOMAIN="${NAMECHEAP_DOMAIN:-teamofsilicons.com}"
HOST_FQDN="${BRIEFCASE_PUBLIC_HOST:-backend.briefcase.$DOMAIN}"

case "$HOST_FQDN" in
  *".$DOMAIN") SUBDOMAIN="${HOST_FQDN%".$DOMAIN"}" ;;
  "$DOMAIN") SUBDOMAIN="@" ;;
  *) fail "$HOST_FQDN is not inside $DOMAIN" ;;
esac
SLD="${DOMAIN%%.*}"
TLD="${DOMAIN#*.}"

# Namecheap authorizes by source address, and its own echo service reports the
# address exactly as Namecheap sees it.
CLIENT_IP="${NAMECHEAP_CLIENT_IP:-$(curl --fail --silent --show-error --max-time 10 \
  https://dynamicdns.park-your-domain.com/getip || true)}"
[ -n "$CLIENT_IP" ] || fail 'could not determine this machine public IP; set NAMECHEAP_CLIENT_IP'

printf 'domain      %s\n' "$DOMAIN"
printf 'record      %s.%s -> %s (%s, ttl %s)\n' "$SUBDOMAIN" "$DOMAIN" "$VALUE" "$RECORD_TYPE" "$TTL"
printf 'calling as  %s from %s\n\n' "$NAMECHEAP_API_USER" "$CLIENT_IP"

current=$(curl --fail --silent --show-error --max-time 30 --get "$ENDPOINT" \
  --data-urlencode "ApiUser=$NAMECHEAP_API_USER" \
  --data-urlencode "ApiKey=$NAMECHEAP_API_KEY" \
  --data-urlencode "UserName=$NAMECHEAP_USERNAME" \
  --data-urlencode "ClientIp=$CLIENT_IP" \
  --data-urlencode "Command=namecheap.domains.dns.getHosts" \
  --data-urlencode "SLD=$SLD" \
  --data-urlencode "TLD=$TLD") || fail 'the getHosts call failed'

# Merge in memory, print the result, and only then decide whether to send it.
# setHosts is a whole-zone replacement: anything dropped here is dropped for
# real, so a zone that comes back empty is treated as a failure, not as a
# zone with nothing in it.
plan=$(HOST="$SUBDOMAIN" VALUE="$VALUE" TYPE="$RECORD_TYPE" TTL="$TTL" python3 - "$current" <<'PY'
import html, os, re, sys

# Namecheap answers with flat, attribute-only elements. Reading them with the
# standard XML parser would be the obvious choice, but it needs expat, which is
# not something a deploying machine can be assumed to have working — and this
# shape does not need a parser to be read correctly.
document = sys.argv[1]

status = re.search(r'<ApiResponse[^>]*\bStatus="([^"]*)"', document)
if not status or status.group(1) != "OK":
    errors = [html.unescape(text) for text in re.findall(r"<Error[^>]*>([^<]*)</Error>", document)]
    print("ERROR " + ("; ".join(e for e in errors if e.strip()) or "Namecheap refused the request"))
    raise SystemExit(0)

def attributes(element):
    return {
        name: html.unescape(value)
        for name, value in re.findall(r'(\w+)="([^"]*)"', element)
    }

hosts = [attributes(element) for element in re.findall(r"<host\b([^>]*?)/?>", document)]
if not hosts:
    print("ERROR the domain reports no existing records; refusing to replace the zone")
    raise SystemExit(0)

wanted_host = os.environ["HOST"]
wanted = {
    "Name": wanted_host,
    "Type": os.environ["TYPE"],
    "Address": os.environ["VALUE"],
    "TTL": os.environ["TTL"],
}

records, replaced = [], False
for host in hosts:
    record = {
        "Name": host.get("Name", ""),
        "Type": host.get("Type", ""),
        "Address": host.get("Address", ""),
        "TTL": host.get("TTL", "1800"),
        "MXPref": host.get("MXPref", "10"),
    }
    if record["Name"] == wanted_host and record["Type"] in {"CNAME", "A", "AAAA", "ALIAS", "URL"}:
        if replaced:
            continue  # one record per name; drop any duplicate of the old shape
        was = f'{record["Type"]} {record["Address"]}'
        record.update(wanted)
        record["MXPref"] = "10"
        replaced = True
        print(f'CHANGE {wanted_host}: {was} -> {wanted["Type"]} {wanted["Address"]}')
    records.append(record)

if not replaced:
    records.append({**wanted, "MXPref": "10"})
    print(f'ADD    {wanted_host}: {wanted["Type"]} {wanted["Address"]}')

for index, record in enumerate(records, start=1):
    print(f'KEEP{index:03d} {record["Name"]:<24} {record["Type"]:<6} {record["Address"]} ttl={record["TTL"]}')

parameters = []
for index, record in enumerate(records, start=1):
    parameters.append(f'HostName{index}={record["Name"]}')
    parameters.append(f'RecordType{index}={record["Type"]}')
    parameters.append(f'Address{index}={record["Address"]}')
    parameters.append(f'TTL{index}={record["TTL"]}')
    if record["Type"] == "MX":
        parameters.append(f'MXPref{index}={record["MXPref"]}')
print("PARAMS " + "\n".join(parameters))
PY
)

if printf '%s' "$plan" | head -1 | grep -q '^ERROR '; then
  fail "$(printf '%s' "$plan" | head -1 | sed 's/^ERROR //')"
fi

printf '%s\n\n' "$(printf '%s' "$plan" | sed '/^PARAMS /,$d')"

if [ "$APPLY" != true ]; then
  printf 'nothing sent. Re-run with --apply to write these records.\n'
  exit 0
fi

# Everything after the PARAMS marker is one name=value per line, sent as form
# fields so no value has to survive shell quoting.
params_file=$(mktemp)
trap 'rm -f "$params_file"' EXIT
printf '%s' "$plan" | sed -n '/^PARAMS /,$p' | sed '1s/^PARAMS //' > "$params_file"

curl_args=(
  --fail --silent --show-error --max-time 60 "$ENDPOINT"
  --data-urlencode "ApiUser=$NAMECHEAP_API_USER"
  --data-urlencode "ApiKey=$NAMECHEAP_API_KEY"
  --data-urlencode "UserName=$NAMECHEAP_USERNAME"
  --data-urlencode "ClientIp=$CLIENT_IP"
  --data-urlencode "Command=namecheap.domains.dns.setHosts"
  --data-urlencode "SLD=$SLD"
  --data-urlencode "TLD=$TLD"
)
while IFS= read -r pair; do
  [ -n "$pair" ] && curl_args+=(--data-urlencode "$pair")
done < "$params_file"

response=$(curl "${curl_args[@]}") || fail 'the setHosts call failed'
printf '%s' "$response" | python3 -c "
import html, re, sys
document = sys.stdin.read()
status = re.search(r'<ApiResponse[^>]*\bStatus=\"([^\"]*)\"', document)
if status and status.group(1) == 'OK':
    print('records updated')
else:
    errors = [html.unescape(text) for text in re.findall(r'<Error[^>]*>([^<]*)</Error>', document)]
    print('Namecheap refused: ' + ('; '.join(e for e in errors if e.strip()) or 'unknown error'))
    raise SystemExit(1)
"

printf '\nDNS changes take a few minutes to propagate. Check with:\n'
printf '  dig +short %s\n' "$HOST_FQDN"
printf '  curl https://%s/api/version\n' "$HOST_FQDN"
