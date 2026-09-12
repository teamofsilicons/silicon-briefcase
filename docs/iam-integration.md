# IAM integration and production webhook approval

This is an operator/integrator guide, not an extra public Briefcase API.
Application clients must not receive Briefcase's server-side secrets or expose
IAM platform-admin operations through the Briefcase package or CLI.

## Official client and compatibility

The [delegated-upload protocol](api/delegated-uploads.md) documents endpoint
registration, exact manifests, narrow staging capabilities and fresh commit
proofs. It never turns an IAM authorization snapshot into a reusable grant.

The backend imports registry `silicon-iam-client = "=1.7.0"`. Its typed methods
own all IAM network calls, API-version negotiation, redirects, and transport.
Runtime dependency auto-updates are disabled for the backend: upgrading the
dependency requires a deliberate build and deployment. This is distinct from
the default-on client/CLI updater behavior described in their own guides.

IAM must provide the current authorization snapshot contract (backend
migration 0067 and testing migration 9003). Briefcase cross-checks identity,
organization, membership, role/tag disclosure, authorization epoch, audience,
and testing environment; incomplete or conflicting facts fail closed. Older
identity-only responses cannot be treated as complete authorization.

IAM's [user-selected organization consent](https://github.com/teamofsilicons/silicon-iam/blob/main/docs/ORGANIZATION_CONSENT.md)
requires migrations 0072–0074. Briefcase starts login with only `app_id` and
`redirect_uri`; IAM owns the user's organization choices. The server exchanges
the SLT and reads selected active authorization snapshots through the official
client. It never calls IAM's direct-user consent endpoints or infers grants from
membership, a file URL, `X-Org-ID`, or cached organization IDs. Existing legacy
sessions with no explicit grants must revisit IAM. Selected-grant additions are
additive within the parent IAM session; future memberships are not automatic.

The IAM base is `https://backend.iam.teamofsilicons.com/`, not its `/api/v1/`
subpath. The Briefcase SDK base, in contrast, includes `/api/v1/`.

## Temporary IAM failures

Set `BRIEFCASE_IAM_REQUEST_TIMEOUT_MS=4000` (also the configuration default).
The deployment template previously overrode this to 2000 ms. The normal
Briefcase request deadline remains 15 seconds.

Read-only token introspection makes at most two attempts within one nine-second
budget. Timeouts, connection failures, and HTTP 502–504 can retry after 200–455 ms
of jitter. A rate-limited response honors Retry-After; if that delay cannot fit
within the budget, the failure is returned without an early retry. Invalid
credentials, inactive tokens, malformed replies and authorization-binding failures
never become successful authorizations. Every attempt uses the same token, app
credential, organization, and testing-environment selection. No stale authorization
is used as a fallback. Token exchanges, refreshes, and single-use OBO proofs are
not retried by this policy.

Structured logs preserve `iam.failure`, upstream HTTP status and validated UUID
request IDs; introspection logs also record attempt, elapsed milliseconds, and
whether a retry is planned. Raw client errors, URLs, credentials, response bodies,
and arbitrary upstream messages are excluded. A `dependency_unavailable` response
means online verification failed, not that a recipient lacks file access.

Reproduction and regression checks:

```sh
cargo test --locked --lib infrastructure::iam -- --nocapture
```

The controlled slow-IAM test reproduces a 503 with a two-second timeout and a
2.5-second response, then verifies recovery with the four-second setting. Other
checks cover transient retry recovery, preserved test scope, two-attempt limits,
rate-limit delays, total-budget exhaustion, denied/inactive authority, and log
redaction. This reproduces the failure mechanism; it does not establish what
caused the original live IAM delay.

## IDs and credentials

| Value | Meaning | Who keeps it |
| --- | --- | --- |
| `tos>briefcase` | Canonical public production Application ID | Public configuration |
| `01a070db-89b4-7542-83f1-4fad5cbce625` | Internal Application UUID; resource for admin step-up | Non-secret operator metadata |
| Application secret | Backend authentication to IAM | AWS Secrets Manager; never client responses |
| Webhook signing secret and key version | Verify IAM's exact signed body | IAM and Briefcase backend secret stores |
| Test Application secret | Fresh credential for the imported Application in one IAM test plane | Encrypted Briefcase pairing; never substitute production secret |
| IAM test root | Select outbound IAM test plane and match signed test webhooks | Encrypted pairing |
| Briefcase test selector | The paired IAM test Application secret; selects the sandbox and key-authorized self-service | Authorized operator/client secret storage |

Do not log credentials, persist raw webhook envelopes, copy secrets into docs,
or place them in build context. The provisioned production secret is named
`silicon-briefcase/production`. Local backups are outside the repository with
owner-only permissions. Keep the sandbox encryption key stable across normal
deployments: replacing it without migrating encrypted rows loses access to
stored pairing credentials.

## Who can approve a webhook

IAM's dedicated webhook-approval operation accepts a direct Carbon session
belonging to the Application's current owning-organization owner/admin, or an
IAM reviewer with `applications.review`. The owning owner/admin does **not**
need that platform capability. An Application secret, Application-bound member
token or AWS access is not an approval credential.

This narrow operation activates a verified Application's pending webhook
destination only. It does not approve additional scopes or change the
Application's review status. General platform Application review remains a
different operation; do not route webhook-only approval through it or grant
platform authority just to complete setup.

