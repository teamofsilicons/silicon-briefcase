# Honeycomb participant integration

Briefcase provides a protected participant control plane for Honeycomb's shared
testing coordinator. Honeycomb owns application and endpoint configuration, releases,
shared lifecycle and recovery policy; IAM verifies runtime identities, grants and
OBO proofs. Briefcase owns isolated file data, quotas and provider cleanup.

A participant deployment requires Honeycomb's Briefcase transport, matching internal
authentication and verified shared-environment behavior. Environments that include
IAM also require its shared lifecycle transport. Validate these requirements for
each deployment; archive validation alone does not verify service coordination.
See [Honeycomb's testing workflow](https://docs.honeycomb.teamofsilicons.com/testing-environments/)
for creating, importing and managing shared environments.

## Configure the service

Users enter Briefcase with the imported application's test `app_secret` and a test
SLT or existing test actor ID. They do not create, copy or configure an internal
service credential. See [testing environments](testing-environments.md).

Honeycomb's managed deployment tooling automatically provisions the internal
credential in both services' secret stores. Provisioning preserves an existing
matching credential, resumes a partial update without rotating it, and stops if
the stores contain conflicting credentials. The ordinary Briefcase testing database
and encryption configuration remains described in [deployment](deployment.md).

These settings are internal deployment configuration, not user setup steps:

| Internal variable | Purpose |
| --- | --- |
| `BRIEFCASE_HONEYCOMB_SERVICE_TOKEN` | Deployment-provisioned bearer credential for participant PUT/GET calls; at least 32 characters |
| `BRIEFCASE_HONEYCOMB_BASE_URL` | HTTPS Honeycomb origin for activity reporting; defaults to `https://backend.honeycomb.teamofsilicons.com/` |

The internal credential is separate from Honeycomb's environment `testing_key`,
the IAM app secret, and member tokens. Participant calls still require dedicated
service authentication; invalid or unavailable credentials fail closed. Credentials
must not appear in URLs, logs, release archives or browser configuration.

## Apply or recover an operation

Both methods use the backend host root, outside `/api/v1`:

```http
PUT /internal/honeycomb/organizations/{org}/testing-environments/{id}/operations/{op}
Authorization: Bearer <dedicated service token>
Content-Type: application/json
```

```json
{
  "operation_id": "<non-nil UUID matching op>",
  "environment_id": "<non-nil UUID matching id>",
  "org_id": "<organization matching org>",
  "app_id": "tos>briefcase",
  "environment_revision": 1,
  "generation": 1,
  "key_version": 1,
  "action": "prepare",
  "testing_key": "<32 alphanumeric characters>",
  "snapshot": {},
  "reason": "Shared environment preparation",
  "retired_apps": []
}
```

The revision, generation and key version are positive integers. Supported actions
are `prepare`, `import`, `refresh-import`, `rotate-key`, `clean`, `disable`, `restore`
and `purge`, plus `retire-applications` for exact application retirement. The latter
requires a nonempty `retired_apps` list and echoes it in the receipt. When it includes
the application identity configured by `BRIEFCASE_IAM_APP_ID`, Briefcase waits for
its storage cleanup and retains a retired
participant fence; a later explicit prepare/import can attach an empty Briefcase
again. Retirement of other applications preserves Briefcase data. `snapshot`,
`reason` and `retired_apps` are optional and default to JSON
null, an empty string and an empty array respectively. Unknown fields are rejected.
All path identities must match the body. The operation's `app_id` must match the
current application identity configured by `BRIEFCASE_IAM_APP_ID`. The examples
use the deployed `tos>briefcase` identity; other deployments use their configured ID.

Persist the complete input and operation UUID before dispatch. Repeating the same
operation with unchanged inputs recovers its result. Reusing an operation UUID with
changed input or sending a stale lifecycle revision fails; do not replace an
uncertain operation with a new identity.

```http
GET /internal/honeycomb/organizations/{org}/testing-environments/{id}/operations/{op}
Authorization: Bearer <dedicated service token>
```

PUT and GET return a durable, secret-free receipt:

```json
{
  "operation_id": "<UUID>",
  "environment_id": "<UUID>",
  "app_id": "tos>briefcase",
  "environment_revision": 1,
  "generation": 1,
  "key_version": 1,
  "retired_apps": [],
  "state": "completed"
}
```

States are `pending`, `completed` and `failed`. GET observes the stored receipt;
retry the identical PUT to advance an unfinished operation after cleanup progresses.
A dispatched request or a pending receipt is not successful participant completion.
The protected service credential works independently of disabled test sessions.

## Cleanup and stale-work fences

Briefcase uses shared/exclusive database lifecycle fences for participant operations
and file traffic. Cleaning disables access while test records and provider objects
are removed. The operation remains pending while durable storage cleanup exists;
only then can it complete. Generation and key-version checks reject stale discovery,
requests and webhook contexts. Restore preserves retained data and cannot recover
content removed by clean. `retire-applications` requires a nonempty `retired_apps`
selection. If it includes the current configured application identity, Briefcase
clears its data before reporting
retirement; otherwise it retains its own data and acknowledges the selection. Receipts
echo `retired_apps` so the coordinator can match the exact request. Shared environments
are excluded from legacy local expiry.

## Report activity

The API persists activity for active shared environments. Its background reporter sends:

```http
POST /api/v1/environments/{id}/apps/tos%3Ebriefcase/activity
X-Testing-Environment-Key: <current Honeycomb testing_key>
Idempotency-Key: <stable activity identity>
Content-Type: application/json

{"generation":1,"key_version":1}
```

The activity path uses the URL-encoded current `BRIEFCASE_IAM_APP_ID`; the example
shows `tos>briefcase`. The origin comes from `BRIEFCASE_HONEYCOMB_BASE_URL`. Requests
reject redirects and
use a five-second timeout. Successful delivery advances the recorded activity only
for the same active generation and key version; failed delivery remains retryable.
Honeycomb determines inactivity and recovery deadlines from its coordinated state.

## Deployment acceptance

Configure Honeycomb to send the exact participant contract above and preserve each
operation identity until it reaches a terminal result. Validate with an isolated
shared environment: prepare/import, Briefcase app-secret discovery and test login,
permission-limited file access, rotation, cleaning with real provider deletion,
disable, restoration and an explicitly authorized purge. Verify that the same UUID
is used throughout and that stale requests, credentials and events fail closed.
Confirm activity reaches Honeycomb and interrupted operations recover their receipts.
These checks require deployed services; local package validation alone cannot prove
them. Existing `tos>briefcase` IAM identity adoption into Honeycomb must preserve its
identity and credentials through Honeycomb's supported adoption workflow.

## Build the release archive

The release workflow builds the six manifest targets with `honeycomb-managed`.
After those binaries are available beneath `clients/rust/target/<triple>/release/`,
run with Python 3.11+ and the official Honeycomb CLI:

```sh
python3 scripts/package-release.py
```

Use `--honeycomb /path/to/honeycomb` if it is not on PATH. The script checks native
binary OS/architecture and version metadata, stages `honeycomb.yaml` at the root,
runs Honeycomb validation, packs the archive, then validates the archive again.
The output is `dist/briefcase-1.1.0.tar.gz` with SHA-256 and binary hash records.
Building an archive does not publish a release or adopt the IAM application.
