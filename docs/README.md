# Silicon Briefcase

Organization-scoped files for Carbons, Silicons, and IAM-authorized applications. This is the documentation source for **https://docs.briefcase.teamofsilicons.com**, covering the first official release **1.0.0**.

## Start here

| Guide | What you can do |
| --- | --- |
| [Browser app](browser.md) | Sign in, browse, preview, upload, share, and restore |
| [CLI](cli/README.md) | Manage files and testing environments from a terminal |
| [Rust client](client/README.md) | Integrate the official typed, streaming client |
| [HTTP API](api/README.md) and [operation reference](api/reference.md) | Authenticate, use endpoints, and understand errors |
| [Operation inventory](api/operations.md) | All 58 operation IDs, methods, paths and revisions |
| [Sharing and logs](sharing.md) | Member/email/tag invitations, public links, mail and audit history |
| [OBO applications](obo.md) | Act inside an app namespace with current user permissions |
| [Delegated uploads](api/delegated-uploads.md) | Reserve, transfer, publish and recover staged uploads |
| [Testing environments](testing-environments.md) | Select a paired test plane using its IAM app secret |
| [IAM integration](iam-integration.md) | Configure scopes, critical endpoints and signed webhooks |
| [Version policy](version-policy.md) | Negotiation, compatibility, deprecation and sunset |
| [Deployment](deployment.md) | Run the backend, gateway, worker, storage and docs |

## Addresses

| Surface | Address |
| --- | --- |
| Application and permanent links | `https://briefcase.teamofsilicons.com/org/{org_id}/{path}` |
| API | `https://backend.briefcase.teamofsilicons.com/api/v1/` |
| Contract negotiation | `https://backend.briefcase.teamofsilicons.com/api/version` |
| IAM webhook receiver | `https://backend.briefcase.teamofsilicons.com/webhook/` |
| Documentation | `https://docs.briefcase.teamofsilicons.com/` |

IAM owns login, membership, roles and tags. An organization contains public, private, tag, and app folders. File names and paths stay readable; authorization decides what each actor can discover. Public within an organization and anyone-with-link access are distinct settings.

Upload any file type. Reuploading a name publishes the next immutable version on the same file ID. Download folders as streamed tar.zst, restore files from the 45-day bin, and inspect the preceding year of logs. The default limits are 100 GB per UTC day and 1 PB storage per organization; testing planes are limited to 2 GiB and ten active environments.

The [OpenAPI document](../openapi.yaml) is the wire reference. [UNDERSTANDING.md](../UNDERSTANDING.md) is the human-maintained product specification. Development 0.x contracts are unsupported by this release.
