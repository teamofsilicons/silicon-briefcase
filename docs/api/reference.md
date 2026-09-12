# HTTP operation reference

This reference is generated from the v1 [OpenAPI contract](../../openapi.yaml). Paths starting `/api/v1/` are absolute; normal requests require `Authorization: Bearer` and `X-Org-ID` unless the operation specifies another authority. Start with [API conventions](README.md), [sharing](../sharing.md), and [IAM OBO](../obo.md).

All operations below have revision **1.0.0**. Public links expose read/download only. Test selection always uses `X-Briefcase-App-Secret` in addition to the stated actor authority.

## Read the public IAM application ID before login

`GET /iam` · `readIamInfo`

No member token or organization is required. An optional X-Briefcase-App-Secret selects the paired test application's identity. Application secrets and environment keys are never returned.

Authority: anonymous.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Briefcase-App-Secret` | header | no | Selects a Briefcase test plane; omission selects production |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Public IAM configuration (Cache-Control no-store) | `application/json` IamInfo |
| default | Error | `application/json` Error |

## Verify the current Carbon or Silicon session with IAM

`GET /auth/status` · `readLoginStatus`

No organization selection is required. Missing, invalid, expired, or revoked member tokens return authenticated false. Active tokens return their identity and currently granted organizations; an empty grant list does not make an active session unauthenticated. IAM outages, malformed upstream responses, and invalid test-plane credentials remain errors.

Authority: anonymous **or** bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Briefcase-App-Secret` | header | no | Selects a Briefcase test plane; omission selects production |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Live authentication status (Cache-Control no-store) | `application/json` LoginStatus |
| default | Error | `application/json` Error |

## Report the API version and per-operation contract revisions

`GET /version` · `readApiVersion`

The compatibility document a client reads before its first real call. It names the API majors this build serves, the one selected for the caller, and every operation with the revision of its request and response shape.

A client advertises the majors it supports, newest first, in `Briefcase-Supported-API-Versions`. The server answers with the newest both sides support in `Briefcase-API-Version`, and `406` when there is none. `Vary` names the request header, so a cache never serves one client's selection to another.

An operation's `version` changes whenever its request or response shape changes observably; adding an operation leaves the others alone. A client that verifies its own operations against this list fails at startup rather than at the first call that no longer means what it did.

Also served unversioned at `/api/version`, so a client can negotiate before it knows which base path to use.

Authority: anonymous.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `Briefcase-Supported-API-Versions` | header | no | API majors the client supports, newest first, comma separated |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Compatibility document | `application/json` ApiVersionDocument |
| 406 | The client supports no API version this build serves | empty |
| default | Error | `application/json` Error |

## Exchange an IAM short-lived token for a Briefcase Application session

`POST /auth/slt` · `exchangeShortLivedToken`

Accepts only the single-use SLT obtained from IAM's hosted Application login. Briefcase supplies its own Application credential. When X-Briefcase-App-Secret is present, Briefcase uses only the mapped IAM testing-environment key and test-only Application credential. Preserve the same Idempotency-Key when recovering an uncertain result.

Authority: anonymous.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `Idempotency-Key` | header | yes | Stable key for recovering the same IAM token exchange |
| `X-Briefcase-App-Secret` | header | no | Selects a Briefcase test plane; omission selects production |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `slt` | string | yes |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Opaque access token, rotating refresh token, and the live organizations reachable by the IAM session | `application/json` ApplicationSessionTokens |
| 400 | Missing or invalid idempotency key or request body | empty |
| 401 | The SLT is invalid, expired, consumed, or belongs to another plane | empty |
| 503 | IAM or the configured testing plane is unavailable | empty |
| default | Error | `application/json` Error |

## Rotate an IAM Application refresh token

`POST /auth/refresh` · `refreshApplicationSession`

A successful refresh consumes the presented token and returns its replacement. Serialize refreshes per token family. Retry an uncertain result only with the exact same token, Idempotency-Key, and testing environment selection; rejected refreshes are terminal.

Authority: anonymous.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `Idempotency-Key` | header | yes | Stable key for recovering the same IAM token exchange |
| `X-Briefcase-App-Secret` | header | no | Selects a Briefcase test plane; omission selects production |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `refresh_token` | string | yes |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Rotated access and refresh tokens, including the live organizations reachable by the IAM session | `application/json` ApplicationSessionTokens |
| 400 | Missing or invalid idempotency key or request body | empty |
| 401 | The refresh token is invalid, consumed, revoked, or belongs to another plane | empty |
| 503 | IAM or the configured testing plane is unavailable | empty |
| default | Error | `application/json` Error |

## List Briefcase testing environments owned by an organization

`GET /organizations/{org_id}/testing-environments` · `listTestingEnvironments`

