#!/usr/bin/env bash
# Ship Silicon Briefcase to AWS.
#
# The image is immutable and tagged with the commit it was built from, the
# stack carries that exact tag, and replacing the instance is what puts a new
# build in service. Nothing here reads a secret: the instance fetches those
# from Secrets Manager itself.
#
#   ./deploy/deploy.sh              build, push, update the stack, replace the instance
#   ./deploy/deploy.sh image        build and push only
#   ./deploy/deploy.sh stack        update the stack with the current image tag
#   ./deploy/deploy.sh refresh      replace the running instance with the current template
#   ./deploy/deploy.sh status       what is deployed, and whether it is healthy
#   ./deploy/deploy.sh plan         show what a stack update would change, and stop
#
# Configuration comes from deploy/aws/production.env — copy the example beside
# it and fill in the shared network and listener identifiers.

set -euo pipefail

readonly REPOSITORY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly STACK_NAME="${BRIEFCASE_STACK_NAME:-silicon-briefcase-production}"
readonly ECR_REPOSITORY="${BRIEFCASE_ECR_REPOSITORY:-silicon-briefcase-production}"
readonly TEMPLATE="$REPOSITORY_ROOT/deploy/aws/production.yaml"
readonly CONFIG="${BRIEFCASE_DEPLOY_CONFIG:-$REPOSITORY_ROOT/deploy/aws/production.env}"

fail() {
  printf 'deploy: %s\n' "$1" >&2
  exit 1
}

step() {
  printf '\n\033[1m==> %s\033[0m\n' "$1"
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "$1 is required but not installed"
}

load_configuration() {
  [ -f "$CONFIG" ] || fail "no configuration at $CONFIG (copy production.env.example and fill it in)"
  # shellcheck disable=SC1090
  set -a && . "$CONFIG" && set +a

  : "${AWS_REGION:?set AWS_REGION in $CONFIG}"
  : "${BRIEFCASE_VPC_ID:?set BRIEFCASE_VPC_ID in $CONFIG}"
  : "${BRIEFCASE_PRIVATE_SUBNET_A:?set BRIEFCASE_PRIVATE_SUBNET_A in $CONFIG}"
  : "${BRIEFCASE_PRIVATE_SUBNET_B:?set BRIEFCASE_PRIVATE_SUBNET_B in $CONFIG}"
  : "${BRIEFCASE_ALB_SECURITY_GROUP_ID:?set BRIEFCASE_ALB_SECURITY_GROUP_ID in $CONFIG}"
  : "${BRIEFCASE_HTTPS_LISTENER_ARN:?set BRIEFCASE_HTTPS_LISTENER_ARN in $CONFIG}"
  : "${BRIEFCASE_CERTIFICATE_ARN:?set BRIEFCASE_CERTIFICATE_ARN in $CONFIG}"
  : "${BRIEFCASE_APP_SECRET_ARN:?set BRIEFCASE_APP_SECRET_ARN in $CONFIG}"
  PUBLIC_HOST="${BRIEFCASE_PUBLIC_HOST:-backend.briefcase.teamofsilicons.com}"
  LISTENER_RULE_PRIORITY="${BRIEFCASE_LISTENER_RULE_PRIORITY:-30}"
}

account_id() {
  aws sts get-caller-identity --query Account --output text
}

# The tag names the exact commit, and a dirty tree is refused: an image nobody
# can rebuild from source is not something to put in production.
image_tag() {
  local revision
  revision=$(git -C "$REPOSITORY_ROOT" rev-parse --short=12 HEAD)
  if [ -n "$(git -C "$REPOSITORY_ROOT" status --porcelain)" ]; then
    if [ "${BRIEFCASE_ALLOW_DIRTY:-}" = "true" ]; then
      printf '%s-dirty' "$revision"
      return
    fi
    fail 'the working tree has uncommitted changes; commit them or set BRIEFCASE_ALLOW_DIRTY=true'
  fi
  printf '%s' "$revision"
}

