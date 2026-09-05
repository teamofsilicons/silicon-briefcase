# Paired IAM and Briefcase testing environments

Use the same member operations against an isolated dataset, with identities
issued by a paired IAM test plane. A test environment is not a second public
hostname, a new EC2 per run, or a flag that disables permissions.

The hosted deployment has one Briefcase API/worker EC2 and two separate RDS
instances. Production holds normal data and environment control records; the
shared testing database holds all sandbox data, namespaced by environment UUID.
Creating a sandbox uses that existing infrastructure; it does not provision RDS.

## Four values that must not be confused

| Value | Where it is used | Secret? |
| --- | --- | --- |
| IAM environment UUID | `iam --test`, Briefcase pairing request | No |
| IAM root key | Outbound calls to IAM; signed test webhook matching | Yes |
| Briefcase environment UUID | `briefcase --test`, production lifecycle URLs | No |
| Briefcase root key | `X-Testing-Environment-Key`, Rust `EnvironmentKey` | Yes |

Pairing additionally needs the canonical imported Application ID (`tos>briefcase`
on this deployment) and its fresh **test-only** IAM Application secret. IAM's
root key selects a plane; it is not an Application credential. Briefcase's root
key selects its sandbox; it is not a Carbon/Silicon bearer token.

Never paste a root key into `--test`, put a test UUID in the root-key header, or
reuse the production app secret in a test plane. Omitting the test selector
selects production, not a default sandbox. Production and test sessions do not
interchange, and unknown selectors fail closed.

## Prepare IAM

These commands create state. Run them deliberately in the intended profile,
with secret output kept out of logs and shared terminals.

1. From a production IAM session in the organization, create the IAM sandbox:

   ```bash
   iam --url https://backend.iam.teamofsilicons.com --org tos \
     env create briefcase-manual-e2e --description 'Disposable integration data'
   ```

   Record the returned UUID as `IAM_TEST_ID`; keep its root key in private
   storage. The IAM CLI stores environment keys per profile. Production
   lifecycle commands must not inherit `SILICON_IAM_TEST`.
2. Use `iam --test "$IAM_TEST_ID" signup --help` or `login --help` to establish
   a test Carbon. A production account/session is not automatically copied.
   Keep the same IAM URL/profile throughout setup.
3. In that signed-in test session, import the production Briefcase Application:

   ```bash
   iam --url https://backend.iam.teamofsilicons.com --test "$IAM_TEST_ID" \
     app import 'tos>briefcase'
   ```

   Import preserves the canonical Application ID and returns a fresh test-only
   Application secret. The test Carbon must administer an existing target
   organization; import does not grant access to an unrelated existing one.
   Keep the returned secret separate from production credentials.

The IAM CLI's `env key` command retrieves a key in the production control
plane if an authorized caller needs it again. Handle its output as a secret.
Do not create or replace test webhook destinations merely to get first login
working: IAM 1.2 online snapshots support first-use bootstrap directly.

## Create the Briefcase sandbox through the CLI

First log into production Briefcase with an organization-bound SLT for
`tos>briefcase`; see [CLI login](cli/README.md). Then:

```bash
briefcase env create briefcase-manual-e2e \
  --description 'Disposable integration data' \
  --iam-environment-id "$IAM_TEST_ID" \
  --iam-app-id 'tos>briefcase'
# Hidden prompts: IAM root key, then the imported test Application secret.
```

Any current Carbon/Silicon organization member can create a sandbox. The
organization owns it and the actor is recorded as creator. Creation validates
the complete IAM pairing and returns an independent Briefcase UUID/root key.
The CLI saves the root privately; set `BRIEFCASE_TEST_ID` to the returned UUID.

For non-interactive jobs, inject `BRIEFCASE_IAM_ENVIRONMENT_KEY` and
`BRIEFCASE_IAM_APP_SECRET` through the job's secret store. Those variables
refer to the **test pairing**, not production server credentials. Do not echo
them or put literal credentials into shell history.

Now obtain a Briefcase-targeted SLT inside the paired IAM plane, for the same
organization and the member whose permissions you want to exercise:

