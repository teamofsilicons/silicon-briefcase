# Briefcase interface API update

Backend build 5.0.1 retains API namespace v1 and contract 5.0.0. The 59 existing
operations expose the complete Briefcase frontend, including Organizations
management and dynamic team/organization sharing. No database migration or
CLI/client update is required.

Every authenticated bearer request now performs live Accounts introspection,
requiring an active access token issued to Briefcase for the same authenticated
subject. Refresh tokens and other audiences are rejected. Introspection is
not cached, and JWT expiry/not-before leeway is zero. An unavailable Accounts
service denies access. This replaces the original migration policy that only
introspected sensitive operations and reused answers for 30 seconds.

The [interface guide](../docs/interfaces.md) covers sign-in, personal/work
account discovery, capabilities, every frontend operation, examples, retries,
pagination, errors, limits and update behavior. OpenAPI now describes
organization responses and all supported action bodies, and correctly models
team/organization principals in invitation responses. The static docs also
publish Markdown sources and `llms.txt` for interface creators.

## Release procedure

1. Run the backend suite with an isolated PostgreSQL database, strict Clippy,
   the OpenAPI contract/reference checks and docs build/link checks.
2. Build an immutable ARM64 backend candidate at the reviewed source commit.
   Confirm the production app credentials can introspect a real Briefcase
   access token with `token_type`, `client_id` and the exact `sub` present.
3. Back up the existing API/worker service units. Replace only their image
   references, preserving runtime environment, sessions, mounts and data. No
   migration or web deployment is required. Keep the prior digest for rollback.
4. Check readiness and version, then verify authenticated upload/download,
   metadata, organization context and revoked/wrong-class token rejection with
   the authorized agent. Revoke only the separate verification session.
5. Publish the static docs through a versioned directory and atomic symlink.
   Verify `/interfaces/`, `/interfaces.md`, `/llms.txt` and `/openapi.yaml` over
   HTTPS before setting the app's `interface_api_documentation_url` to
   `https://docs.briefcase.teamofsilicons.com/interfaces/` in Silicon Apps.

The production agent has no organization memberships. Production verification
must not claim work-account admin/member changes were exercised. Those roles,
sharing inheritance and membership removal are covered by the existing local
organization tests. Hosted OAuth completion and notification delivery are
outside this backend/docs change.

Production results will be recorded in `verification-interfaces-2026-11-17.json`.
