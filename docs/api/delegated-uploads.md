# Delegated uploads

Briefcase 4 uses Accounts User verification proofs for reserve, commit, status and cancel, and a narrow upload capability only for bytes. The full workflow and scopes are in [Apps acting for an account](../obo.md). The [OpenAPI schemas](../../openapi.yaml) describe the exact typed manifests and responses.

Persist the logical operation id and manifest through retries. Hash the source before reserve. Match its length and SHA-256 during transfer. A reservation is not publication: only commit makes content visible. An expired, cancelled or already published reservation must be reconciled by its status, not blindly restarted. Storage cleanup runs in the worker. Existing IAM reservations must be drained before cutover because their authority bindings differ.