```bash
iam --url https://backend.iam.teamofsilicons.com --test "$IAM_TEST_ID" \
  --org tos login --app-id 'tos>briefcase'
briefcase --test "$BRIEFCASE_TEST_ID" login --org tos
# Paste the test SLT at the hidden prompt.
briefcase --test "$BRIEFCASE_TEST_ID" env current
briefcase --test "$BRIEFCASE_TEST_ID" ls
```

`env current` proves root-key selection only. `ls` also exercises the test
bearer and current IAM authorization. Neither proves that webhooks work.
The CLI uses the hosted Briefcase URL automatically unless a saved profile or
an explicit URL override selects another deployment; the test UUID selects
the isolated dataset on that deployment.
Do not use the old profile-photo mutation workaround: it is obsolete.

## The same setup through HTTP

All paths below are relative to `/api/v1`. Send production bearer authentication
and `X-Org-ID: tos` to create/manage an environment. Do not include a test root
on these UUID-addressed lifecycle routes.

```http
POST /api/v1/organizations/tos/testing-environments
Authorization: Bearer <production-Briefcase-access-token>
X-Org-ID: tos
Idempotency-Key: <persisted-unique-request-key>
Content-Type: application/json

{
  "name": "briefcase-manual-e2e",
  "description": "Disposable integration data",
  "iam_environment_id": "<IAM-environment-UUID>",
  "iam_environment_key": "<IAM-root-key>",
  "iam_app_id": "tos>briefcase",
  "iam_app_secret": "<test-only-Application-secret>"
}
```

Angle-bracket values are placeholders, not usable credentials. The response
contains flat environment metadata (`id`, `name`, and the other fields)
alongside `key`; it never echoes the IAM root or Application secret. The Rust
model groups those metadata fields under `.environment` through Serde flattening.
Persist the request and idempotency key securely before
submission; retry an uncertain result with the exact same request/key.

To exchange a test SLT, call `POST /auth/slt` with `X-Org-ID`, the Briefcase root
header and an idempotency key, body `{"slt":"<test-SLT>"}`. For ordinary file
operations send:

```http
GET /api/v1/entries
X-Org-ID: tos
X-Testing-Environment-Key: <Briefcase-root-key>
Authorization: Bearer <paired-test-member-access-token>
```

Only `/testing-environment` and its `/cleanings` self-service operation use the
root key without a member bearer. The latter is destructive. Full endpoint
shapes, ETags, status codes and lifecycle routes are in the [API guide](api/README.md).

## The same setup through Rust

The [Rust guide](client/README.md) includes `TestingEnvironmentCreate` and all
lifecycle methods. Use a production `Client` for creation, then construct a
separate client for data-plane calls:

```rust
use briefcase_client::{Client, Config, EnvironmentKey, ListEntries};

let sandbox = Client::connect(
    Config::new("https://backend.briefcase.teamofsilicons.com/api/v1/", "tos")?
        .with_auto_update(false)
        .with_environment(EnvironmentKey::new(briefcase_root_key)?)
        .with_token(test_member_access_token),
).await?;
let metadata = sandbox.current_testing_environment().await?;
let entries = sandbox.list_entries(&ListEntries::default()).await?;
```

The root and bearer variables come from private caller-owned storage. The
package neither logs in behind your back nor stores sessions globally.
Environment mutation methods have `_with_key` variants for durable retries.
Use the [complete read-only example](client/examples/sandbox.rs) to inspect an
already prepared plane without creating, cleaning, or deleting anything.

## Lifecycle and authority

| Action | Authority / effect |
| --- | --- |
| Create | Current production organization member; empty sandbox and new root |
| Retrieve/rotate key, edit, clean, retire, restore, re-pair | Creator or current organization owner/admin through production management |
| Describe current / clean current | The selected Briefcase root is sufficient |
| Ordinary file calls | Root plus valid paired test bearer; actual member permissions apply |
| OBO file creation | Root plus a valid proof from the paired IAM plane, not a bearer alongside it |

```bash
briefcase env show "$BRIEFCASE_TEST_ID"
briefcase env key "$BRIEFCASE_TEST_ID"             # retrieves and stores secret
briefcase env rotate-key "$BRIEFCASE_TEST_ID"      # immediately invalidates old root
briefcase env pair-iam "$BRIEFCASE_TEST_ID" \
  --iam-environment-id "$REPLACEMENT_IAM_TEST_ID" --iam-app-id 'tos>briefcase'
```

