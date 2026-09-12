# API version policy

**1.0.0 is the first official Briefcase release.** The service, Rust client, CLI and web gateway use release 1.0.0, API major `v1`, and the published OpenAPI contract 1.0.0. Earlier development releases are unsupported; this release includes breaking changes.

## Negotiation

Before sending credentials, the official client requests `GET /api/version` with `Briefcase-Supported-API-Versions: v1`. It verifies the service identity, selected major and every known operation's exact ID, revision, method and path. A missing or changed required operation fails startup. Extra operation IDs are allowed; duplicate IDs are invalid.

Every versioned request also negotiates that header. Omission selects the latest served major. A request with no supported major returns 406. Responses identify `Briefcase-API-Version: v1` and vary by the negotiation header. An application must not silently ignore a failed contract check; install matching service and consumer releases.

## Compatibility matrix

| API contract | Backend | Rust client | CLI | Browser gateway | IAM SDK |
| --- | --- | --- | --- | --- | --- |
| 1.0.0 / v1 | 1.0.0 | 1.0.0 | 1.0.0 | 1.0.0 | silicon-iam-client 1.7.0 |
| Development 0.x | Unsupported | Unsupported | Unsupported | Unsupported | Not a release target |

The [operation inventory](api/operations.md) and [OpenAPI document](../openapi.yaml) are the wire reference. All initial official operation revisions are 1.0.0. Future observable request, response, and behavior changes must update the affected operation revision and its consumers together. Breaking API-major changes use a new versioned namespace.

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
