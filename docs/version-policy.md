# API version policy

> **Integration preview for Briefcase 3.0.0 / IAM 5.0.0.** These docs are published ahead of the coordinated runtime rollout. Check `/api/version` before switching a production client; a published guide does not mean the new service is live.

Briefcase 3.0.0 uses IAM 5.0.0 and reusable OBO access tokens. Its delegated JSON operation revisions change to 3.0.0, and the legacy raw upload returns 410. The route namespace remains `/api/v1`. Ordinary file operations and the capability-only byte transfer keep their previous revisions. Deploy matched consumers and service together; see the [migration guide](obo.md).

## Negotiation

Before sending credentials, the official client requests `GET /api/version` with `Briefcase-Supported-API-Versions: v1`. It verifies the service identity, selected major and every known operation's exact ID, revision, method and path. A missing or changed required operation fails startup. Extra operation IDs are allowed; duplicate IDs are invalid.

Every versioned request also negotiates that header. Omission selects the latest served major. A request with no supported major returns 406. Responses identify `Briefcase-API-Version: v1` and vary by the negotiation header. An application must not silently ignore a failed contract check; install matching service and consumer releases.

## Compatibility matrix

| API contract | Backend | Rust client / CLI | IAM SDK |
| --- | --- | --- | --- |
| 3.0.0 / v1 | 3.0.0 | 3.0.0 | silicon-iam-client 5.0.0 |
| 2.1.0 / v1 | 2.1.0 | 2.1.0 | silicon-iam-client 4.0.0 |

Clients must not ignore mismatched operation revisions. Contract 3.0 changes delegated authentication behavior; 2.x clients do not pass the complete 3.0 operation check. Existing ordinary resource IDs and stored data are unchanged; this release adds no Briefcase database migration. The [operation inventory](api/operations.md) and [OpenAPI document](../openapi.yaml) are the wire reference.

## Deprecation and sunset

The `briefcase.api_contracts` table is the operator-controlled lifecycle registry. Versions are `active`, `deprecated`, or `sunset`. Deprecate only after publishing a replacement and updating this matrix. An administrator can mark a version deprecated:

```sql
UPDATE briefcase.api_contracts
SET status = 'deprecated', deprecated_at = clock_timestamp(), last_request_at = NULL
WHERE version = 'v1' AND status = 'active';
```

A deprecated version remains usable. Each request records its exact arrival and receives a deprecation header and a link to this policy. The worker sunsets a deprecated version only after **seven uninterrupted days with zero requests**, measured from the later of deprecation and last request. Request updates and retirement serialize on the same database row, so an in-flight update cannot be lost to a retirement race. Active versions are never automatically sunset. A retired version returns 410, including during initial negotiation.

The seven-day rule determines retirement dynamically; there is no speculative fixed Sunset date. Worker downtime delays retirement rather than shortening the window. Operators should inspect `last_request_at`, `deprecated_at`, `sunset_at` and release telemetry before removing an old implementation. Restoring an implementation requires an explicit lifecycle update and supported code.

## Consumer contract checks

CI validates the backend route/status inventory against OpenAPI and the official client's operation inventory. Wire tests cover exact credentials, request bodies, pagination, retries and response decoding; CLI tests verify command behavior and credential isolation. PostgreSQL integration tests verify permission, version, quota and lifecycle behavior. A contract change must update the reference, server registry, SDK registry, consumer fixtures, and documentation in the same change.
