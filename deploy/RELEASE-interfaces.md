# Briefcase interface API update

Backend build 5.0.2 retains API namespace v1 and contract 5.0.0. The 59 existing
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

Completed small uploads and multipart initialization now replay before new-upload
quota and self-destruct checks. Retries retain their original file, timer, version
and usage charge; current permissions and request fingerprints are still checked.
This fixes the conflict found by live verification of a self-destructing upload.

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

## Production result — 2026-11-17

Backend 5.0.2 is live. API and worker run source `71291c8` by immutable image
digest; the website and runtime configuration were preserved. PRs #43 and #44
are merged. All CI jobs passed, including the final main run. The validation
commit `67afa3d` changes only the new test fixture's timestamp precision from
the built source; the production code and documentation are identical.

Authenticated production checks passed for upload and same-key replay,
download integrity and ranges, entry/version/sharing metadata, search, expiring
public links, link removal, cleanup, wrong-audience/refresh-token rejection and
immediate revocation. The separate verification session was revoked; the
shared agent CLI session remains valid.

The [interface guide](https://docs.briefcase.teamofsilicons.com/interfaces/),
Markdown source, `llms.txt`, OpenAPI and reference pages are live over HTTPS.
Silicon Apps now lists the guide as Briefcase's interface API documentation.

Detailed images, source revisions, check results and limitations are recorded in
[the production verification record](verification-interfaces-2026-11-17.json).
