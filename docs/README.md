# Silicon Briefcase documentation

Start here to integrate another application, use the Rust package, operate the
CLI, or work inside an IAM-paired testing environment.

## Choose your interface

| Guide | Audience and contents |
| --- | --- |
| [API](api/README.md) | HTTP authentication, request/response conventions, all public operations, permissions, filters, uploads, errors, and retention |
| [42-operation map](api/operations.md) | Exact HTTP route and revision mapped to the Rust method and CLI command |
| [Rust client](client/README.md) | Stateless `briefcase-client`, login/refresh, typed requests, streaming, retries, testing, and method inventory |
| [CLI](cli/README.md) | Installation, SLT login, profiles, file commands, sharing, scripting, environment management, and updates |
| [Testing environments](testing-environments.md) | IAM preparation, pairing, UUID/key distinctions, API/CLI/client usage, lifecycle, and hands-on checks |
| [IAM integration](iam-integration.md) | Official IAM SDK, application IDs, current authorization, signed webhooks, approval, and OBO setup |
| [Deployment runbook](deployment.md) | AWS topology, secrets, deployment, rollback, logs, and operational limits |

## Hosted addresses and identifiers

| Item | Value |
| --- | --- |
| API base for clients | `https://backend.briefcase.teamofsilicons.com/api/v1/` |
| Anonymous compatibility document | `https://backend.briefcase.teamofsilicons.com/api/version` |
| Health / readiness | Host-root `/healthz` / `/readyz` |
| IAM webhook receiver | `https://backend.briefcase.teamofsilicons.com/webhook/` |
| Permanent user-facing URLs | `https://briefcase.teamofsilicons.com/org/{org_id}/{path}` |
| Production IAM Application ID | `tos>briefcase` |

The application ID is public, not a credential. Its internal UUID is not the
ID used for app authentication or login. Quote IDs containing `>` in a shell.
The permanent-URL format is a product contract, not a claim that the frontend
has been deployed. Example actor paths must be replaced with actual IAM IDs.

## Authentication in one minute

1. IAM owns identities, organizations, membership, roles, and tags.
2. A Carbon or Silicon obtains an organization-bound, Briefcase-targeted SLT
   from IAM. Briefcase accepts that SLT, not the actor's password or OTP.
3. The backend exchanges it using the server-held Application secret and
   returns an access/refresh pair. The CLI stores and rotates it; a Rust caller
   owns its own storage and refresh policy.
4. Ordinary requests carry the actor bearer and `X-Org-ID`. A test request also
   carries the separate Briefcase root key. That key does not replace the actor.
5. Another application uses a single-use IAM OBO proof on `/obo/files`, not its
   own application secret on the member API.

Webhooks reconcile changes but do not grant request authority. First-use login
and sandbox bootstrap use current online IAM snapshots and need no synthetic
membership-update webhook. Production webhook approval is a separate IAM
platform-admin operation; see the [approval runbook](iam-integration.md).

## Contract and sources

[UNDERSTANDING.md](../UNDERSTANDING.md) is the requested product behavior.
[openapi.yaml](../openapi.yaml) describes the wire contract. The guides explain
how to use the API, package, and CLI. SDK/CLI sources are maintained in
the sibling `briefcase-client-rust` repository.
