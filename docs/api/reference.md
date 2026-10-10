# HTTP operation reference

Generated from the current [OpenAPI contract](../../openapi.yaml). API namespace `/api/v1`, contract 4.0.0. [Authentication and conventions](README.md) apply. Schemas in OpenAPI are authoritative.

## Report the API version and per-operation contract revisions

`GET /version` · `readApiVersion`

The compatibility document a client reads before its first real call. It names the API majors this build serves, the one selected for the caller, and every operation with the revision of its request and response shape.

A client advertises the majors it supports, newest first, in `Briefcase-Supported-API-Versions`. The server answers with the newest both sides support in `Briefcase-API-Version`, and `406` when there is none. `Vary` names the request header, so a cache never serves one client's selection to another.

An operation's `version` changes whenever its request or response shape changes observably; adding an operation leaves the others alone. A client that verifies its own operations against this list fails at startup rather than at the first call that no longer means what it did.

Also served unversioned at `/api/version`, so a client can negotiate before it knows which base path to use.

| Parameter | Location | Required |
| --- | --- | --- |
| `Briefcase-Supported-API-Versions` | header | no |

| Response | Meaning |
| --- | --- |
| 200 | Compatibility document |
| 406 | The client supports no API version this build serves |
| default | Error |

## Describe how to sign in to Briefcase with Silicon Accounts

`GET /accounts` · `readAccountsInfo`

Public, so a client can find Briefcase's app id and the Silicon Accounts origin before it holds a token. Carbons sign in at Silicon Accounts (hosted pages for the web, the device flow for the CLI); Silicons exchange a short-lived token from `silicon-accounts login --app briefcase`. Either way Briefcase receives a Silicon Accounts access token whose audience is `briefcase`. The response also lists the scopes another app can hold in a User verification proof to act for an account here.

| Response | Meaning |
| --- | --- |
| 200 | Sign-in facts (Cache-Control no-store) |
| default | Error |

## Read the signed-in Carbon or Silicon as Briefcase knows it

`GET /me` · `readAccount`

Verifies the access token and returns the account's uuid (the permanent key), its current public id, kind, display name and photo, and a Silicon's custodian. A token issued before the account signed out, removed Briefcase's access or was deleted answers 401 with the exact reason.

| Response | Meaning |
| --- | --- |
| 200 | The signed-in account (Cache-Control no-store) |
| default | Error |

## List the drives the caller opens with the owner's rights

`GET /drives` · `listDrives`

Every Carbon and Silicon has one drive. The list holds the caller's own drive and, for a Carbon, the drive of every Silicon it looks after, where it holds the owner's rights (it never acts as the Silicon). Each item names the owner, how the caller relates to it (`own` or `custodial`), the drive's root folder and its permanent URL. What others shared with the caller is listed by `GET /shared`.

| Response | Meaning |
| --- | --- |
| 200 | The caller's drives (Cache-Control no-store) |
| default | Error |

## List what other accounts shared with the caller

`GET /shared` · `listSharedWithMe`

Newest share first, one item per shared entry, each with the account whose drive holds it so a client can group by owner. Only live shares on entries the caller can still see are listed. A shared entry opens by its id like any other.

| Parameter | Location | Required |
| --- | --- | --- |
| `cursor` | query | no |
| `limit` | query | no |

| Response | Meaning |
| --- | --- |
| 200 | Entries shared with the caller (Cache-Control no-store) |
| default | Error |

## List who may share with a Silicon besides its custodian and the custodian's other Silicons

`GET /silicons/{silicon}/allowed-accounts` · `listSiliconAllowedAccounts`

A Silicon acts on what it receives, so it accepts shares only from its custodian, the other Silicons its custodian looks after, and the accounts listed here. Only the Silicon itself and its custodian may read the list (403 `not_silicons_custodian`); naming a Carbon is 422 `not_a_silicon`.

| Response | Meaning |
| --- | --- |
| 200 | The accounts allowed to share with the Silicon, oldest first (Cache-Control no-store) |
| default | Error |

## Let an account share with a Silicon

`PUT /silicons/{silicon}/allowed-accounts/{account}` · `allowAccountToShareWithSilicon`

