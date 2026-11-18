# API version policy

Contract **5.3.0** adds recipient review of incoming offers, sender approvals and withdrawal. The 5.3 Rust client and CLI handle queued responses. Migration 0057 stores pending offers separately from active grants.

Contract **5.2.0** adds incoming share settings for every account. Migration 0056 defaults all recipients to open. The setting applies to direct grants and invitations and affects the legacy Silicon allowance operations. The matching 5.2 CLI and Rust client negotiate the new operation revisions. Deploy the Silicon Contacts Briefcase share-check route before enabling the new service. See [Sharing files](sharing.md).

Contract **5.0.0** adds personal/work organization controls and live organization/team sharing. It requires the matching 5.0 Rust client and CLI: group recipient variants and changed invitation operation revisions are incompatible with older exhaustive clients. `listInvitations` is revision 2.0.0; `createInvitation` is 2.1.0. The route namespace remains `/api/v1`. The retired telemetry operation is no longer advertised; its old URL still returns 410 without reading the body. Migration 0053 replaces legacy IAM tag authority with live Organizations membership.

Briefcase 4.0.0 authenticates with Silicon Accounts: every route takes a Silicon Accounts access token issued to `briefcase`, the delegated `/obo/` operations take a User verification proof (`Authorization: Proof sap_…`) whose scope is the operation's endpoint id, and `X-Org-ID` is no longer read (except as the transfer route's tenant selector). `/iam`, `/auth/*` and the testing-environment operations are removed; `/accounts` and `/me` are added. The route namespace remains `/api/v1`.

Contract 4.0.0 also makes every account's drive the unit of storage and sharing. `/drives`, `/shared` and the Silicon allow-list (`/silicons/{silicon}/allowed-accounts`) are added; listings, uploads, the bin, usage and storage settings take `drive` (an owner's `c:`/`si:` id or uuid). Operations whose request, response or refusals changed carry new revisions: `listEntries`, `createFolder`, `createInvitation`, `readOrganizationUsage` and `configureOrganizationBucket` 2.0.0; `updateEntry`, `uploadFile`, `grantPermission`, `inspectEffectivePermissions`, `listNotifications` and `listBin` 1.1.0; `setLinkAccess` and `resolvePermanentUrl` 1.2.0.

Contract 4.1.0 removes telemetry: Briefcase records none. `submitTelemetry` (`POST /telemetry`, revision 1.0.0) stays in the catalogue only because 4.0 clients verify it at startup; it now answers 410 `telemetry_removed` without reading the body and is removed with the next contract major. The 4.1.0 client no longer lists it and sends `x-briefcase-telemetry: off` on every request. Every other operation revision is unchanged, so 4.0 and 4.1 clients and services interoperate in both directions.

## Negotiation

Before sending credentials, the official client requests `GET /api/version` with `Briefcase-Supported-API-Versions: v1`. It verifies the service identity, selected major and every known operation's exact ID, revision, method and path. A missing or changed required operation fails startup. Extra operation IDs are allowed; duplicate IDs are invalid.

Every versioned request also negotiates that header. Omission selects the latest served major. A request with no supported major returns 406. Responses identify `Briefcase-API-Version: v1` and vary by the negotiation header. An application must not silently ignore a failed contract check; install matching service and consumer releases.

## Compatibility matrix

| API contract | Backend | Rust client / CLI | Identity SDK |
| --- | --- | --- | --- |
| 5.3.0 / v1 | 5.3.0 | 5.3.0 | silicon-accounts-client 0.4.0 |
| 5.2.0 / v1 | 5.2.0 | 5.2.0 | silicon-accounts-client 0.4.0 |
| 5.0.0 / v1 | 5.0.0 | 5.0.0 | silicon-accounts-client 0.4.0 |
| 4.1.0 / v1 | 4.1.0 | 4.1.0 | silicon-accounts-client 0.4.0 |
| 4.0.0 / v1 | 4.0.0 | 4.0.0 | silicon-accounts-client 0.4.0 |
| 4.0.0 / v1 | 4.0.0 | 4.0.0 | silicon-iam-client 5.0.0 |
| 2.1.0 / v1 | 2.1.0 | 2.1.0 | silicon-iam-client 4.0.0 |

Clients must not ignore mismatched operation revisions. Contract 3.0 changes delegated authentication behavior; 2.x clients do not pass the complete 3.0 operation check. Existing ordinary resource IDs and stored data are unchanged; the 5.0 release applies migration 0053. The [operation inventory](api/operations.md) and [OpenAPI document](../openapi.yaml) are the wire reference.

## Deprecation and sunset

The `briefcase.api_contracts` table is the operator-controlled lifecycle registry. Versions are `active`, `deprecated`, or `sunset`. Deprecate only after publishing a replacement and updating this matrix. An administrator can mark a version deprecated:

```sql
UPDATE briefcase.api_contracts
SET status = 'deprecated', deprecated_at = clock_timestamp(), last_request_at = NULL
WHERE version = 'v1' AND status = 'active';
```

A deprecated version remains usable. Each request records its exact arrival and receives a deprecation header and a link to this policy. The worker sunsets a deprecated version only after **seven uninterrupted days with zero requests**, measured from the later of deprecation and last request. Request updates and retirement serialize on the same database row, so an in-flight update cannot be lost to a retirement race. Active versions are never automatically sunset. A retired version returns 410, including during initial negotiation.

The seven-day rule determines retirement dynamically; there is no speculative fixed Sunset date. Worker downtime delays retirement rather than shortening the window. Operators should inspect `last_request_at`, `deprecated_at`, `sunset_at` and the service logs before removing an old implementation. Restoring an implementation requires an explicit lifecycle update and supported code.

## Consumer contract checks

CI validates the backend route/status inventory against OpenAPI and the official client's operation inventory. Wire tests cover exact credentials, request bodies, pagination, retries and response decoding; CLI tests verify command behavior and credential isolation. PostgreSQL integration tests verify permission, version, quota and lifecycle behavior. A contract change must update the reference, server registry, SDK registry, consumer fixtures, and documentation in the same change.
