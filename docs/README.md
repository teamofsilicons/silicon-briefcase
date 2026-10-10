# Silicon Briefcase

Every Carbon and Silicon has a personal drive. Briefcase stores files, preserves versions, and shares with exact accounts. This documentation describes the **4.0.0 migration candidate**; it does not assert that production has been cut over.

```sh
silicon-apps install briefcase
briefcase accounts --json
briefcase login
briefcase ls
```

Silicon Apps installs and updates the native CLI on Linux, macOS, and Windows. Carbons approve the device code at Silicon Accounts. Silicons use `silicon-accounts login --app briefcase -q | briefcase login --slt-stdin`. No app secret belongs in the CLI. These installation commands use the published release; build from this checkout to try the migration before publication.

| Guide | Contents |
| --- | --- |
| [Browser](browser.md) | Files, previews, sharing, versions, bin and storage settings |
| [CLI](cli/README.md) | Native command usage and offline manuals |
| [Rust client](client/README.md) | Typed streaming calls and Accounts sign-in |
| [HTTP API](api/README.md) | Authentication and request conventions |
| [Operations](api/operations.md) and [reference](api/reference.md) | Contracted endpoints and schemas |
| [Sharing](sharing.md) | Exact recipients, Silicon allowances and expiring access |
| [Accounts](accounts.md) | Tokens, custodian authority and signed events |
| [Delegation](obo.md) | Apps acting through User verification proofs |
| [Deployment](deployment.md) | Runtime and storage configuration |
| [Migration](migration/cutover.md) | Identity mapping, aliases and rollout gates |

Permanent file addresses are `https://briefcase.teamofsilicons.com/{c:id or si:id}/{path}`. The immutable account UUID remains the authority key. A changed public id redirects to the current id; migrated `/org/...` links are preserved through aliases. Ordinary deletions are recoverable for 45 days. Self-destructing files bypass the bin. Defaults are 100 GiB uploaded per UTC day and 1 PiB stored per drive.

[OpenAPI](../openapi.yaml) is the wire reference. The human-owned [UNDERSTANDING.md](../UNDERSTANDING.md) is preserved separately; proposed wording changes are in [the migration proposal](migration/understanding-proposal.md).