Only the Silicon itself or its custodian may allow (403 `not_silicons_custodian`). Allowing the Silicon itself is 422 `allowance_self`.

| Response | Meaning |
| --- | --- |
| 201 | The account may now share with the Silicon |
| 204 | The account was already allowed; nothing changed |
| default | Error |

## Stop letting an account share with a Silicon

`DELETE /silicons/{silicon}/allowed-accounts/{account}` · `disallowAccountToShareWithSilicon`

Shares the account already made stay; the Silicon or its custodian can revoke them one by one. 404 when the account was not allowed.

| Response | Meaning |
| --- | --- |
| 204 | The account may no longer share with the Silicon |
| default | Error |

## List folder contents, the top of a drive, or a filtered view of a drive

`GET /entries` · `listEntries`

Returns the hundred most recently changed visible entries per page, newest first. Every Carbon and Silicon has one drive; `drive` names which (its owner's `c:`/`si:` id or uuid), defaulting to the caller's own, and is ignored with `parent_id` because a folder names its drive. Without `filter`, the listing browses one level: the top of the drive, or the contents of `parent_id`/`path`. With `filter` and no parent, it searches every entry in the drive the caller may reach, which is how a `location:` predicate selects a subtree. In another account's drive the caller sees only what was shared with it and what it (or a Silicon it looks after) created there.

The filter language combines freely. Terms are ANDed; `or` introduces an alternative; `not` or a leading `-` negates; parentheses group. Supported keys:

`last:N` / `first:N` take N entries chronologically (1-100, single page); `sort:newest` / `sort:oldest` order the result, newest first by default; `between:DD-MM-YYYY=DD-MM-YYYY` bounds the last change with both days inclusive; `after:DD-MM-YYYY` and `before:DD-MM-YYYY` bound one side; `from:@c:ada` matches the creator; `to:@si:scout` matches an explicit share; `for:@c:ada` matches what that account can reach (ids are resolved at Silicon Accounts to the account they name now; an unknown id is 422 `unknown_account`); `contains:'term'` matches names and extracted content, with `*` as a wildcard; `has:'term'` matches extracted content only; `name:'term'` matches names only; `location:'notes/2026'` matches a path prefix; `is:` takes `file`, `folder`, a renderer (`image`, `video`, `document`, `spreadsheet`, `presentation`, `audio`, `archive`, `code`, `unsupported`), or an extension such as `md`; `permissions:` takes `read`, `write`, `update`, `delete`, or `manage_permissions` and matches the caller's effective access. A bare word is shorthand for `contains:`.

Filtering only ever returns entries the caller can already see.

| Parameter | Location | Required |
| --- | --- | --- |
| `parent_id` | query | no |
| `path` | query | no |
| `drive` | query | no |
| `filter` | query | no |
| `cursor` | query | no |
| `limit` | query | no |

| Response | Meaning |
| --- | --- |
| 200 | Visible entries |
| default | Error |

## createFolder

`POST /entries` · `createFolder`

Creates a folder in `parent_id` or `parent_path`, or at the top of a drive when neither is given. `drive` names the drive for `parent_path` or the top (its owner's `c:`/`si:` id or uuid; the caller's own by default) and is refused with `parent_id`. Only the drive's owner, and the custodian of a Silicon that owns it, may add at the top; inside a folder `write` is needed. `apps` at the top of a drive is kept for apps (422 `reserved_name`). There is no Public space and there are no tags any more, so `root_type` other than `private` is 422 `public_space_retired` or `tags_retired`, and `tag` is `tags_retired`. `invitees` follow the sharing rules of `POST /entries/{entry_id}/invitations`. Sibling name collisions return 409 without exposing hidden entry metadata.

| Parameter | Location | Required |
| --- | --- | --- |
| `Idempotency-Key` | header | yes |

| Response | Meaning |
| --- | --- |
| 201 | Folder created |
| default | Error |

## getEntry

`GET /entries/{entry_id}` · `getEntry`



| Response | Meaning |
| --- | --- |
| 200 | Entry metadata |
| default | Error |

## updateEntry

`PATCH /entries/{entry_id}` · `updateEntry`

Renames an entry and/or moves it to another folder of the same drive. Entries move only within the drive that holds them; a destination folder the caller can see in another drive is 422 `cross_drive_move` (download and upload instead), and one it cannot see answers 404.

| Parameter | Location | Required |
| --- | --- | --- |
| `Idempotency-Key` | header | yes |

| Response | Meaning |
| --- | --- |
| 200 | Updated entry |
| default | Error |

## moveEntryToBin

`DELETE /entries/{entry_id}` · `moveEntryToBin`

The entry's creator, the drive's owner, and the custodian of either may move an entry to the bin. Delete is never shared.

| Response | Meaning |
| --- | --- |
| 204 | Entry moved to the 45-day bin |
| default | Error |

## Stream current file bytes for sandboxed in-place rendering

`GET /entries/{entry_id}/content` · `readEntryContent`

Briefcase relays the bytes itself, so the response is always bound to a current verified account and is never cacheable. Responses are sandboxed by Content-Security-Policy and served with `X-Content-Type-Options: nosniff`. Byte ranges are supported for media playback.

| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |
| `Range` | header | no |

| Response | Meaning |
| --- | --- |
| 200 | Complete file bytes |
| 206 | Requested byte range of the file |
| default | Error |

## Download current file bytes as an attachment

`GET /entries/{entry_id}/download` · `downloadEntry`

Streams file bytes or a tar.zst folder archive. Folder archives include only entries the caller may read. No whole archive is buffered. Range requests apply to files only.

| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |
| `Range` | header | no |

| Response | Meaning |
| --- | --- |
| 200 | Complete file bytes |
| 206 | Requested byte range of the file |
| default | Error |

## Upload a file of any supported size

`POST /uploads` · `uploadFile`

One request carries the whole file. Briefcase stages the bytes, then decides internally whether they fit a single provider request or need a multipart transfer, so a client never drives parts. Files up to 5 TiB are accepted; operators size the upload deadline and temporary disk for the sizes they expect.

The destination folder is named either by `parent_id` or by `path`, never both; with neither, the file goes at the top of the drive. A path is resolved within the drive `drive` names (its owner's `c:`/`si:` id or uuid; the caller's own by default), so `reports/q3` addresses the same folder its permanent URL shows. `drive` is refused with `parent_id`, because a folder names its drive.

Uploading a name that an active file already carries is how a file is updated: the bytes become that file's next version, the response is that same entry, and its history keeps the previous complete version history. Creating a file needs `write` on the folder; replacing one needs `update` on the file. A folder with the same name is a conflict.

Two limits apply to the drive the file lands in, whoever uploads it: 100 GiB of uploads per UTC day, and 1 PiB of storage. The daily figure counts uploaded bytes and returns at midnight UTC; the storage figure counts what is currently kept, so deleting content returns capacity once the bytes are gone. Either limit may be configured per drive. An upload that does not fit is refused before its bytes are stored, and `GET /usage` reports both.

`self_destruct_minutes` makes a new file self-destruct: 1 to 43,200 minutes (30 days) after the upload finishes, the file is permanently deleted. It never goes to the bin, and deleting it by hand before then is permanent too. It can only be chosen when the upload creates a file; naming an existing file is refused with `self_destruct_requires_new_file`, and a later version never changes the timer. The creator, the drive's owner, and the custodian of either can keep the file with `DELETE /entries/{entry_id}/self-destruct`.

| Parameter | Location | Required |
| --- | --- | --- |
| `Idempotency-Key` | header | yes |

| Response | Meaning |
| --- | --- |
| 201 | File created, or a new version published on the existing file |
| 413 | The file exceeds the 5 TiB maximum |
| 429 | The drive's daily upload allowance is spent; Retry-After gives the seconds until it resets at 00:00 UTC |
| 507 | The drive has no storage capacity left |
| default | Error |

## listPermissions

`GET /entries/{entry_id}/permissions` · `listPermissions`



| Response | Meaning |
| --- | --- |
| 200 | Explicit permissions |
| default | Error |

## grantPermission

`POST /entries/{entry_id}/permissions` · `grantPermission`

Anyone signed in to Briefcase can share with a Carbon. A Silicon accepts shares only from its custodian, the other Silicons its custodian looks after, and accounts it or its custodian allowed with `PUT /silicons/{silicon}/allowed-accounts/{account}`; anyone else is refused with 403 `silicon_not_open`. Only the entry's creator, the drive's owner, and the custodian of either may share. The recipient is named by `c:`/`si:` id (resolved at Silicon Accounts; 422 `unknown_account` or `account_inactive` otherwise), stored by uuid, and gets a notification in its inbox (and an email when it shared its email with Briefcase).

| Response | Meaning |
| --- | --- |
| 201 | Permission granted |
| default | Error |

## revokePermission

`DELETE /entries/{entry_id}/permissions/{grant_id}` · `revokePermission`



| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |
| `grant_id` | path | yes |

| Response | Meaning |
| --- | --- |
| 204 | Grant revoked |
| default | Error |

## Report what the caller may do on named files and folders

`POST /permissions/effective` · `inspectEffectivePermissions`

Names up to 100 targets by identifier, path, or both. Identifiers may name entries in any drive; each is answered inside its own drive. Paths are resolved within `drive` (its owner's `c:`/`si:` id or uuid; the caller's own by default). A target that does not exist and one the caller cannot read are both reported as unresolved, so the answer never confirms a hidden entry.

| Response | Meaning |
| --- | --- |
| 200 | Effective access per readable target |
| default | Error |

## searchFiles

`GET /search` · `searchFiles`



| Parameter | Location | Required |
| --- | --- | --- |
| `q` | query | yes |
| `limit` | query | no |

| Response | Meaning |
| --- | --- |
| 200 | Permission-filtered results |
| default | Error |

## Submit a bounded operational event

`POST /telemetry` · `submitTelemetry`

Best-effort anonymous relay to Space Station. No table key or credential is required. Events are marked client_reported and never confer authority. At most 100 events per second per process; duplicate IDs are suppressed for 60 seconds. A 204 acknowledges intake, not remote persistence. A disabled or unconfigured collector discards events.

| Response | Meaning |
| --- | --- |
| 204 | Event accepted for best-effort delivery or collection disabled |
| 429 | Process-wide telemetry intake limit reached |
| default | Error |

## Submit a bug report with an optional proposed fix

`POST /reports` · `submitReport`

Stores the account-authored report in the caller's drive. No telemetry, credentials, or files are collected automatically. Reuse the same idempotency key and body after an uncertain response.

| Parameter | Location | Required |
| --- | --- | --- |
| `Idempotency-Key` | header | yes |

| Response | Meaning |
| --- | --- |
| 200 | Durable report receipt, including on an idempotent replay |
| default | Error |

## Read the central notification inbox

`GET /notifications` · `listNotifications`

Returns the twenty newest notifications for the authenticated Carbon or Silicon together with the unread count used for the badge. The inbox is the account's own; each subject names the drive that holds its entry, usually someone else's.

| Response | Meaning |
| --- | --- |
| 200 | Newest notifications and unread badge count |
| default | Error |

## Mark the entire notification inbox read

`POST /notifications/read` · `readNotifications`



| Response | Meaning |
| --- | --- |
| 200 | Inbox after every notification was marked read |
| default | Error |

## Read the retained action history of an entry

`GET /entries/{entry_id}/activity` · `listEntryActivity`

Returns up to the last hundred recorded actions on the entry — who created, read, updated, downloaded, deleted, or restored it, and when.

| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |

| Response | Meaning |
| --- | --- |
| 200 | Newest-first action history |
| default | Error |

## Retired raw delegated upload

`POST /obo/files` · `createFileOnBehalfOfMember`

Returns HTTP 410 without reading upload bytes. Use the delegated reserve, capability transfer, and commit operations with a Silicon Accounts User verification proof.

| Response | Meaning |
| --- | --- |
| 410 | Raw delegated uploads are retired; use reserve, transfer, and commit |
| default | Error |

## Reserve private upload storage with current member authority

`POST /obo/uploads/reserve` · `reserveDelegatedUpload`

Reserve private storage for the account a User verification proof speaks for (scope briefcase.uploads.reserve). The exact destination, logical operation UUID, size and SHA-256 remain immutable. An unchanged retry returns current state and may rotate an idle capability. A capability cannot publish.

| Response | Meaning |
| --- | --- |
| 200 | Current authorized upload state |
| 404 | Operation or destination is absent or not visible |
| 409 | Logical intent or current lifecycle state conflicts |
| default | Error |

## Publish previously staged bytes with fresh member authority

`POST /obo/uploads/commit` · `commitDelegatedUpload`

Publish staged bytes with a valid User verification proof (scope briefcase.uploads.commit). The proof's account, issuing app, destination, resource permission and quota are verified before atomic publication. Repeating an unchanged logical operation does not create another file version.

| Response | Meaning |
| --- | --- |
| 200 | Current authorized upload state |
| 404 | Operation or destination is absent or not visible |
| 409 | Logical intent or current lifecycle state conflicts |
| default | Error |

## Reconcile one logical upload with a current proof

`POST /obo/uploads/status` · `getDelegatedUploadStatus`

Reconcile an upload with a valid proof for the same account and issuing app (scope briefcase.uploads.status). Status reveals no capability or file bytes. Destination write access is not required to reconcile an existing operation.

| Response | Meaning |
| --- | --- |
| 200 | Current authorized upload state |
| 404 | Operation or destination is absent or not visible |
| 409 | Logical intent or current lifecycle state conflicts |
| default | Error |

## Cancel unpublished private staging

`POST /obo/uploads/cancel` · `cancelDelegatedUpload`

Cancel unpublished content with a valid proof for the same account and issuing app (scope briefcase.uploads.cancel). Cancellation never deletes committed files and retains cleanup obligations until provider outcomes are known.

| Response | Meaning |
| --- | --- |
| 200 | Current authorized upload state |
| 404 | Operation or destination is absent or not visible |
| 409 | Logical intent or current lifecycle state conflicts |
| default | Error |

## Transfer exact private bytes using a narrow upload capability

`PUT /obo/uploads/{upload_id}/content` · `transferDelegatedUpload`

Sends only the capability issued by the reservation. Do not send an access token or proof; the drive is found from the reservation (older clients may still send X-Org-ID, which must then name it). Content-Length and the raw bytes must match the reserved size; the complete SHA-256 is checked before provider writes. A successful transfer is private staging only, not file publication. Commit separately with a valid proof. A failed or uncertain transfer must be reconciled through the fresh-authorized status endpoint before retry.

| Parameter | Location | Required |
| --- | --- | --- |
| `X-Org-ID` | header | no |
| `upload_id` | path | yes |
| `Content-Length` | header | yes |

| Response | Meaning |
| --- | --- |
| 200 | Exact bytes are privately staged |
| 401 | Invalid or revoked upload capability |
| 409 | Transfer cannot start in the current lifecycle state |
| 404 | No reservation has this upload id |
| 413 | Content exceeds the reservation or maximum size |
| default | Error |

## Create a child folder for a represented member

`POST /obo/folders/create` · `createFolderOnBehalfOfMember`

Create a child folder with a User verification proof (scope briefcase.folders.create). An empty parent_path selects the account's folder for the issuing app, `apps/{app}/`; a path names a folder in the account's own drive (inside `apps/{app}/` unless the app is first-party). Ordinary write permissions apply and the folder records the issuing app. Retry uncertain results with the unchanged manifest and operation_id; current authority is verified again.

| Response | Meaning |
| --- | --- |
| 201 | Created folder or the same logical operation's authorized replay |
| 404 | Parent is absent or not visible |
| 409 | Name or logical operation conflicts with existing state |
| default | Error |

## List the represented member's visible entries

`POST /obo/entries/list` · `listEntriesOnBehalfOfMember`

List the ordinary privacy-filtered entry view with a User verification proof (scope briefcase.entries.list). Parent UUID and path are mutually exclusive; with neither, the app's folder `apps/{app}/` is listed. Apps see only their folder in the account's own drive; a first-party app sees what the account sees and may name another drive with `drive` (others get 422 `drive_not_delegated`). Each page verifies the proof again and uses an explicit JSON cursor, filter and limit. An HTTP query parameter cannot change the manifest.

| Response | Meaning |
| --- | --- |
| 200 | Permission-filtered entries and optional next cursor |
| 404 | Parent is absent or not visible |
| default | Error |

## Stream a file for a represented member

`POST /obo/files/read` · `readFileOnBehalfOfMember`

Read a current file with a User verification proof (scope briefcase.files.read) and ordinary resource permissions (inside `apps/{app}/` of the account's drive unless the app is first-party). Range and download disposition come only from the JSON body. Inline content retains sandbox CSP, nosniff and private no-store headers. Every request verifies current authority.

| Response | Meaning |
| --- | --- |
| 200 | Complete file bytes |
| 206 | Requested byte range of the file |
| 404 | File is absent or unreadable |
| 416 | The bound byte range is unsatisfiable |
| default | Error |

## Move an application-created entry to the recoverable bin

`POST /obo/entries/trash` · `trashEntryOnBehalfOfMember`

Recoverably remove content the issuing app created with a User verification proof (scope briefcase.entries.trash). An entry another app (or the account itself) created answers 403 `not_created_by_app`. Current subtree delete permission and app namespace confinement apply. Retry the same operation_id and entry_id after an uncertain result. A completed key cannot delete a restored entry.

| Response | Meaning |
| --- | --- |
| 204 | Entry moved to the bin or the same deletion was recognized |
| 403 | The issuing app did not create this entry (`not_created_by_app`) |
| 404 | Entry is absent or not visible |
| 409 | Logical operation or current entry state conflicts |
| default | Error |

## listVersions

`GET /entries/{entry_id}/versions` · `listVersions`



| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |
| `cursor` | query | no |
| `limit` | query | no |

| Response | Meaning |
| --- | --- |
| 200 | Newest-first page of at most 100 retained immutable versions |
| default | Error |

## restoreVersion

`POST /entries/{entry_id}/versions/{version_id}/restore` · `restoreVersion`



| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |
| `version_id` | path | yes |
| `Idempotency-Key` | header | yes |

| Response | Meaning |
| --- | --- |
| 200 | Version restored as a new current version |
| default | Error |

## Report a drive's consumption and limits, in bytes

`GET /usage` · `readOrganizationUsage`

Answers how much space a drive is actually consuming — exact byte counts, never percentages, so a client renders whichever unit or proportion it prefers. The drive is the caller's own, or with `drive` that of a Silicon it looks after; only the drive's owner and that custodian may read it (404 otherwise).

`storage.used_bytes` is what every retained version currently weighs, including versions of entries sitting in the bin, because those bytes are still stored. It falls as soon as content is really deleted. `daily_uploads.used_bytes` is what has been uploaded since the last midnight UTC, and `resets_at` is the next one.

Limits default to 100 GiB of uploads per UTC day and 1 PiB of storage, and either may be raised or lowered per drive, so the figures reported here are the ones actually in force. Uploads are charged to the drive they land in, whoever uploads them.

| Parameter | Location | Required |
| --- | --- | --- |
| `drive` | query | no |

| Response | Meaning |
| --- | --- |
| 200 | Consumption and limits |
| default | Error |

## List a drive's recoverable entries, newest deletion first

`GET /bin` · `listBin`

The drive is the caller's own unless `drive` names another. Its owner and the custodian of a Silicon that owns it see everything in its bin; anyone else sees what they created there and what was shared with them.

| Parameter | Location | Required |
| --- | --- | --- |
| `cursor` | query | no |
| `limit` | query | no |
| `drive` | query | no |

| Response | Meaning |
| --- | --- |
| 200 | Deleted entries visible to the actor |
| default | Error |

## restoreEntry

`POST /bin/{entry_id}/restore` · `restoreEntry`



| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |

| Response | Meaning |
| --- | --- |
| 200 | Restored entry |
| default | Error |

## Configure a drive's bring-your-own S3 bucket

`PUT /storage/configuration` · `configureOrganizationBucket`

Configures storage for the caller's own drive, or with `drive` for a Silicon it looks after. Only the drive's owner and that custodian may (403 otherwise). Briefcase checks the bucket with a test upload, read, update and delete before it is used. The STS External ID Briefcase presents when it assumes the role is derived from the drive and stored with the configuration; name it in the role's trust policy.

| Parameter | Location | Required |
| --- | --- | --- |
| `Idempotency-Key` | header | no |

| Response | Meaning |
| --- | --- |
| 200 | Configuration saved after CRUD validation |
| default | Error |

## Resolve an entry's permanent address

`GET /paths/{owner}/{path}` · `resolvePermanentUrl`

The address mirrors the folder structure below the drive's owner, for example `/paths/c:ada/notes/2026/plan.md` for https://briefcase.teamofsilicons.com/c:ada/notes/2026/plan.md. The owner is a `c:`/`si:` id or the account's uuid (entries' `content_url` uses the uuid, so it survives id changes). Without `disposition` the entry and the caller's effective access are returned; with it the file bytes are streamed. Without `Authorization` only what anyone with the link may read is served. An id the owner gave up answers `308` with the address under its current id. Anything the caller cannot read answers `404`.

| Parameter | Location | Required |
| --- | --- | --- |
| `owner` | path | yes |
| `path` | path | yes |
| `disposition` | query | no |
| `Range` | header | no |

| Response | Meaning |
| --- | --- |
| 200 | Entry metadata, or complete file bytes when a disposition is requested |
| 206 | Requested byte range of the file |
| 308 | The owner gave up the id the address used; the same address under the owner's current id |
| default | Error |

## Resolve the top of a drive

`GET /paths/{owner}` · `resolveDriveTop`

The drive's own address (https://briefcase.teamofsilicons.com/c:ada). Its owner and the custodian of a Silicon that owns it see the top folder; someone with a share inside sees a traversal entry; anyone else gets `404`. The top of a drive never has a link, so this always takes an access token. An id the owner gave up answers `308`.

| Parameter | Location | Required |
| --- | --- | --- |
| `owner` | path | yes |

| Response | Meaning |
| --- | --- |
| 200 | The drive's top folder |
| 308 | The owner gave up the id the address used; the same address under the owner's current id |
| default | Error |

## Resolve an address from before personal drives

`GET /org/{org_id}/{path}` · `resolveLegacyPermanentUrl`

Addresses such as `/org/tos/public/handbook/guide.md` keep working after the move to personal drives. They resolve through the aliases the cutover recorded when it moved each file (an address below a moved folder resolves through the folder) and answer exactly like `GET /paths/{owner}/{path}` for the entry's current place, whose `permanent_url` the response carries. Without `Authorization` only link-shared entries are served.

| Parameter | Location | Required |
| --- | --- | --- |
| `org_id` | path | yes |
| `path` | path | yes |
| `disposition` | query | no |
| `Range` | header | no |

| Response | Meaning |
| --- | --- |
| 200 | Entry metadata, or complete file bytes when a disposition is requested |
| 206 | Requested byte range of the file |
| default | Error |

## List what an archive holds

`GET /entries/{entry_id}/archive` · `listArchiveContents`

Lists the entries of a zip, tar, tar.gz, tar.zst or 7z file without extracting anything, for anyone who may read the file. Zip archives are listed from their central directory whatever their size; tar archives are read as a stream (up to 1 GiB of archive); 7z archives up to 512 MiB. At most `limit` entries are returned; `truncated` says when there are more. Any other file answers 422 `not_an_archive`; an unreadable or encrypted archive answers 422 `archive_unreadable` or `archive_encrypted`.

| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |
| `limit` | query | no |

| Response | Meaning |
| --- | --- |
| 200 | The archive's entries |
| default | Error |

## List the live shares on an entry

`GET /entries/{entry_id}/invitations` · `listInvitations`



| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |
| `cursor` | query | no |

| Response | Meaning |
| --- | --- |
| 200 | Success |
| default | Error |

## Share with a Carbon or Silicon named by id

`POST /entries/{entry_id}/invitations` · `createInvitation`

Anyone signed in to Briefcase can share with a Carbon. A Silicon accepts shares only from its custodian, the other Silicons its custodian looks after, and accounts it or its custodian allowed with `PUT /silicons/{silicon}/allowed-accounts/{account}`; anyone else is refused with 403 `silicon_not_open`. Only the entry's creator, the drive's owner, and the custodian of either may share. The recipient is named by `c:`/`si:` id (resolved at Silicon Accounts; 422 `unknown_account` or `account_inactive` otherwise), stored by uuid, and gets a notification in its inbox (and an email when it shared its email with Briefcase). `principal.type` is `carbon` or `silicon`; `email` and `tag` recipients are retired (422 `email_recipients_retired`, `tag_recipients_retired`).

| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |
| `Idempotency-Key` | header | yes |

| Response | Meaning |
| --- | --- |
| 201 | Success |
| default | Error |

## Revoke a share (including one made to a tag before personal drives)

`DELETE /entries/{entry_id}/invitations/{grant_id}` · `revokeInvitation`



| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |
| `grant_id` | path | yes |
| `Idempotency-Key` | header | yes |

| Response | Meaning |
| --- | --- |
| 204 | Success. Ending an expiring share early sends no notification. |
| default | Error |

## Extend, shorten, or make permanent a live expiring share

`PATCH /entries/{entry_id}/invitations/{grant_id}` · `changeExpiringShare`

Send exactly one field. `expires_in_minutes` restarts the share's clock from now (1 to 43,200 minutes), which extends or shortens it. `permanent: true` keeps the access for good; when the recipient already holds a permanent grant on this entry, the expiring share folds into it and the response is that permanent grant. An expired share no longer exists (404); a permanent grant is `not_an_expiring_share` (409).

| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |
| `grant_id` | path | yes |
| `Idempotency-Key` | header | yes |

| Response | Meaning |
| --- | --- |
| 200 | The grant as it now stands |
| default | Error |

## Keep a self-destructing file

`DELETE /entries/{entry_id}/self-destruct` · `makeEntryPermanent`

Stops the file's self-destruct timer. Only the file's creator, the drive's owner, and the custodian of either may; anyone else who can see the file gets 403. A file whose timer is not running is `not_self_destructing` (409).

| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |

| Response | Meaning |
| --- | --- |
| 204 | The file is permanent. Repeating the call after success is `not_self_destructing` (409) unless it carries the same optional `Idempotency-Key`, which replays the success. |
| default | Error |

## Read explicit and inherited anonymous access

`GET /entries/{entry_id}/link-access` · `readLinkAccess`



| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |

| Response | Meaning |
| --- | --- |
| 200 | Success |
| default | Error |

## Set anyone-with-link read/download access

`PUT /entries/{entry_id}/link-access` · `setLinkAccess`

Managed by the entry's creator, the drive's owner, and the custodian of either. The top of a drive and its apps folders never get a link (422 `protected_folder`); anything inside them can.

| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |
| `Idempotency-Key` | header | yes |

| Response | Meaning |
| --- | --- |
| 200 | Success |
| default | Error |

## Read 365 days of file/folder logs, newest first

`GET /entries/{entry_id}/logs` · `listEntryLogs`



| Parameter | Location | Required |
| --- | --- | --- |
| `entry_id` | path | yes |
| `cursor` | query | no |
| `limit` | query | no |

| Response | Meaning |
| --- | --- |
| 200 | Success |
| default | Error |

## Read or download a publicly shared file or folder

`GET /public/{owner}/{path}` · `readPublicEntry`

What anyone with the link may read, at `/public/{owner}/{path}`. The owner is a `c:`/`si:` id or the account's uuid; the first segment of an address from before personal drives (`tos` in `/public/tos/…`) also works, through the cutover's aliases. An id the owner gave up answers `308` with the address under its current id.

| Parameter | Location | Required |
| --- | --- | --- |
| `owner` | path | yes |
| `path` | path | yes |
| `view` | query | no |
| `cursor` | query | no |

| Response | Meaning |
| --- | --- |
| 200 | Success |
| 308 | The owner gave up the id the address used; the same address under the owner's current id |
| default | Error |

## Share or link an entry for the account, inside the issuing app's folder

`POST /obo/invitations` · `inviteOnBehalfOfMember`

Requires a User verification proof with scope briefcase.invitations.create from an app allowed for it. Ordinary invitation permissions and logical idempotency apply.

| Response | Meaning |
| --- | --- |
| 200 | Success |
| default | Error |

## Share or link an entry for the account, inside the issuing app's folder

`POST /obo/link-access` · `setLinkAccessOnBehalfOfMember`

Requires a User verification proof with scope briefcase.link_access.update from an app allowed for it. Ordinary link-sharing permissions and logical idempotency apply.

| Response | Meaning |
| --- | --- |
| 200 | Success |
| default | Error |