Re-pairing validates and replaces the complete IAM UUID/root/app-ID/app-secret
tuple while preserving Briefcase UUID, root and data. Once the sandbox has an
IAM organization projection, keep the same IAM environment UUID when updating
its root or Application secret. To use a different IAM environment, create a
new Briefcase sandbox. A used sandbox rejects that switch with HTTP409
`testing_environment_iam_rebind_requires_new_environment`; its existing pairing
and data remain unchanged. Cleaning does not remove the IAM projection.

A different UUID can be selected before the first IAM projection is created.
This restriction prevents identical public handles in different IAM planes
from silently inheriting existing file ownership and grants. Accepted pairing
updates fence old in-flight configuration; the CLI discards the obsolete test
session. Obtain a fresh SLT from the paired IAM plane. Rotating an IAM root or
app secret without updating the pairing makes outbound IAM requests fail;
there is no production fallback.

Destructive actions below are for disposable test data only:

```bash
briefcase --test "$BRIEFCASE_TEST_ID" env clean    # root-authorized content erasure
briefcase env delete "$BRIEFCASE_TEST_ID"          # retire; current root destroyed
briefcase env restore "$BRIEFCASE_TEST_ID"         # restore during recovery; new root
```

Clean retains the environment/key and paired IAM identity projection but erases
file data, versions, grants, activity, notifications, storage settings and usage.
It queues exact S3 cleanup descriptors before removing source metadata. Logical
completion does not imply all physical object deletion has already finished;
the worker retries cleanup. The operation cannot be undone by environment restore.

Retirement is different: data remains recoverable for 30 days, the root stops
working immediately, and restoration generates a new root. After purge, restore
is unavailable. At most ten environments may be active across the deployment;
retained deleted rows may make `env list --status all` longer than ten.

Each sandbox is capped at 2 GiB (2,147,483,648 bytes). Exceeding it returns
`testing_environment_storage_limit_exhausted` with the product's current message:
`In test enviorment you are limited to a total storage of 2gb per enviorment.`

Idle environments are automatically retired after 30 days without accepted
test-plane activity. The separate recovery window starts when retirement occurs.

## Signed webhook routing

IAM test events use the normal backend `/webhook/` URL and an authenticated
outer test wrapper. Briefcase verifies its signature before using the embedded
IAM root to find the active pairing. Test rows cannot be written into production
merely by changing an unsigned routing field. Do not log the wrapper/root or
send the Briefcase root as if it were an IAM root.

Production webhook review and test-plane event routing are separate concerns.
IAM test registrations can activate without a production platform review, but
imported webhook configuration and signing keys must still be checked against
the actual IAM import contract. The shared Briefcase receiver uses its configured
signing-key ring; do not invent a per-environment signing secret unsupported by
that receiver. See [IAM integration](iam-integration.md).

## Manual verification checklist

Run these intentionally against named disposable environments; this page is a
checklist, not an automated suite or a claim they have all passed on hosting.

- Confirm contract negotiation, root self-description, and first SLT login/`ls`
  without forcing a webhook. Record environment IDs, versions and request IDs.
- Upload a known small file, download it and compare bytes; replace the same
  name and inspect versions; restore one and verify its content.
- Exercise Public, own Private, a second actor's shared folder, and a tag folder.
  Verify hidden paths return 404, read-only cannot write, and update cannot delete.
- Switch among two separate test actors and two separate sandboxes. Wrong roots,
  wrong-plane tokens, stale rotated roots and cross-organization paths must fail.
- Verify current role/tag removal denies subsequent requests. Separately cause a
  signed test event, check IAM delivery and the correct projection, and verify
  duplicate/stale events do not restore removed authority.
- Exercise clean, rotation, re-pair, retirement and restoration on disposable
  data; confirm old keys/sessions fail and uncertain mutations replay safely.
- Check quota errors without creating oversized production objects. Ensure
  another environment and production remain unchanged.
- Test OBO only after registering the endpoint catalog; exact-body proof success
  and replay/tampered-body rejection are separate checks.

For each check record expected versus actual behavior and failures. Do not mark
complete solely because `/healthz` or one listing succeeds. Never record raw
tokens, roots, app secrets or signed test wrappers in the evidence report.
