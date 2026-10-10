# Apps acting for an account

Briefcase 4 accepts Silicon Accounts **User verification proofs** on its delegated routes. An initiating app asks Accounts for approval targeting `briefcase` with the operation's scope. It sends `Authorization: Proof sap_…`; it never forwards its ordinary app access token or an IAM OBO header.

The backend verifies the proof at Accounts and checks the issuer against `BRIEFCASE_PROOF_ISSUERS`. The proof's represented immutable account UUID acts in its own drive. Ordinary issuers are confined to `apps/{issuer}/`; explicitly configured first-party issuers can reach what the represented account may reach, including shares and custodial drives. File permissions remain in force.

| Scope | POST route under `/api/v1` |
| --- | --- |
| `briefcase.folders.create` | `/obo/folders/create` |
| `briefcase.entries.list` | `/obo/entries/list` |
| `briefcase.files.read` | `/obo/files/read` |
| `briefcase.entries.trash` | `/obo/entries/trash` |
| `briefcase.uploads.reserve` | `/obo/uploads/reserve` |
| `briefcase.uploads.commit` | `/obo/uploads/commit` |
| `briefcase.uploads.status` | `/obo/uploads/status` |
| `briefcase.uploads.cancel` | `/obo/uploads/cancel` |
| `briefcase.invitations.create` | `/obo/invitations` |
| `briefcase.link_access.update` | `/obo/link-access` |

The exact destination, range and disposition belong in the typed JSON payload. Mutations carry a stable non-nil `operation_id`. Retain the same manifest after an uncertain response and obtain a fresh proof for each request. A proof is not an ongoing permission grant or a signature over the JSON body. Do not retry a spent proof.

## Delegated uploads

1. Reserve with the destination, filename, content type, size and SHA-256 under the reserve scope.
2. Transfer bytes with the returned `X-Briefcase-Upload-Capability` to the exact transfer route. Send no account bearer or proof on the transfer.
3. Commit with a fresh proof under the commit scope. Publication rechecks authority, integrity and quota.
4. Reconcile uncertain outcomes through status and a fresh status proof. Cancel abandoned reservations with a cancel proof.

The capability cannot read or publish, and expires with its reservation. Raw `/obo/files` upload is retired with 410. See [OpenAPI](../openapi.yaml), the [Rust guide](client/README.md), and the [migration cutover](migration/cutover.md) for pre-Accounts reservations and consumers.
