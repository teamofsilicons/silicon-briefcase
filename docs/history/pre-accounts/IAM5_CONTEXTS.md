# IAM 5 account and organization contexts

Based on the [IAM 5 migration guide](https://docs.iam.teamofsilicons.com/migrating-to-iam-5/).

Ordinary login and inspection reject legacy unscoped/multiple-organization sessions
and OBO scopes. Refresh keeps the original actor and organization. Testing actor
login can send `org_id`; a production login code must match any explicitly selected
organization. Each CLI profile holds an independent context.

The browser gateway links independent sessions with a private server-side group.
A random public context ID guards requests and media URLs; it is not a credential.
Only contexts in the same group and plane can be listed or selected. Callback
flows retain the original group even when the Strict main cookie is absent on
IAM's cross-site redirect. Logout rejects only the selected session, preserving
others. Legacy persisted sessions require a new login.

Validation: client/CLI/gateway regression tests, focused IAM contract tests,
frontend typecheck and build. Live IAM 5 validation remains a release gate.