This lifecycle route always authenticates in production. The path organization must equal X-Org-ID. Active environments are returned by default; status=deleted or status=all changes that selection.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `org_id` | path | yes | string |
| `status` | query | no | string: active, deleted, all |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Matching environments; at most ten can be active | `application/json` TestingEnvironmentPage |
| default | Error | `application/json` Error |

## Create an empty Briefcase testing environment

`POST /organizations/{org_id}/testing-environments` · `createTestingEnvironment`

Provisions an IAM application testing environment through the official SDK and creates its empty Briefcase data plane. The IAM root credential and application secret are encrypted at rest. The returned key is the IAM test application secret (ask_ followed by 43 URL-safe characters), which selects this plane in X-Briefcase-App-Secret. At most ten environments may be active and each has a 2 GiB storage ceiling. Completed retries recover the original encrypted response.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `org_id` | path | yes | string |
| `Idempotency-Key` | header | yes | string |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `name` | string | yes |  |
| `description` | string / null | no |  |
| `iam_test_key` | IamTestingEnvironmentKeyValue | no |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 201 | Empty environment and its IAM test application secret (no-store) | `application/json` TestingEnvironmentWithKey |
| default | Error | `application/json` Error |

## Read one testing environment without revealing credentials

`GET /organizations/{org_id}/testing-environments/{environment_id}` · `getTestingEnvironment`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `org_id` | path | yes | string |
| `environment_id` | path | yes | string (uuid) |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Environment metadata | `application/json` TestingEnvironment |
| default | Error | `application/json` Error |

## Rename or re-describe an active testing environment

`PATCH /organizations/{org_id}/testing-environments/{environment_id}` · `updateTestingEnvironment`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `org_id` | path | yes | string |
| `environment_id` | path | yes | string (uuid) |
| `Idempotency-Key` | header | yes | string |
| `If-Match` | header | yes | Strong ETag containing the expected resource version |