image_uri() {
  printf '%s.dkr.ecr.%s.amazonaws.com/%s:%s' "$(account_id)" "$AWS_REGION" "$ECR_REPOSITORY" "$(image_tag)"
}

ensure_repository() {
  if ! aws ecr describe-repositories --region "$AWS_REGION" \
      --repository-names "$ECR_REPOSITORY" >/dev/null 2>&1; then
    step "Creating ECR repository $ECR_REPOSITORY"
    aws ecr create-repository --region "$AWS_REGION" \
      --repository-name "$ECR_REPOSITORY" \
      --image-tag-mutability IMMUTABLE \
      --image-scanning-configuration scanOnPush=true \
      --encryption-configuration encryptionType=AES256 >/dev/null
  fi
}

build_and_push() {
  require_command docker
  # Fail on the local problem before spending a round trip on AWS.
  image_tag >/dev/null
  ensure_repository
  local uri registry
  uri=$(image_uri)
  registry="${uri%%/*}"

  if aws ecr describe-images --region "$AWS_REGION" --repository-name "$ECR_REPOSITORY" \
      --image-ids imageTag="$(image_tag)" >/dev/null 2>&1; then
    step "Image $(image_tag) is already in ECR; reusing it"
    return
  fi

  step "Building $uri for linux/arm64"
  # The instances are Graviton, so the image is built for their architecture
  # rather than the laptop's.
  docker buildx build \
    --platform linux/arm64 \
    --tag "$uri" \
    --file "$REPOSITORY_ROOT/Dockerfile" \
    --provenance false \
    --load \
    "$REPOSITORY_ROOT"

  step "Pushing to ECR"
  aws ecr get-login-password --region "$AWS_REGION" \
    | docker login --username AWS --password-stdin "$registry"
  docker push "$uri"
}

deploy_stack() {
  step "Deploying stack $STACK_NAME"
  local parameters=(
    "VpcId=$BRIEFCASE_VPC_ID"
    "PrivateSubnetA=$BRIEFCASE_PRIVATE_SUBNET_A"
    "PrivateSubnetB=$BRIEFCASE_PRIVATE_SUBNET_B"
    "AlbSecurityGroupId=$BRIEFCASE_ALB_SECURITY_GROUP_ID"
    "HttpsListenerArn=$BRIEFCASE_HTTPS_LISTENER_ARN"
    "CertificateArn=$BRIEFCASE_CERTIFICATE_ARN"
    "AppSecretArn=$BRIEFCASE_APP_SECRET_ARN"
    "BackendImageUri=$(image_uri)"
    "PublicHostName=$PUBLIC_HOST"
    "ListenerRulePriority=$LISTENER_RULE_PRIORITY"
  )
  [ -n "${BRIEFCASE_PUBLIC_SITE_BASE_URL:-}" ] &&
    parameters+=("PublicSiteBaseUrl=$BRIEFCASE_PUBLIC_SITE_BASE_URL")
  [ -n "${BRIEFCASE_IAM_BASE_URL:-}" ] &&
    parameters+=("IamBaseUrl=$BRIEFCASE_IAM_BASE_URL")

  aws cloudformation deploy \
    --region "$AWS_REGION" \
    --stack-name "$STACK_NAME" \
    --template-file "$TEMPLATE" \
    --capabilities CAPABILITY_NAMED_IAM \
    --parameter-overrides "${parameters[@]}" \
    --tags Service=silicon-briefcase Environment=production \
    ${1+"$@"}
}

