# Build an interface with Briefcase

An interface can use the same HTTP API as the Briefcase website to browse,
create, preview, download, organize, share and recover files, and manage the
signed-in account's organizations. No Briefcase website session or CLI is
required. Briefcase remains responsible for storage and authorization.

| Resource | Address |
| --- | --- |
| API base | `https://api.briefcase.teamofsilicons.com/api/v1` |
| Contract and operation revisions | `GET https://api.briefcase.teamofsilicons.com/api/version` |
| Machine-readable API | [OpenAPI](../openapi.yaml) |
| All supported operations | [Operation inventory](api/operations.md) and [reference](api/reference.md) |
| Sign-in provider | [Silicon Accounts](https://developers.teamofsilicons.com/docs/accounts/) |
| Organization and team rules | [Organizations](organizations.md) |

The API namespace is **v1**, the contract is **5.3.0**, and operations have their
own revisions. Send `Briefcase-Supported-API-Versions: v1`; an unsupported major
returns 406. Read `/api/version` before integration and compare the operations
you use. Package/build versions can change independently of the contract.

## Sign in and identify the selected account

Use the **access token issued for the `briefcase` app** by Silicon Accounts,
including when Accounts issues it through an Interface or Cell:

```http
GET /api/v1/me HTTP/1.1
Host: api.briefcase.teamofsilicons.com
Authorization: Bearer <briefcase-access-token>
Briefcase-Supported-API-Versions: v1
```

An illustrative response is:

```json
{
  "uuid": "00000000-0000-4000-8000-000000000001",
  "id": "c:saket.tos",
  "kind": "carbon",
  "display_name": "Saket",
  "pfp_url": null,
  "custodian": null
}
```

Briefcase verifies the JWT's signature, issuer, audience, expiry and not-before
time, and introspects it at Accounts on **every authenticated request**. It
requires `active: true`, `token_type: "access_token"`, `client_id: "briefcase"`
and the same account subject. Expiry has zero clock leeway. No successful
introspection result is cached. Revoked or expired tokens, refresh tokens,
Interface secrets/session capabilities, first-party Accounts tokens and other
apps' tokens cannot authorize Briefcase API calls. If Accounts cannot verify
access, the request fails closed.

`GET /accounts` is public discovery of the app ID and Accounts origin. Accounts
owns sign-in, account selection, refresh and sign-out. The interface keeps its
refresh credentials server-side and sends only the Briefcase access token here.
Never request Briefcase's app secret. Accounts profile scopes do not grant file
permissions or organization roles.

For a browser interface, put these calls behind your own server/BFF and secure
session. Stream content through that server for previews and downloads; an
`<img>` or `<a>` cannot attach a bearer header. Briefcase's API does not expose
cross-origin cookie authentication or a browser CORS contract. Its website's
`/api/*` proxy and login cookies are private implementation details.

## Personal and work accounts

After sign-in, fetch `GET /me`, `GET /drives` and `GET /organizations` using the
same token. Key application state by `/me.uuid`. Use the organization response
for current roles and teams; **do not infer authority from an ID suffix**.

A personal account with no organization membership returns:

```json
{
  "account_uuid": "00000000-0000-4000-8000-000000000001",
  "organizations": [],
  "groups": [],
  "share_targets": []
}
```

Anyone with a valid personal Carbon or Silicon account can use Briefcase. Its
files stay private until shared. A work account such as `c:saket.tos` gets the
current `tos` organization context when its authenticated UUID belongs to that
organization and its app policy allows Briefcase. A typical response includes
these fields (other capability flags are shown in OpenAPI):

```json
{
  "account_uuid": "00000000-0000-4000-8000-000000000002",
  "organizations": [{
    "org_id": "tos", "name": "Team of Silicons", "logo_url": null,
    "capabilities": {"owner": false, "admin": true, "manage_admins": false},
    "member_count": 8, "team_count": 2
  }],
  "groups": ["organization:tos", "team:tos/00000000-0000-4000-8000-000000000003"],
  "share_targets": [{
    "type": "team", "id": "tos/00000000-0000-4000-8000-000000000003",
    "name": "Design", "org_id": "tos"
  }]
}
```

Personal and work accounts have separate UUIDs and drives. Linked accounts
help with switching; they do not share authority or files. Clear the previous
account's cached data on switching. An empty organization list means this
session has no currently accessible membership; it never authorizes falling
back to another account. Organizations rechecks membership and app policy on
each request. On a dependency error show a retry state, rather than relabeling
a work account as personal.

`GET /drives` lists the account's own drive and permitted custodial Silicon
drives. Each item includes the root folder UUID as `root_id`. `GET /shared` lists accessible direct,
team and organization shares from other drives. Organization administration
does not by itself give access to all members' files. On legacy entry objects,
`org_id` names the **drive owner's UUID**, not the Organizations handle.

## Create, browse, upload and preview

The examples below use paths relative to the API base and a bearer token on
every request. UUIDs are illustrative; use IDs returned by earlier calls.

| Task | Request | Authority |
| --- | --- | --- |
| List drive root | `GET /entries?limit=100` | Own or permitted custodial drive |
| List a folder | `GET /entries?parent_id=FOLDER_UUID&limit=100` | Read folder; only visible children are returned |
| Read metadata | `GET /entries/ENTRY_UUID` | Read entry |
| Create folder | `POST /entries` | Own/custodial root, or `write` on destination folder |
| Upload a new file | `POST /uploads` with multipart `file` | `write` on destination folder |
| Replace file content | Same upload destination and filename | `update` on the existing file; creates a version |
| Preview / download | `GET /entries/FILE_UUID/content` or `/download` | Read file |
| Download a folder | `GET /entries/FOLDER_UUID/download` | Read; streams accessible content as tar.zst |
| Inspect archive | `GET /entries/FILE_UUID/archive` | Read; bounded listing |
| Rename / move | `PATCH /entries/ENTRY_UUID` | Applicable entry and destination rights |
| Search | `GET /search?q=roadmap&limit=20` | Results are filtered to current access |

Create a folder using a fresh `Idempotency-Key` of 8–255 printable ASCII
characters (UUIDs are convenient):

```http
POST /api/v1/entries
Authorization: Bearer <briefcase-access-token>
Content-Type: application/json
Idempotency-Key: 00000000-0000-4000-8000-000000000010

{"name":"Project plans"}
```

The 201 response is an `Entry`; retain its `id`, `path`, `url`, `type` and
`effective_access`. Lists return `{"items":[...],"next_cursor":null}` when
finished. For nested folders send `parent_id`, or `drive` plus `parent_path`;
do not send both addressing forms. Omit `drive` for the signed-in account.

From your backend, a multipart upload can be sent with standard fetch:

```js
// accessToken is the current app-specific token kept by your server.
const body = new FormData();
body.set("parent_id", folder.id);
body.set("file", new Blob(["Project roadmap\n"], { type: "text/plain" }), "roadmap.txt");
const response = await fetch("https://api.briefcase.teamofsilicons.com/api/v1/uploads", {
  method: "POST",
  headers: {
    Authorization: `Bearer ${accessToken}`,
    "Idempotency-Key": persistedUploadKey,
    "Briefcase-Supported-API-Versions": "v1"
  },
  body
});
if (!response.ok) throw await response.json();
const file = await response.json();
```

Let the multipart library set Content-Type and its boundary. For large files,
use a streaming multipart implementation or the [Rust client](client/README.md)
instead of buffering a Blob. Upload paths use the multipart field `path`
(folder creation uses JSON `parent_path`). `drive` cannot accompany `parent_id`.

Content and download responses support single byte ranges (`Range: bytes=0-1023`)
and return 206 when honored; an unsatisfiable range returns 416. Preserve
Content-Type, Content-Disposition, Content-Range and sandbox headers in your
proxy. Treat uploaded HTML/SVG and other active content as untrusted. Resolve
permanent paths with `GET /paths/{owner}/{path}`; URL-encode owner and path
segments. Account IDs can change, so retain immutable entry/account IDs.

## Share with people, teams and organizations

Use the entry's `effective_access` and `POST /permissions/effective` to present
available actions. Show sharing management only with `manage_permissions`.
The API checks authorization again when the action runs.

For team recipients, choose an exact object from `/organizations.share_targets`
and send its `type` and `id`. The team ID contains the organization handle and
immutable team UUID, separated by `/`. Never substitute the display name.

```http
POST /api/v1/entries/FOLDER_UUID/invitations
Authorization: Bearer <briefcase-access-token>
Content-Type: application/json
Idempotency-Key: 00000000-0000-4000-8000-000000000011

{
  "principal": {"type":"team","id":"tos/00000000-0000-4000-8000-000000000003"},
  "access": ["read","write","update"],
  "inherit": true
}
```

The 201 response contains a grant `id`, resolved `principal`, `access`,
`inherit` and `expires_at`. `read` permits viewing/downloading, `write` permits
adding children to a folder, and `update` permits editing. They are independent;
group grants never delegate deletion or sharing management. For a file omit
`write`. `inherit: true` applies folder access to descendants.

Use `{"type":"organization","id":"tos"}` for an organization, or
`{"type":"carbon","id":"c:ada.tos"}` / `{"type":"silicon","id":"si:scout"}`
for an account. Carbons and Silicons receive shares from any signed-in account by default. Settings can restrict new shares to the recipient’s saved contacts and custodian circle. See [sharing](sharing.md).

Current and future team members receive access. Removal from the team or
organization, team deletion, and current app exclusions remove group-derived
access on subsequent requests, including listing, search and download. Separate
direct grants still apply. Organization admins can discover all teams for
sharing; ordinary members can discover their own teams. An admin's ability to
select a team does not make the admin a member of that team.

| Task | Request |
| --- | --- |
| List shares | `GET /entries/ENTRY_UUID/invitations?cursor=...` |
| Revoke a share | `DELETE /entries/ENTRY_UUID/invitations/GRANT_UUID` |
| Expiring share | Create with `expires_in_minutes` from 1 to 43,200 and `access:["read"]` |
| Extend expiry / make permanent | `PATCH /entries/ENTRY_UUID/invitations/GRANT_UUID` with `{"expires_in_minutes":60}` / `{"permanent":true}` |
| Inspect public link | `GET /entries/ENTRY_UUID/link-access` |
| Enable / disable public link | `PUT /entries/ENTRY_UUID/link-access` with `{"enabled":true}` / `{"enabled":false}` |
| Expiring public link | Enable with `expires_in_minutes`; use the returned `url` |

Invitation mutations and link updates require an Idempotency-Key. Anonymous
links are independent read-only capabilities, enabled explicitly; authenticated
file URLs do not automatically become public. Public paths use
`GET /public/{owner}/{path}`. Never place a bearer token in a URL.

## Organization management inside your interface

`GET /organizations/{org}` returns role-filtered `org`, `members`, `teams`,
`invites` and `capabilities`. Use the current capabilities to display controls.
Owners have organization authority, admins follow the owner's delegated
switches, and team admins can manage their permitted teams. The server enforces
each action even if a client ignores these hints.

Send actions to `POST /organizations/{org}/actions`, with an Idempotency-Key
of 8–200 characters. Actions return current visible organization state,
with operation-specific fields such as `team` or `invite` when applicable.
Removing the acting member can instead return `{"left":true,"org_id":"tos"}`.

| Action | Body fields in addition to `action` | Role checks |
| --- | --- | --- |
| `update` | `name`, `logo_url`, `admin_team_permissions` | Admin; only owners set team permission switches |
| `team_create` | `name`, optional `description` | `team_structure` |
| `team_update` | `team`, optional `name`, `description`, `admin_team`, `settings` | Structure/settings authority; admin teams also require Manage admins |
| `team_delete` | `team` | Structure authority; admin-team restrictions also apply |
| `team_member_add`, `team_member_remove` | `team`, `member` (account UUID) | Team member-management authority; protected admins/owners cannot be bypassed |
| `team_admin_set` | `team`, `member`, `admin` (boolean) | Team admin-management authority; target must already be a member |
| `admin_set` | `member`, `admin`, optional `manage_admins` | Manage admins; only owners change Manage admins authority |
| `invite` | `target` (email or Carbon ID), `handle`, optional `teams`, `team_admins`, `admin`, `manage_admins`, `settings` | Authority for the requested teams and roles |
| `invite_cancel`, `invite_resend` | `invite` | Authority over that invitation |
| `member_remove` | `member` | Member-removal authority; ownership governance is protected |
| `member_update` | `member`, `settings` | Admin; member app allowances/exclusions |
| `policy_update` | `scope` (`org`, `team`, `member`), `settings`, plus `team` or `member` for that scope | Scope-specific authority |

For example, `{"action":"team_create","name":"Design"}` returns the new
team UUID in `team`. Refresh `/organizations` afterward to update sharing
targets. Policy settings use `mode` (`everywhere`, `only`, `except`), `apps`,
`extra_apps` and `excluded_apps`. Exclusions win. Team admins can only add
exclusions; they cannot grant themselves broader app access. All five
`admin_team_permissions` switches are `enabled`, `structure`, `members`,
`admins` and `settings`.

Organization creation/joining, invitation acceptance, account credentials and
ownership governance remain with [Organizations](https://organizations.teamofsilicons.com/)
and Accounts. Select the resulting work account through Accounts and repeat
the discovery calls; Briefcase automatically uses its verified membership.

## Remaining frontend features

| Feature | Endpoints | Permission / behavior |
| --- | --- | --- |
| Versions | `GET /entries/{id}/versions`, `POST /entries/{id}/versions/{version}/restore` | Read history; restoring needs update and publishes a new version |
| Bin | `DELETE /entries/{id}`, `GET /bin`, `POST /bin/{id}/restore` | Deletion/restoration follows creator, owner and custodian authority; ordinary files retained 45 days |
| Self-destructing files | Upload with `self_destruct_minutes`; `DELETE /entries/{id}/self-destruct` to keep | 1–43,200 minutes; permanent deletion bypasses bin; keeping requires owner/creator/custodian authority |
| Activity and sharing logs | `GET /entries/{id}/activity`, `GET /entries/{id}/logs` | Visible entry history; sharing administration rules apply to logs |
| Notifications | `GET /notifications`, `POST /notifications/read` | Caller's newest 20 notifications; POST with no body marks the whole inbox read |
| Storage and quotas | `GET /usage?drive=OWNER`, `PUT /storage/configuration` | Configure owned/custodial drives with the documented S3 bucket/role settings |
| Incoming shares | `GET` / `PUT /accounts/{account}/incoming-shares` | Account or its Silicon custodian |
| Legacy Silicon allowances | `GET /silicons/{silicon}/allowed-accounts`, `PUT` / `DELETE /silicons/{silicon}/allowed-accounts/{account}` | Silicon or its custodian |
| Low-level permissions | `GET` / `POST /entries/{id}/permissions`, `DELETE /entries/{id}/permissions/{grant}` | Account grants; use invitations for team/org recipients |
| Integration reports | `POST /reports` with `{"message":"Steps and request ID"}` | Signed-in account; Idempotency-Key required |

All these operations use the same Briefcase bearer token. The separate `/obo/*`
API uses scoped User verification proofs for delegated apps; it is not needed
to build a full interface and does not carry organization membership. See
[delegation](obo.md) when that narrower workflow is intended.

For restricted direct offers, handle HTTP 202 and `pending_review: true`. Use the account’s `share-review` endpoint to display pending items and allowed senders. The review decision can accept the item, allow the sender, or dismiss the offer.

## Pagination, retries, limits and updates

- Follow `next_cursor` until null/absent, keeping other filters fixed. Cursors
  are opaque and scoped to the query. Ordinary entry, shared, bin and version
  pages allow 1–100 items; invitation pages contain at most 100. Search accepts limits up to 20;
  notifications return the newest 20 without pagination. Organization snapshots are
  role-filtered objects rather than cursor pages.
- Persist a key per logical mutation **before sending** it. Retry the same
  route, body and key after a timeout or uncertain result. Authorization is
  rechecked on replay. Changing the body with the same key returns 409. A new
  key is a new operation. Operations without required keys are identified in
  OpenAPI; send one on supported mutations when an uncertain response matters.
- Retry network failures and 429/503 with bounded exponential backoff and
  jitter; honor `Retry-After` when present. A quota error may need a later
  quota reset or user action. Reconcile metadata after an uncertain upload or
  mutation instead of blindly creating another copy.
- On 401, refresh through Accounts once and retry with the new access token;
  if that fails, require sign-in. On 403 refresh capabilities and explain the
  denied action. A 404 may mean either missing data or data the account cannot
  see. Do not expose previously cached private content after access ends.
- Default drive quotas are 100 GiB of uploads per UTC day and 1 PiB stored.
  `GET /usage` reports actual limits and remaining capacity. Files have a 5 TiB
  protocol ceiling; available capacity, staging space and deadlines can be
  lower. 413 means too large, 507 means storage is exhausted. There is no
  published fixed request-per-second allowance; 429 can reflect account lookup
  budgets or upstream limits.
- Briefcase offers no general public SSE/WebSocket or outgoing webhook
  subscription API. Refresh affected queries after mutations, on focus and
  with conservative polling as needed. The Accounts-to-Briefcase `/webhook/`
  endpoint is signed internal delivery, not an interface subscription endpoint.

Errors have `{"error":{"code":"...","message":"...","request_id":"..."}}`.
Handle status and stable codes; show the supplied explanation without exposing
credentials. For integration problems submit `/reports` or use the
[repository issue tracker](https://github.com/teamofsilicons/silicon-briefcase/issues)
with the request ID, operation, contract version, timestamp and a redacted
reproduction. Never include tokens, private file contents or app secrets.