Request: `application/merge-patch+json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `name` | string | no |  |
| `description` | string / null | no |  |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `name` | string | no |  |
| `description` | string / null | no |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Updated environment metadata | `application/json` TestingEnvironment |
| 409 | If-Match does not name the current resource version | empty |
| default | Error | `application/json` Error |

## Retire an environment with a two-day recovery window

`DELETE /organizations/{org_id}/testing-environments/{environment_id}` · `deleteTestingEnvironment`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `org_id` | path | yes | string |
| `environment_id` | path | yes | string (uuid) |
| `Idempotency-Key` | header | yes | string |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Soft-deleted environment, including its purge deadline | `application/json` TestingEnvironment |
| default | Error | `application/json` Error |

## Retrieve the selected IAM test application secret

`GET /organizations/{org_id}/testing-environments/{environment_id}/key` · `getTestingEnvironmentKey`

Restricted to the creator or a current organization admin/owner.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `org_id` | path | yes | string |
| `environment_id` | path | yes | string (uuid) |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Current key and generation | `application/json` TestingEnvironmentKey |
| default | Error | `application/json` Error |

## Replace every credential in the paired IAM testing plane

`POST /organizations/{org_id}/testing-environments/{environment_id}/iam-pairings` · `replaceTestingEnvironmentIamPairing`

Restricted to the creator or an organization administrator. Verify the complete replacement IAM pairing live. Replacing the app secret immediately replaces the Briefcase selector and advances the control/key generation. Existing initialized data cannot be rebound to a different IAM environment identity; create a new environment for that. Lifecycle changes wait for accepted work through the environment fence.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `org_id` | path | yes | string |
| `environment_id` | path | yes | string (uuid) |
| `Idempotency-Key` | header | yes | string |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `iam_environment_id` | string (uuid) | yes |  |
| `iam_environment_key` | IamTestingEnvironmentKeyValue | yes |  |
| `iam_app_id` | string | yes |  |
| `iam_app_secret` | string | yes |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Environment metadata with the replacement public pairing | `application/json` TestingEnvironment |
| default | Error | `application/json` Error |

## Erase an environment's isolated data while retaining its key

`POST /organizations/{org_id}/testing-environments/{environment_id}/cleanings` · `cleanTestingEnvironment`

Atomically erases Briefcase content, versions, permissions, logs, notifications, idempotency state, storage settings, and consumption. Exact provider deletion and multipart-abort descriptors are durably queued before source metadata is removed, then retried by the worker. The paired IAM directory projection is retained so existing test identities can use the empty environment immediately; deterministic roots are rebuilt on their next request.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `org_id` | path | yes | string |
| `environment_id` | path | yes | string (uuid) |
| `Idempotency-Key` | header | yes | string |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Cleaning result | `application/json` TestingEnvironmentCleaning |
| default | Error | `application/json` Error |

## Restore a retired environment before its purge deadline

`POST /organizations/{org_id}/testing-environments/{environment_id}/restorations` · `restoreTestingEnvironment`

Restore before the two-day purge deadline, provided the paired IAM environment and application credential remain valid. Returns the current IAM app secret and advances the control generation.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `org_id` | path | yes | string |
| `environment_id` | path | yes | string (uuid) |
| `Idempotency-Key` | header | yes | string |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Restored environment and its current IAM app secret (no-store) | `application/json` TestingEnvironmentWithKey |
| default | Error | `application/json` Error |

## Describe the environment selected by its IAM app secret

`GET /testing-environment` · `describeCurrentTestingEnvironment`

Authority: testingEnvironmentKey.

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Minimal environment metadata visible to a key holder | `application/json` TestingEnvironmentSelf |
| default | Error | `application/json` Error |

## Erase isolated Briefcase data using its test app secret

`POST /testing-environment/cleanings` · `cleanCurrentTestingEnvironment`

Uses only the selected environment root as authority and performs the same atomic Briefcase-state erasure as the production control-plane cleaning route while retaining the paired IAM identity projection. Provider deletion is durably queued and retried after logical erasure.

Authority: testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `Idempotency-Key` | header | yes | string |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Cleaning result | `application/json` TestingEnvironmentCleaning |
| default | Error | `application/json` Error |

## List folder contents, or filter everything the caller can reach

`GET /entries` · `listEntries`

Returns the hundred most recently changed visible entries per page, newest first. Without `filter`, the listing browses one level: the organization roots, or the contents of `parent_id`/`path`. With `filter` and no parent, it searches every entry the caller may reach, which is how a `location:` predicate selects a subtree.

The filter language combines freely. Terms are ANDed; `or` introduces an alternative; `not` or a leading `-` negates; parentheses group. Supported keys:

`last:N` / `first:N` take N entries chronologically (1-100, single page); `sort:newest` / `sort:oldest` order the result, newest first by default; `between:DD-MM-YYYY=DD-MM-YYYY` bounds the last change with both days inclusive; `after:DD-MM-YYYY` and `before:DD-MM-YYYY` bound one side; `from:@{carbon:id}` matches the creator; `to:@{silicon:id}` matches an explicit share; `for:@{id}` matches what that member can reach; `contains:'term'` matches names and extracted content, with `*` as a wildcard; `has:'term'` matches extracted content only; `name:'term'` matches names only; `location:'private/cos:tos'` matches a path prefix; `is:` takes `file`, `folder`, a renderer (`image`, `video`, `document`, `spreadsheet`, `presentation`, `audio`, `archive`, `code`, `unsupported`), or an extension such as `md`; `permissions:` takes `read`, `write`, `update`, `delete`, or `manage_permissions` and matches the caller's effective access. A bare word is shorthand for `contains:`.

Filtering only ever returns entries the caller can already see.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `parent_id` | query | no | Omit for the organization root |
| `path` | query | no | Parent folder addressed by path instead of identifier |
| `filter` | query | no | Filter expression, for example "last:5 location:'private' (contains:'apple' or contains:'cat') is:md" |
| `cursor` | query | no | string |
| `limit` | query | no | integer |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Visible entries | `application/json` EntryPage |
| default | Error | `application/json` Error |

## createFolder

`POST /entries` · `createFolder`

Without a parent, creates a user folder at the organization base with the required root_type access boundary (and IAM tag for type tag). The new folder is a sibling of the reserved containers, not redirected into one. With parent_id or parent_path, creates inside that folder and inherits its boundary. Existing folders are not relocated. Sibling name collisions return 409 without exposing hidden entry metadata.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `Idempotency-Key` | header | yes | string |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `name` | string | yes |  |
| `parent_id` | string / null (uuid) | no |  |
| `root_type` | string: public, private, tag | no | Required only when creating at organization root |
| `tag` | string | no | Required for a tag root |
| `invitees` | array of PermissionGrantCreate | no |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 201 | Folder created | `application/json` Entry |
| default | Error | `application/json` Error |

## getEntry

`GET /entries/{entry_id}` · `getEntry`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Entry metadata | `application/json` Entry |
| default | Error | `application/json` Error |

## updateEntry

`PATCH /entries/{entry_id}` · `updateEntry`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |
| `Idempotency-Key` | header | yes | string |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `name` | string | no |  |
| `parent_id` | string (uuid) | no |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Updated entry | `application/json` Entry |
| default | Error | `application/json` Error |

## moveEntryToBin

`DELETE /entries/{entry_id}` · `moveEntryToBin`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |

| Response | Meaning | Body |
| --- | --- | --- |
| 204 | Entry moved to the 45-day bin | empty |
| default | Error | `application/json` Error |

## Stream current file bytes for sandboxed in-place rendering

`GET /entries/{entry_id}/content` · `readEntryContent`

Briefcase relays the bytes itself, so the response is always bound to a current IAM identity and is never cacheable. Responses are sandboxed by Content-Security-Policy and served with `X-Content-Type-Options: nosniff`. Byte ranges are supported for media playback.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |
| `Range` | header | no | Single `bytes=` range. A malformed or multi-range value is ignored and the complete file is returned. |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Complete file bytes | `application/octet-stream` string (binary) |
| 206 | Requested byte range of the file | `application/octet-stream` string (binary) |
| default | Error | `application/json` Error |

## Download current file bytes as an attachment

`GET /entries/{entry_id}/download` · `downloadEntry`

Streams file bytes or a tar.zst folder archive. Folder archives include only entries the caller may read. No whole archive is buffered. Range requests apply to files only.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |
| `Range` | header | no | Single `bytes=` range. A malformed or multi-range value is ignored and the complete file is returned. |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Complete file bytes | `application/octet-stream` string (binary) |
| 206 | Requested byte range of the file | `application/octet-stream` string (binary) |
| default | Error | `application/json` Error |

## Upload a file of any supported size

`POST /uploads` · `uploadFile`

One request carries the whole file. Briefcase stages the bytes, then decides internally whether they fit a single provider request or need a multipart transfer, so a client never drives parts. Files up to 5 TiB are accepted; operators size the upload deadline and temporary disk for the sizes they expect.

The destination folder is named either by `parent_id` or by `path`, never both. A path is resolved from the organization base, so `private/cos:tos/reports` addresses the same folder its permanent URL shows.

Uploading a name that an active file already carries is how a file is updated: the bytes become that file's next version, the response is that same entry, and its history keeps the previous complete version history. Creating a file needs `write` on the folder; replacing one needs `update` on the file. A folder with the same name is a conflict.

Two organization limits apply: 100 GiB of uploads per UTC day, and 1 PiB of storage. The daily figure counts uploaded bytes and returns at midnight UTC; the storage figure counts what is currently kept, so deleting content returns capacity once the bytes are gone. Either limit may be configured per organization. An upload that does not fit is refused before its bytes are stored, and `GET /usage` reports both.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `Idempotency-Key` | header | yes | string |

Request: `multipart/form-data` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `parent_id` | string (uuid) | no | Destination folder identifier |
| `path` | string | no | Destination folder path, as an alternative to parent_id |
| `file` | string (binary) | yes |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 201 | File created, or a new version published on the existing file | `application/json` Entry |
| 413 | The file exceeds the 5 TiB maximum | empty |
| 429 | The organization's daily upload allowance is spent; Retry-After gives the seconds until it resets at 00:00 UTC | empty |
| 507 | The organization has no storage capacity left | empty |
| default | Error | `application/json` Error |

## listPermissions

`GET /entries/{entry_id}/permissions` · `listPermissions`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Explicit permissions | `application/json` object |
| default | Error | `application/json` Error |

## grantPermission

`POST /entries/{entry_id}/permissions` · `grantPermission`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `principal` | ActorRef | yes |  |
| `access` | array of string: read, write, update | yes |  |
| `inherit` | boolean | no |  Default: True. |

| Response | Meaning | Body |
| --- | --- | --- |
| 201 | Permission granted | `application/json` PermissionGrant |
| default | Error | `application/json` Error |

## revokePermission

`DELETE /entries/{entry_id}/permissions/{grant_id}` · `revokePermission`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |
| `grant_id` | path | yes | string (uuid) |

| Response | Meaning | Body |
| --- | --- | --- |
| 204 | Grant revoked | empty |
| default | Error | `application/json` Error |

## Report what the caller may do on named files and folders

`POST /permissions/effective` · `inspectEffectivePermissions`

Names up to 100 targets by identifier, path, or both. A target that does not exist and one the caller cannot read are both reported as unresolved, so the answer never confirms a hidden entry.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `entry_ids` | array of string (uuid) | no |  |
| `paths` | array of string | no |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Effective access per readable target | `application/json` PermissionInspectionResult |
| default | Error | `application/json` Error |

## searchFiles

`GET /search` · `searchFiles`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `q` | query | yes | string |
| `limit` | query | no | integer |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Permission-filtered results | `application/json` object |
| default | Error | `application/json` Error |

## Read the central notification inbox

`GET /notifications` · `listNotifications`

Returns the twenty newest notifications for the authenticated Carbon or Silicon together with the unread count used for the badge.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Newest notifications and unread badge count | `application/json` NotificationInbox |
| default | Error | `application/json` Error |

## Mark the entire notification inbox read

`POST /notifications/read` · `readNotifications`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Inbox after every notification was marked read | `application/json` NotificationInbox |
| default | Error | `application/json` Error |

## Read the retained action history of an entry

`GET /entries/{entry_id}/activity` · `listEntryActivity`

Returns up to the last hundred recorded actions on the entry — who created, read, updated, downloaded, deleted, or restored it, and when.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Newest-first action history | `application/json` ActivityPage |
| default | Error | `application/json` Error |

## Create a file for a member on behalf of another application

`POST /obo/files` · `createFileOnBehalfOfMember`

The compatible one-shot delegated file upload. The request body is the raw file bytes, and everything else — the destination folder path, the file name, and the media type — travels as metadata bound into the IAM OBO proof, so an application cannot redirect a proof it legitimately obtained to another destination.

Register the endpoint in IAM as `briefcase.files.create` at the path `/api/v1/obo/files` with metadata keys `path` (empty selects the application's own folder under `apps/{app_id}/private/{actor}`), `name`, and `content_type`. Exchange the proof over the SHA-256 digest of the exact bytes, then send those bytes here. Verification consumes the proof exactly once and is never retried.

The represented member's own permissions still apply: the file is only created where that member may add content, and a name that an active file already carries publishes that file's next version. Any supported size is accepted; Briefcase picks the storage route internally, and the organization's daily and total upload allowances apply as usual.

Authority: oboAccess + appId **or** oboAccess + appId + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | no | Optional; IAM derives the tenant and the header must agree with it |

Request: `application/octet-stream` (required).

| Response | Meaning | Body |
| --- | --- | --- |
| 201 | File created, or a new version published, for the represented member | `application/json` Entry |
| 413 | The file exceeds the 5 TiB maximum | empty |
| 429 | The organization's daily upload allowance is spent; Retry-After gives the seconds until it resets at 00:00 UTC | empty |
| 507 | The organization has no storage capacity left | empty |
| default | Error | `application/json` Error |

## Reserve private upload storage with current member authority

`POST /obo/uploads/reserve` · `reserveDelegatedUpload`

Register `briefcase.uploads.reserve` at `/api/v1/obo/uploads/reserve`. The exact destination, logical operation UUID, size and lowercase SHA-256 are immutable. A fresh proof binds POST and the exact JSON digest with empty metadata. A repeated reservation with unchanged intent can return its current state; a capability is issued only when a transfer can start. Store the non-secret operation UUID and manifest for reconciliation, never a parent token or proof. Capabilities do not authorize publication.

Authority: oboAccess + appId **or** oboAccess + appId + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | no | Optional; IAM derives the organization and any supplied value must agree |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `operation_id` | string (uuid) | yes | Stable non-nil logical upload UUID |
| `parent_path` | string | yes | Existing folder path; empty selects the private app folder |
| `name` | string | yes |  |
| `content_type` | string | yes |  |
| `size` | integer | yes |  |
| `sha256` | string | yes | Full raw-file digest |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Current authorized upload state | `application/json` DelegatedUploadReservation |
| 404 | Operation or destination is absent or not visible | empty |
| 409 | Logical intent or current lifecycle state conflicts | empty |
| default | Error | `application/json` Error |

## Publish previously staged bytes with fresh member authority

`POST /obo/uploads/commit` · `commitDelegatedUpload`

Register `briefcase.uploads.commit` at `/api/v1/obo/uploads/commit`. Mint a new proof after transfer, binding this small JSON body and empty metadata. Current IAM identity, membership, originating application, test generation, destination permission and quota are rechecked before atomic publication. The capability cannot commit. A response lost after publication is reconciled by the same logical operation UUID without creating another file version.

Authority: oboAccess + appId **or** oboAccess + appId + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | no | Optional; IAM derives the organization and any supplied value must agree |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `operation_id` | string (uuid) | yes |  |
| `upload_id` | string (uuid) | yes |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Current authorized upload state | `application/json` DelegatedUploadStatus |
| 404 | Operation or destination is absent or not visible | empty |
| 409 | Logical intent or current lifecycle state conflicts | empty |
| default | Error | `application/json` Error |

## Reconcile one logical upload with a fresh IAM proof

`POST /obo/uploads/status` · `getDelegatedUploadStatus`

Register `briefcase.uploads.status` at `/api/v1/obo/uploads/status`. Use a fresh proof for the exact JSON body and empty metadata after an uncertain response. This endpoint returns only the represented member and originating application's operation in the selected plane. It does not return a staging capability or read file bytes. A staged result can be committed with another fresh proof; a committed result carries its published entry UUID.

Authority: oboAccess + appId **or** oboAccess + appId + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | no | Optional; IAM derives the organization and any supplied value must agree |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `operation_id` | string (uuid) | yes |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Current authorized upload state | `application/json` DelegatedUploadStatus |
| 404 | Operation or destination is absent or not visible | empty |
| 409 | Logical intent or current lifecycle state conflicts | empty |
| default | Error | `application/json` Error |

## Cancel unpublished private staging

`POST /obo/uploads/cancel` · `cancelDelegatedUpload`

Register `briefcase.uploads.cancel` at `/api/v1/obo/uploads/cancel`. Use a fresh proof for the exact JSON body and empty metadata. Cancellation revokes further staging and publication, with durable provider cleanup. Cleanup may remain pending while a writer lease or ambiguous provider operation is reconciled. Cancellation never deletes a published file; use the separately authorized trash operation for that.

Authority: oboAccess + appId **or** oboAccess + appId + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | no | Optional; IAM derives the organization and any supplied value must agree |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `operation_id` | string (uuid) | yes |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Current authorized upload state | `application/json` DelegatedUploadStatus |
| 404 | Operation or destination is absent or not visible | empty |
| 409 | Logical intent or current lifecycle state conflicts | empty |
| default | Error | `application/json` Error |

## Transfer exact private bytes using a narrow upload capability

`PUT /obo/uploads/{upload_id}/content` · `transferDelegatedUpload`

Sends only the capability issued by the fresh-authorized reservation, the organization selector and the same test root when using a test plane. Do not send an IAM bearer, proof or application credential. Content-Length and the raw bytes must match the reserved size; the complete SHA-256 is checked before provider writes. A successful transfer is private staging only, not file publication. Use a fresh commit proof separately. A failed or uncertain transfer must be reconciled through the fresh-authorized status endpoint before retry.

Authority: uploadCapability **or** uploadCapability + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `upload_id` | path | yes | string (uuid) |
| `Content-Length` | header | yes | integer |

Request: `application/octet-stream` (required).

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Exact bytes are privately staged | `application/json` DelegatedUploadStatus |
| 401 | Invalid or revoked upload capability | empty |
| 409 | Transfer cannot start in the current lifecycle state | empty |
| 413 | Content exceeds the reservation or maximum size | empty |
| default | Error | `application/json` Error |

## Create a child folder for a represented member

`POST /obo/folders/create` · `createFolderOnBehalfOfMember`

Register `briefcase.folders.create` at `/api/v1/obo/folders/create` with an empty IAM metadata schema. Mint a fresh proof for POST and the SHA-256 of the exact JSON body. No bearer credential accompanies the proof. Every input, including the logical operation UUID, is body-bound; query parameters and Idempotency-Key do not select them. An empty parent_path selects the represented member's private folder for the originating application. Otherwise the path must resolve to an existing folder. Ordinary write permission is required, and the created folder retains its originating application for auditing. Retry an uncertain result with the same operation_id and unchanged input, but a fresh proof. Authorization is checked again before replay.

Authority: oboAccess + appId **or** oboAccess + appId + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | no | Optional; IAM derives the organization and any supplied value must agree |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `operation_id` | DelegatedOperationId | yes |  |
| `parent_path` | string | yes | Existing parent path; the empty string selects the private application folder |
| `name` | string | yes | One valid child folder name |

| Response | Meaning | Body |
| --- | --- | --- |
| 201 | Created folder or the same logical operation's authorized replay | `application/json` Entry |
| 404 | Parent is absent or not visible | empty |
| 409 | Name or logical operation conflicts with existing state | empty |
| default | Error | `application/json` Error |

## List the represented member's visible entries

`POST /obo/entries/list` · `listEntriesOnBehalfOfMember`

Register `briefcase.entries.list` at `/api/v1/obo/entries/list` with an empty IAM metadata schema. Each page requires a fresh proof bound to POST and the exact JSON body digest. The ordinary entry-list semantics apply, including traversal-only folders, privacy-filtered results, filters and opaque pagination. A parent can be addressed by UUID or path, never both. Omission lists roots, or searches the visible tree when a filter is present. No query parameter changes the request.

Authority: oboAccess + appId **or** oboAccess + appId + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | no | Optional; IAM derives the organization and any supplied value must agree |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `parent_id` | string / null (uuid) | no |  |
| `path` | string / null | no | Alternative parent path; must not be blank |
| `filter` | string / null | no | Ordinary permission-filtered entry expression |
| `cursor` | string / null | no |  |
| `limit` | integer / null | no |  Default: 100. |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Permission-filtered entries and optional next cursor | `application/json` EntryPage |
| 404 | Parent is absent or not visible | empty |
| default | Error | `application/json` Error |

## Stream a file for a represented member

`POST /obo/files/read` · `readFileOnBehalfOfMember`

Register `briefcase.files.read` at `/api/v1/obo/files/read` with an empty IAM metadata schema. Mint a fresh proof for POST and the exact JSON body digest. Ordinary read/download authorization applies. Only the body-bound range and download flag control delivery; an HTTP Range header or query parameter cannot override them. Inline content uses the normal sandbox CSP, nosniff and private no-store response headers. Attachment content is served as application/octet-stream. The proof's authority applies only to this file request and is never cached.

Authority: oboAccess + appId **or** oboAccess + appId + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | no | Optional; IAM derives the organization and any supplied value must agree |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `entry_id` | string (uuid) | yes |  |
| `range` | string / null | no | Single bytes= range; malformed or multi-range syntax falls back to the complete file just as ordinary reads do |
| `download` | boolean | no |  Default: False. |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Complete file bytes | `application/octet-stream` string (binary) |
| 206 | Requested byte range of the file | `application/octet-stream` string (binary) |
| 404 | File is absent or unreadable | empty |
| 416 | The bound byte range is unsatisfiable | empty |
| default | Error | `application/json` Error |

## Move an application-created entry to the recoverable bin

`POST /obo/entries/trash` · `trashEntryOnBehalfOfMember`

Register `briefcase.entries.trash` at `/api/v1/obo/entries/trash` with an empty IAM metadata schema. Mint a fresh proof for POST and the SHA-256 of the exact JSON body. The represented member must currently have delete authority across the subtree, and the originating application may delete only entries it created. System containers cannot be deleted. Retry an uncertain result with the same operation_id and entry_id but a fresh proof; current authority is rechecked before recognizing the same completed deletion. A successful old key cannot delete an entry that has since been restored. This is recoverable deletion with normal retention, not permanent removal.

Authority: oboAccess + appId **or** oboAccess + appId + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | no | Optional; IAM derives the organization and any supplied value must agree |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `operation_id` | DelegatedOperationId | yes |  |
| `entry_id` | string (uuid) | yes |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 204 | Entry moved to the bin or the same deletion was recognized | empty |
| 404 | Entry is absent or not visible | empty |
| 409 | Logical operation or current entry state conflicts | empty |
| default | Error | `application/json` Error |

## listVersions

`GET /entries/{entry_id}/versions` · `listVersions`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |
| `cursor` | query | no | string |
| `limit` | query | no | integer |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Newest-first page of at most 100 retained immutable versions | `application/json` FileVersionPage |
| default | Error | `application/json` Error |

## restoreVersion

`POST /entries/{entry_id}/versions/{version_id}/restore` · `restoreVersion`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |
| `version_id` | path | yes | string |
| `Idempotency-Key` | header | yes | string |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Version restored as a new current version | `application/json` Entry |
| default | Error | `application/json` Error |

## Report the organization's consumption and limits, in bytes

`GET /usage` · `readOrganizationUsage`

Answers how much space the organization is actually consuming — exact byte counts, never percentages, so a client renders whichever unit or proportion it prefers.

`storage.used_bytes` is what every retained version currently weighs, including versions of entries sitting in the bin, because those bytes are still stored. It falls as soon as content is really deleted. `daily_uploads.used_bytes` is what has been uploaded since the last midnight UTC, and `resets_at` is the next one.

Limits default to 100 GiB of uploads per UTC day and 1 PiB of storage, and either may be raised or lowered per organization, so the figures reported here are the ones actually in force.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Consumption and limits | `application/json` OrganizationUsage |
| default | Error | `application/json` Error |

## List recoverable entries, newest deletion first

`GET /bin` · `listBin`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `cursor` | query | no | string |
| `limit` | query | no | integer |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Deleted entries visible to the actor | `application/json` EntryPage |
| default | Error | `application/json` Error |

## restoreEntry

`POST /bin/{entry_id}/restore` · `restoreEntry`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Restored entry | `application/json` Entry |
| default | Error | `application/json` Error |

## configureOrganizationBucket

`PUT /storage/configuration` · `configureOrganizationBucket`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `Idempotency-Key` | header | no | Retain with the exact configuration to recover a lost response. A completed failed probe is replayed; use a new key to run validation again. |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `bucket_name` | string | yes |  |
| `region` | string | yes |  |
| `role_arn` | string | yes |  |
| `prefix` | string | yes |  |
| `aws_account_id` | string | yes |  |
| `encryption_mode` | string: sse_s3, sse_kms | yes |  |
| `kms_key_arn` | string | no |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Configuration saved after CRUD validation | `application/json` BucketConfigurationStatus |
| default | Error | `application/json` Error |

## Resolve the clean permanent URL of an entry

`GET /org/{org_id}/{path}` · `resolvePermanentUrl`

The path segment mirrors the folder structure exactly, for example `/org/tos/private/cos:tos/top_secret/this_secret.md`. The organization segment must match `X-Org-ID`. Without `disposition` the entry and its effective access are returned; with `disposition` the file bytes are streamed. Anything the caller cannot read answers `404`.

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `org_id` | path | yes | string |
| `path` | path | yes | Organization-relative entry path |
| `disposition` | query | no | Omit to receive entry metadata |
| `Range` | header | no | Single `bytes=` range. A malformed or multi-range value is ignored and the complete file is returned. |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Entry metadata, or complete file bytes when a disposition is requested | `application/json` Entry, `application/octet-stream` string (binary) |
| 206 | Requested byte range of the file | `application/octet-stream` string (binary) |
| default | Error | `application/json` Error |

## List current member and dynamic tag invitations

`GET /entries/{entry_id}/invitations` · `listInvitations`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |
| `cursor` | query | no | Opaque continuation from next_cursor; pages contain at most 100 invitations. |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Success | `application/json` InvitationPage |
| default | Error | `application/json` Error |

## Invite a current member by ID, verified email, or IAM tag

`POST /entries/{entry_id}/invitations` · `createInvitation`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |
| `Idempotency-Key` | header | yes | string |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `principal` | InvitationRecipient | yes |  |
| `access` | array of string: read, write, update | no |  Default: ['read']. |
| `inherit` | boolean | no |  Default: True. |

| Response | Meaning | Body |
| --- | --- | --- |
| 201 | Success | `application/json` Invitation |
| default | Error | `application/json` Error |

## Revoke a member or tag invitation

`DELETE /entries/{entry_id}/invitations/{grant_id}` · `revokeInvitation`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |
| `grant_id` | path | yes | string (uuid) |
| `Idempotency-Key` | header | yes | string |

| Response | Meaning | Body |
| --- | --- | --- |
| 204 | Success | empty |
| default | Error | `application/json` Error |

## Read explicit and inherited anonymous access

`GET /entries/{entry_id}/link-access` · `readLinkAccess`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Success | `application/json` LinkAccess |
| default | Error | `application/json` Error |

## Set anyone-with-link read/download access

`PUT /entries/{entry_id}/link-access` · `setLinkAccess`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |
| `Idempotency-Key` | header | yes | string |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `enabled` | boolean | yes |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Success | `application/json` LinkAccess |
| default | Error | `application/json` Error |

## Read 365 days of file/folder logs, newest first

`GET /entries/{entry_id}/logs` · `listEntryLogs`

Authority: bearerAuth **or** bearerAuth + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | yes | string |
| `entry_id` | path | yes | string (uuid) |
| `cursor` | query | no | string |
| `limit` | query | no | integer |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Success | `application/json` LogPage |
| default | Error | `application/json` Error |

## Read or download a publicly shared file or folder

`GET /public/{org_id}/{path}` · `readPublicEntry`

Authority: anonymous.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `org_id` | path | yes | string |
| `path` | path | yes | string |
| `view` | query | no | string: metadata, contents, inline, attachment |
| `cursor` | query | no | string |
| `X-Briefcase-App-Secret` | header | no | Selects a Briefcase test plane; omission selects production |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Success | `application/json` object, `application/octet-stream` string (binary), `application/zstd` string (binary) |
| default | Error | `application/json` Error |

## Critical IAM-approved operation inside the calling app subtree

`POST /obo/invitations` · `inviteOnBehalfOfMember`

IAM binds POST, this exact versioned path, the exact body SHA-256, subject, organization, and calling application. Require critical user approval in the IAM endpoint catalog.

Authority: oboAccess + appId **or** oboAccess + appId + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | no | Optional; IAM derives the organization and any supplied value must agree |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `operation_id` | string (uuid) | yes |  |
| `entry_id` | string (uuid) | yes |  |
| `invitation` | InvitationCreate | yes |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Success | `application/json` Invitation |
| default | Error | `application/json` Error |

## Critical IAM-approved operation inside the calling app subtree

`POST /obo/link-access` · `setLinkAccessOnBehalfOfMember`

IAM binds POST, this exact versioned path, the exact body SHA-256, subject, organization, and calling application. Require critical user approval in the IAM endpoint catalog.

Authority: oboAccess + appId **or** oboAccess + appId + testingEnvironmentKey.

| Parameter | Location | Required | Meaning |
| --- | --- | --- | --- |
| `X-Org-ID` | header | no | Optional; IAM derives the organization and any supplied value must agree |

Request: `application/json` (required).

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `operation_id` | string (uuid) | yes |  |
| `entry_id` | string (uuid) | yes |  |
| `enabled` | boolean | yes |  |

| Response | Meaning | Body |
| --- | --- | --- |
| 200 | Success | `application/json` LinkAccess |
| default | Error | `application/json` Error |
