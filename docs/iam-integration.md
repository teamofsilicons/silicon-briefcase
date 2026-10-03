# IAM integration and production webhook approval

This is an operator/integrator guide, not an extra public Briefcase API.
Application clients must not receive Briefcase's server-side secrets or expose
IAM platform-admin operations through the Briefcase package or CLI.

## Official client and compatibility

The [delegated-upload protocol](api/delegated-uploads.md) documents endpoint
registration, exact manifests, narrow staging capabilities and fresh commit
proofs. It never turns an IAM authorization snapshot into a reusable grant.

The backend imports registry `silicon-iam-client = "=1.8.0"`. Its typed methods
own all IAM network calls, API-version negotiation, redirects, and transport.
Runtime dependency auto-updates are disabled for the backend: upgrading the
dependency requires a deliberate build and deployment. The Briefcase Rust
client also requires explicit dependency updates; Honeycomb manages CLI updates.

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
is used as a fallback. Token exchanges, refreshes, and legacy single-use OBO proofs are
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
| `briefcase` | Canonical public production Application ID | Public configuration |
| `01a070db-89b4-7542-83f1-4fad5cbce625` | Internal Application UUID; resource for admin step-up | Non-secret operator metadata |
| Application secret | Backend authentication to IAM | AWS Secrets Manager; never client responses |
| Webhook signing secret and key version | Verify IAM's exact signed body | IAM and Briefcase backend secret stores |
| Test Application secret | Fresh credential for the imported Application in one IAM test plane | Encrypted Briefcase pairing; never substitute production secret |
| IAM test root | Select outbound IAM test plane and match signed test webhooks | Encrypted pairing |
| Briefcase test selector | The paired IAM test Application secret; selects the sandbox; does not grant lifecycle administration | Authorized operator/client secret storage |

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

1. Inspect `iam --url https://backend.iam.teamofsilicons.com -o json app webhook 'briefcase'`.
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
5. Run the official IAM CLI's `app approve-webhook 'briefcase'` with the
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

## OBO registration and consent

> **Live integration baseline — October 3, 2026:** Briefcase contract 3.0.0 and IAM 5.0.0 are deployed, together with the Briefcase 3.0.1 account picker and Carbon/Silicon popup interface. Check `/api/version` for the backend contract; native client releases have their own package version.

Briefcase 3.0.0 verifies reusable `oba_` access tokens online through IAM 5.0.0. Request OBO consent after login, exchange and refresh the OBO tokens through the initiating app, and send `X-App-ID` plus `X-IAM-OBO-Access-Token` to supported delegated routes. The legacy one-shot raw upload is retired. See the [OBO migration guide](obo.md) for endpoint registration, code migration and release gates.

IAM verifies the receiving app and approved endpoint graph; Briefcase still checks the represented identity, selected organization, current membership, role/tag disclosure, app namespace and resource permissions. Missing identity or role disclosure fails closed. Undisclosed tags remain unknown and never grant tag-based access or erase cached assignments. A verified snapshot is never cached as authorization for a later request.

Webhook approval, endpoint registration, login scopes and OBO consent are separate actions. The capability-only byte transfer is not an IAM endpoint and cannot publish; commit requires a valid OBO token and a fresh online authority check.

## First official release requirements

Use IAM client 5.0.0 and the corresponding deployed IAM contract. Subject
snapshots require `self.identity.read` and `self.membership.read`.
`self.tags.read` adds tag-based access; its absence does not block the member's
own private files. Use `self.organizations.read` for organization selection.
Recipient/tag discovery uses `directory.carbons.read`, `directory.silicons.read`,
`directory.memberships.read`, and `directory.tags.read`. Scope-projected
responses must not be parsed as complete admin-directory models.

Optional `self.email.read` lets Briefcase learn the signed-in subject's verified
contact for invitation mail. IAM does not reveal other members' emails; see
[Sharing](sharing.md) for the resulting recipient-resolution limits.

Register `briefcase.invitations.create` (`POST /api/v1/obo/invitations`) and
`briefcase.link_access.update` (`POST /api/v1/obo/link-access`) as critical,
user-approved endpoints in Honeycomb with empty metadata schemas. Both also
create read-only expiring shares when the body carries `expires_in_minutes`. All
other existing file CRUD endpoints remain noncritical. Every OBO path stays
inside the calling app's namespace and the represented actor's permissions.
See [OBO](obo.md).

Testing callers pass the imported IAM test app secret. Briefcase validates it
through the official SDK and discovers its environment without manual pairing.
Honeycomb owns creation, imports, root-key rotation and lifecycle; IAM remains
the runtime identity authority. Production and testing credentials are never
interchangeable. See [participant integration](honeycomb-integration.md).

For current browser integration patterns, see [the application guide](build-with-iam5.md).