The official IAM CLI provides `app approve-webhook`. IAM implements the
dedicated route in
[`webhooks.rs::approve`](https://github.com/teamofsilicons/silicon-iam/blob/main/src/features/applications/webhooks.rs)
with current authority, verified-channel step-up and version/idempotency checks.

## Approval procedure

1. Inspect `iam --url https://backend.iam.teamofsilicons.com -o json app webhook 'tos>briefcase'`.
   The pending URL must be exactly `https://backend.briefcase.teamofsilicons.com/webhook/`.
2. Use a direct Carbon session for the owning organization's current owner or
   admin, or a current IAM `applications.review` reviewer. Do not use the
   Briefcase member token or server-held Application secret.
3. Read the current Application version/ETag. Do not reuse a version copied
   from this dated document or another mutation.
4. Obtain verified-channel step-up for `application.webhook.approve`,
   resource UUID `01a070db-89b4-7542-83f1-4fad5cbce625`, in the same session:

   ```bash
   iam --url https://backend.iam.teamofsilicons.com step-up \
     application.webhook.approve 01a070db-89b4-7542-83f1-4fad5cbce625
   ```

   Complete the code prompt through the user's verified channel. Treat the
   returned assertion as a short-lived credential; do not paste it into logs.
5. Run the official IAM CLI's `app approve-webhook 'tos>briefcase'` with the
   fresh assertion supplied through its global `--step-up` option. Keep the
   same session, organization, service URL and production/test context as the
   inspection. Avoid putting the literal assertion into shell history.

   The equivalent HTTP request is
   `POST /api/v1/applications/{app_id}/webhook/approvals`, with no request body,
   the direct Carbon bearer, current `If-Match`, a persisted `Idempotency-Key`
   and `X-Step-Up-Token`. URL-encode the public app ID. A raw HTTP caller must
   preserve the exact version and key when reconciling an uncertain response;
   a new pending replacement must not reuse the earlier approval intent.
6. Read the webhook again. Success means `status=active`, the expected
   `active_url`, and no pending replacement. Then cause a deliberate event in
   an isolated test environment and confirm signed delivery and reconciliation.

The command is `app approve-webhook`, not `app approve`. Test-environment
webhooks activate immediately and normally have no pending destination to
approve. These are IAM operator actions, outside the public Briefcase SDK/CLI.

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

The caller-facing companion to this section is the [OBO guide](obo.md).

Register these fixed paths in the Briefcase Application's IAM endpoint catalog.
Every operation uses `POST`; endpoint IDs must not be repointed to other paths.

| Endpoint ID | Registered path | IAM metadata schema |
| --- | --- | --- |
| `briefcase.files.create` | `/api/v1/obo/files` | Required strings `path`, `name`, `content_type` |
| `briefcase.folders.create` | `/api/v1/obo/folders/create` | Empty object `{}` |
| `briefcase.entries.list` | `/api/v1/obo/entries/list` | Empty object `{}` |
| `briefcase.files.read` | `/api/v1/obo/files/read` | Empty object `{}` |
| `briefcase.entries.trash` | `/api/v1/obo/entries/trash` | Empty object `{}` |
| `briefcase.uploads.reserve` | `/api/v1/obo/uploads/reserve` | Empty object `{}` |
| `briefcase.uploads.commit` | `/api/v1/obo/uploads/commit` | Empty object `{}` |
| `briefcase.uploads.status` | `/api/v1/obo/uploads/status` | Empty object `{}` |
| `briefcase.uploads.cancel` | `/api/v1/obo/uploads/cancel` | Empty object `{}` |

The one-shot file endpoint keeps its raw-byte body and metadata contract.
The other operations put all inputs in exact JSON body bytes, including range,
disposition, pagination and logical mutation UUIDs. The issuer and audience
must meet IAM's same-organization and authorization rules. Proofs bind exact
body bytes by SHA-256, method, endpoint, audience, actor, and environment; they
are single-use and must not be blindly retried. Keep the same logical mutation
UUID when recovering an uncertain JSON mutation, but obtain a fresh proof.
Briefcase always rechecks current authority and its ordinary resource policy.

The raw `PUT /api/v1/obo/uploads/{upload_id}/content` is not an IAM catalog
endpoint. Its narrow capability permits private staging only; a fresh commit
proof separately authorizes publication. See [delegated uploads](api/delegated-uploads.md).

Webhook approval and OBO catalog registration are separate operations.
Confirm the Briefcase Application's required scope disclosure (`profile`,
`self.organizations.read`, `self.identity.read`, `self.membership.read`) and catalog registration
before making OBO calls. The issuing member's Application token needs
`obo.issue`; `self.identity.read` and `self.membership.read` must also be present in both
that token and the recipient's approved scopes. The [API](obo.md#choose-an-operation)
documents the exact request bodies and recovery behavior. Proofs remain
dependent on current initiator authorization; storing a verified snapshot does
not create permission for later requests.

## First official release requirements

Use IAM client 1.7.0 and the corresponding deployed IAM contract. Subject
snapshots require `self.identity.read`, `self.membership.read`, and
`self.tags.read`. Use `self.organizations.read` for organization selection.
Recipient/tag discovery uses `directory.carbons.read`, `directory.silicons.read`,
`directory.memberships.read`, and `directory.tags.read`. Scope-projected
responses must not be parsed as complete admin-directory models.

Optional `self.email.read` lets Briefcase learn the signed-in subject's verified
contact for invitation mail. IAM does not reveal other members' emails; see
[Sharing](sharing.md) for the resulting recipient-resolution limits.

Register `briefcase.invitations.create` (`POST /api/v1/obo/invitations`) and
`briefcase.link_access.update` (`POST /api/v1/obo/link-access`) as critical,
user-approved endpoints with empty metadata schemas. All other existing file
CRUD endpoints remain noncritical. Every OBO path stays inside the calling
app's namespace and the represented actor's permissions. See [OBO](obo.md).

Testing callers now pass only the paired IAM test app secret. The backend
retains the IAM environment key needed by the SDK's `application.testing-context`
verification. Production and testing Application credentials are never
interchangeable. Rotate in IAM and replace the Briefcase pairing together.
