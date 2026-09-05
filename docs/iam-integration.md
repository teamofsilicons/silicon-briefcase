# IAM integration and production webhook approval

This is an operator/integrator guide, not an extra public Briefcase API.
Application clients must not receive Briefcase's server-side secrets or expose
IAM platform-admin operations through the Briefcase package or CLI.

## Official client and compatibility

The backend imports registry `silicon-iam-client = "=1.2.0"`. Its typed methods
own all IAM network calls, API-version negotiation, redirects, and transport.
Runtime dependency auto-updates are disabled for the backend: upgrading the
dependency requires a deliberate build and deployment. This is distinct from
the default-on client/CLI updater behavior described in their own guides.

IAM must provide the current authorization snapshot contract (backend
migration 0067 and testing migration 9003). Briefcase cross-checks identity,
organization, membership, role/tag disclosure, authorization epoch, audience,
and testing environment; incomplete or conflicting facts fail closed. Older
identity-only responses cannot be treated as complete authorization.

The IAM base is `https://backend.iam.teamofsilicons.com/`, not its `/api/v1/`
subpath. The Briefcase SDK base, in contrast, includes `/api/v1/`.

## IDs and credentials

| Value | Meaning | Who keeps it |
| --- | --- | --- |
| `tos>briefcase` | Canonical public production Application ID | Public configuration |
| `01a070db-89b4-7542-83f1-4fad5cbce625` | Internal Application UUID; resource for admin step-up | Non-secret operator metadata |
| Application secret | Backend authentication to IAM | AWS Secrets Manager; never client responses |
| Webhook signing secret and key version | Verify IAM's exact signed body | IAM and Briefcase backend secret stores |
| Test Application secret | Fresh credential for the imported Application in one IAM test plane | Encrypted Briefcase pairing; never substitute production secret |
| IAM test root | Select outbound IAM test plane and match signed test webhooks | Encrypted pairing |
| Briefcase test root | Select the Briefcase sandbox; key-authorized self-service | Authorized operator/client secret storage |

Do not log credentials, persist raw webhook envelopes, copy secrets into docs,
or place them in build context. The provisioned production secret is named
`silicon-briefcase/production`. Local backups are outside the repository with
owner-only permissions. Keep the sandbox encryption key stable across normal
deployments: replacing it without migrating encrypted rows loses access to
stored pairing credentials.

## Why `applications.review` is required

IAM separates an organization owner's/admin's ability to register or propose
an Application webhook from a platform administrator's ability to activate a
production destination. The decision endpoint checks the **current Carbon's**
platform capability `applications.review`; the Application's own secret and an
organization-admin membership do not supply it.

This is enforced by IAM, not a new permission requested by Briefcase. Having
AWS access also does not make an IAM Carbon a platform administrator. Do not
bypass the decision API with database writes or grant platform authority just
to make an integration test pass. Changing this policy belongs to a separate
IAM product/security change.

Source: IAM's local `src/features/applications/applications.rs::admin_decide`
and its [platform review documentation](https://github.com/teamofsilicons/silicon-iam/blob/main/docs/API_DOCS.md#platform-application-review).

## Approval procedure

1. Inspect `iam --url https://backend.iam.teamofsilicons.com -o json app webhook 'tos>briefcase'`.
   The pending URL must be exactly `https://backend.briefcase.teamofsilicons.com/webhook/`.
2. Use an existing platform-admin Carbon session with `applications.review`.
   `GET /api/v1/admin/applications` is a read-only authority/inventory check.
   If it returns 403, stop; an app secret cannot solve that failure.
3. Read the current Application version/ETag. Do not reuse a version copied
   from this dated document or another mutation.
4. Obtain verified-channel step-up for `platform_admin.application_review`,
   resource UUID `01a070db-89b4-7542-83f1-4fad5cbce625`, in the same session:

   ```bash
   iam --url https://backend.iam.teamofsilicons.com step-up \
     platform_admin.application_review 01a070db-89b4-7542-83f1-4fad5cbce625
   ```

   Complete the code prompt through the user's verified channel. Treat the
   returned assertion as a short-lived credential; do not paste it into logs.
5. An authorized operator calls `POST /api/v1/admin/applications/{app_id}/decisions`
   with `Authorization: Bearer <admin-access-token>`, current `If-Match`, a
   persisted `Idempotency-Key`, `Content-Type: application/json`, and the
   assertion in `X-Step-Up-Token`. Use decision `approve_pending_changes` and a specific
   audit reason. Preserve the existing scopes; this task approves the webhook,
   not an unrelated policy change. Encode the public app ID in the URL.
6. Read the webhook again. Success means `status=active`, the expected
   `active_url`, and no pending replacement. Then cause a deliberate event in
   an isolated test environment and confirm signed delivery and reconciliation.

The current IAM CLI has no `app approve` command; do not invent one. It does
expose the step-up command. The privileged decision itself uses IAM's admin
HTTP workflow, outside the public Briefcase SDK/CLI.

## Receiving signed events

The receiver is `POST /webhook/` at the backend host root. IAM sends event ID,
timestamp, key version and HMAC signature headers. Briefcase verifies the exact
raw bytes and replay window before applying projections, deduplicates events,
and prevents older resource versions from rolling back newer state. During
rotation, retain prior verification keys long enough for in-flight deliveries.

The same endpoint accepts signed IAM test wrappers. The authenticated IAM test
root selects the paired Briefcase environment; it is not the Briefcase root.
Unknown, inactive, retired, or mismatched planes cannot fall back to production.
See the [API signature contract](api/README.md) and [test-plane guide](testing-environments.md).

An unsigned POST returning 401 verifies rejection only. Healthy `/readyz` proves
database readiness only. Neither is evidence that IAM has approved, scheduled,
delivered, or successfully replayed a webhook.

## OBO registration is separate

Register `briefcase.files.create`, method `POST`, path `/api/v1/obo/files`, with
metadata keys `path`, `name`, and `content_type`. The issuer and audience must
meet IAM's same-organization and authorization rules. Proofs bind exact body
bytes by SHA-256, method, endpoint, audience, actor, and environment; they are
single-use and must not be blindly retried.

Webhook approval and OBO catalog registration are separate operations.
Confirm the required scope disclosure (`profile`,
`organizations.read`, `memberships.read`, `roles.read`) and catalog registration
before making OBO calls. The [API](api/README.md) and
[Rust guide](client/README.md) document the file-creation request itself.
