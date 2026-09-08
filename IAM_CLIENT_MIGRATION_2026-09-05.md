# Official IAM client migration — live verification

## Implementation

Briefcase now depends directly on the registry-published
`silicon-iam-client = "=1.2.0"`. `cargo tree -i silicon-iam-client --depth 1`
confirmed the dependency. There is no direct Briefcase `reqwest` dependency or
custom IAM HTTP transport left. `src/infrastructure/iam/official.rs` delegates
version negotiation, environment validation, application discovery, SLT
exchange, refresh, introspection and single-use OBO verification to the SDK.

Bearer and OBO requests use IAM's current authorization snapshots, cross-bound
to principal, actor, organization, audience, membership epoch and test-plane
UUID. Required role/tag disclosure cannot default to empty or lesser facts.
Complete snapshots synchronize the caller's directory record and canonical
tag IDs under the same reconciliation lock used by webhooks. Older membership
versions/epochs cannot overwrite newer directory facts.

SDK dependency auto-updating is disabled. The complete-request timeout remains
configurable; the unsupported separate connect timeout is removed. The SDK
caps incoming bodies at 4 MiB; Briefcase additionally caps decoded models.
The official SDK does not expose replay response headers, so the session broker
no longer invents an `Idempotency-Replayed: false` result. Retry keys still pass
through unchanged.

## What was actually exercised

This was manual use of installed IAM CLI 1.2.0, the real Briefcase CLI/client,
and individual HTTP API requests against running local services, PostgreSQL
and MinIO. No automated test suite, mock server or scenario runner was run.
Small command wrappers only protected credentials and captured private receipts.

IAM ran the updated local source at `ec04ec9`, with backend migration 0067 and
shared-testing migration 9003. The original shared `silicon_iam_testing`
database was used, not the previous dedicated-database workaround. IAM worker
and webhook delivery were deliberately stopped throughout these checks.

| Live operation | Observed result |
| --- | --- |
| Production IAM login; two new IAM test environments | Success |
| Same Carbon handle, email and phone signed up in both environments | Success; distinct principal UUIDs |
| Import the same Briefcase application into each IAM plane | Success; distinct application UUIDs/secrets |
| Production Briefcase SLT login; create two paired Briefcase environments | Success through official SDK |
| First test-plane login and root listing, before any webhook | Success; public/private roots materialized |
| Create folder; upload `hello space.txt`; read its real bytes | Success; correct URL escaping and 77-byte content |
| Read an environment-A entry from environment B | Not found |
| Submit A's bearer under B's test root key | HTTP 401 |
| Register caller and OBO endpoint with IAM CLI | Success |
| Mint proof with IAM CLI; upload using `briefcase app upload` | Success; represented owner and originating app recorded |
| Replay the consumed proof | Rejected as unauthenticated |
| Assign IAM tag while retaining old application bearer | Old bearer rejected immediately |
| Fresh login after tag assignment; browse new tag root | Success without webhook delivery |
| OBO upload into `engineering` with current delegated tag authority | Success |
| Mint proof, revoke tag, then submit still-unexpired proof | Rejected as unauthenticated |
| Fresh login after revocation; read original public file | Success |
| Direct `/auth/slt` request and same-key retry | HTTP 200; identical access/refresh credentials |
| Direct `/auth/refresh` and same-key retry | HTTP 200; refresh rotated; retry returned identical credentials |
| Reuse spent refresh with a different key | HTTP 401 |
| Read entry through API using refreshed bearer | HTTP 200 |
| Clean B after creating disposable folder | Success; 14 metadata rows erased |
| Browse B immediately after clean using existing IAM session | Success; new roots, no webhook or IAM mutation needed |
| Read the cleaned disposable folder | Not found |
| IAM and Briefcase readiness | Both ready |

The tag-revocation check used an organization owner who also owned the created
file. Existing owner/file-owner rights are independent of tag membership; this
run does not claim that removing a tag removes those other rights.

## Remaining qualifications

- This verifies the migrated integration paths, not every Briefcase endpoint
  or every possible role combination. It is not a claim that the entire product
  has zero defects.
- Online snapshots lack tag aggregate versions. A conflicting existing
  tag name/removal is deliberately not overwritten; the request fails
  closed until directory reconciliation catches up. Webhooks are still needed
  for complete non-caller directory/lifecycle maintenance.
- `cargo build --bins` and `cargo check --all-targets` were used for compilation;
  automated tests were not executed, as requested. Existing IAM adapter fixtures
  were updated for the official typed snapshot contract.
- The repository already contained extensive uncommitted work. It was preserved;
  IAM source was not changed. Pre-existing whitespace in `UNDERSTANDING.md` was
  left untouched.

## Retained environments

| Plane | IAM environment | Briefcase environment |
| --- | --- | --- |
| A | `01a070c9-122d-7c63-97ae-988dd3a48409` | `01a070ca-19d3-7102-86ef-0b0b5e79fce1` |
| B | `01a070c9-5b09-7d20-97ff-02d76e8467eb` | `01a070cb-5bbf-7712-a903-f43284e88165` |

A retains the sample folder and three uploaded files. B's disposable fixture
was erased by the clean operation (not a bin/restore operation); its roots were
rebuilt afterward. Credentials and raw receipts remain private under
`/tmp/briefcase-iam12-manual.8QCGcq`, not in the repository. The previous
`E2E_REPORT_2026-09-05.md` is historical; its old IAM bootstrap and OBO limitations
are superseded by the live results above.