# The image tag is baked into the launch template's user data, so a new build
# reaches production by replacing the instance rather than by restarting it.
refresh_instances() {
  local asg
  asg=$(stack_output AutoScalingGroupName)
  [ -n "$asg" ] || fail 'the stack has no auto scaling group yet'

  step "Replacing the instance in $asg"
  local refresh_id
  refresh_id=$(aws autoscaling start-instance-refresh \
    --region "$AWS_REGION" \
    --auto-scaling-group-name "$asg" \
    --preferences '{"MinHealthyPercentage":0,"InstanceWarmup":600,"SkipMatching":false}' \
    --query InstanceRefreshId --output text)
  printf 'instance refresh %s started\n' "$refresh_id"

  while true; do
    local status
    status=$(aws autoscaling describe-instance-refreshes \
      --region "$AWS_REGION" \
      --auto-scaling-group-name "$asg" \
      --instance-refresh-ids "$refresh_id" \
      --query 'InstanceRefreshes[0].Status' --output text)
    case "$status" in
      Successful) printf 'instance refresh complete\n'; return 0 ;;
      Failed|Cancelled) fail "instance refresh $status; check the instance's cloud-init log over SSM" ;;
      *) printf '  %s...\n' "$status"; sleep 20 ;;
    esac
  done
}

stack_output() {
  aws cloudformation describe-stacks \
    --region "$AWS_REGION" \
    --stack-name "$STACK_NAME" \
    --query "Stacks[0].Outputs[?OutputKey=='$1'].OutputValue" \
    --output text 2>/dev/null || true
}

show_status() {
  step "Stack"
  aws cloudformation describe-stacks --region "$AWS_REGION" --stack-name "$STACK_NAME" \
    --query 'Stacks[0].{Status:StackStatus,Updated:LastUpdatedTime}' --output table 2>/dev/null \
    || { printf 'stack %s does not exist yet\n' "$STACK_NAME"; return; }

  step "Deployed image"
  aws cloudformation describe-stacks --region "$AWS_REGION" --stack-name "$STACK_NAME" \
    --query "Stacks[0].Parameters[?ParameterKey=='BackendImageUri'].ParameterValue" --output text

  step "Target health"
  local target_group
  target_group=$(stack_output ApiTargetGroupArn)
  if [ -n "$target_group" ]; then
    aws elbv2 describe-target-health --region "$AWS_REGION" --target-group-arn "$target_group" \
      --query 'TargetHealthDescriptions[].{Target:Target.Id,State:TargetHealth.State,Reason:TargetHealth.Description}' \
      --output table
  fi

  step "Public endpoint"
  local host
  host=$(stack_output PublicHostName)
  host="${host:-$PUBLIC_HOST}"
  printf 'https://%s/api/version\n' "$host"
  curl --silent --show-error --max-time 10 "https://$host/api/version" \
    | head -c 200 || printf '(not reachable — is DNS pointed at the load balancer?)\n'
  printf '\n'
}

print_dns_instructions() {
  local listener_arn alb_arn alb_dns host
  host=$(stack_output PublicHostName)
  host="${host:-$PUBLIC_HOST}"
  listener_arn="$BRIEFCASE_HTTPS_LISTENER_ARN"
  alb_arn=$(aws elbv2 describe-listeners --region "$AWS_REGION" --listener-arns "$listener_arn" \
    --query 'Listeners[0].LoadBalancerArn' --output text)
  alb_dns=$(aws elbv2 describe-load-balancers --region "$AWS_REGION" --load-balancer-arns "$alb_arn" \
    --query 'LoadBalancers[0].DNSName' --output text)

  step "DNS"
  printf 'Point %s at the shared load balancer:\n\n' "$host"
  printf '  type   CNAME\n  host   %s\n  value  %s\n  ttl    300\n\n' "${host%%.*}" "$alb_dns"
  printf 'At Namecheap, either add that record by hand or run:\n\n'
  printf '  ./deploy/dns.sh --value %s\n\n' "$alb_dns"
}

main() {
  require_command aws
  require_command git
  load_configuration

  case "${1:-all}" in
    image)
      build_and_push
      ;;
    stack)
      deploy_stack
      print_dns_instructions
      ;;
    plan)
      deploy_stack --no-execute-changeset
      ;;
    refresh)
      refresh_instances
      ;;
    status)
      show_status
      ;;
    dns)
      print_dns_instructions
      ;;
    all)
      build_and_push
      deploy_stack
      refresh_instances
      print_dns_instructions
      show_status
      ;;
    -h|--help|help)
      sed -n '2,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      ;;
    *)
      fail "unknown command: $1 (try --help)"
      ;;
  esac
}

main "$@"
