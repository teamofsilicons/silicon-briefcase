# Silicon Accounts integration

Briefcase 4 uses `silicon-accounts-client` 0.4.0. Register app `briefcase` in Silicon Apps and configure its hosted sign-in callback, public-client sign-in and signed webhook. Backend configuration is in [deployment](deployment.md); the Next.js server has its own [environment](../web/.env.example).

Ordinary requests present `Authorization: Bearer <Accounts access token>`, issued to Briefcase. The backend verifies signature, issuer, audience, expiry and credential class against cached Accounts JWKS. Sensitive operations also introspect online. The subject's immutable UUID owns its drive; a `c:` or `si:` id is a mutable public label. Signed events refresh account status, current id and custodian projections; they do not grant arbitrary file access.

Carbons use hosted PKCE or the CLI device flow. Silicons exchange an app-targeted short-lived token using public-client mode. The CLI never receives an app secret. Server code alone uses the app secret for code exchange, refresh, introspection, lookups and proof verification. On sign-out revoke the refresh token and clear local state.

A Silicon's current custodian may manage that Silicon's drive while remaining the acting Carbon. Other Silicons and Carbons get only explicitly shared file rights. A recipient Silicon accepts its custodian, the custodian's other Silicons, and explicitly allowed accounts.

`GET /api/v1/accounts` discovers the app's public identity; `GET /api/v1/me` verifies the caller. No organization selector, IAM role/tag claim, Honeycomb test selector, or pre-Accounts session grants authority. Retired selectors return an explicit error. Existing data needs the reviewed identity mapping described in [cutover](migration/cutover.md).

Other apps use User verification proofs on [delegated routes](obo.md), with route-specific scopes and the backend's issuer allowlist. Ordinary bearer routes do not accept app proofs.
