# Personal accounts, organizations and teams

Briefcase uses Silicon Accounts for personal and work identities. The account
switcher opens Accounts’ account chooser. Personal and work accounts have
separate UUIDs and drives. Linking accounts makes switching easier; it does not
give work admins access to linked personal files.

Open **Organizations** in Briefcase to manage:

- People, member invitations, direct admin appointments and Manage admins authority.
- Teams, team membership and team administrators.
- Admin teams, whose current members also have organization admin authority.
- Organization name, admin team-management permissions and app policies.
- Team app allowances and exclusions, with scoped controls for team admins.

Controls follow the current role. Owners govern Manage admins and the team
permission switches. Admins without Manage admins cannot appoint other admins.
Team admins only manage their own teams, and can only add app exclusions.
The Organizations service checks every mutation again, records its audit event,
and retains a stable idempotency receipt for uncertain requests.

Organization creation/joining, account credentials, and ownership transfer or
removal governance remain on the Organizations site. The integration does not
mint organization login tokens or issue Silicon credentials.

## Sharing

In a file or folder’s **Sharing** tab, choose **People**, **Team**, or
**Organization**. People accepts personal and work IDs (`c:ada`,
`c:ada.example`, `si:scout.example`). A team recipient is its immutable team UUID
within the organization, not its display name. Organization admins can select
any team; ordinary members can select teams they belong to.

Grant view/download, edit, or, for folders, adding files. Folder grants can
inherit to children. Delete and sharing administration cannot be delegated by
a group grant. Expiring shares are read-only, for 1–43,200 minutes.

Current and future members receive group access. Leaving or being removed from
the team, deletion of the team, removal from the organization, and current app
exclusions remove that group-derived access on subsequent requests. Separate
account or other group grants still apply. Already delivered file bytes cannot
be withdrawn. Shared with me, search, path resolution, previews and downloads
use the same current group authority. Group grants are logged; group-wide email
fanout is not sent.

## API and client

All routes use the signed-in Briefcase Accounts bearer session:

- `GET /api/v1/organizations`: visible organizations, verified groups, selectable recipients.
- `GET /api/v1/organizations/{org}`: permitted people, teams, invitations and capabilities.
- `POST /api/v1/organizations/{org}/actions`: domain action with `Idempotency-Key`.
- Existing invitation routes accept `principal.type` of `organization` or `team`.
  Use IDs returned by the organizations endpoint.

```sh
briefcase organizations list --json
briefcase organizations show example --json
briefcase share plans organization:example --inherit
briefcase share plans team:example/TEAM_UUID --inherit --access read,update
briefcase organizations action example --file action.json --idempotency-key UNIQUE_KEY
```

An action file may contain `{"action":"team_create","name":"Design"}`.
Reuse its key and body when retrying an uncertain outcome. The Rust client has
`organizations`, `organization`, and `organization_action` methods and
`Recipient::Team` / `Recipient::Organization` variants. `listInvitations` now
uses operation revision 2.0.0 and `createInvitation` 2.1.0: release the matching
client with the API so old exhaustive recipient parsers refuse explicitly.

Delegated app proofs retain owner/custodian and direct account grants; they do
not carry organization membership. Use a Briefcase Accounts session for group
access and organization controls. Public links remain independent read-only
capabilities and do not confer organization membership.

## Authority and deployment

Briefcase obtains a fresh snapshot from Organizations for each bearer request.
The integration verifies the token with live Accounts `/v1/userinfo`, requires
`membership_id = briefcase:<sub>`, and binds the result to the exact UUID that
Briefcase authenticated. It never uses memberships of linked accounts. Requests
fail closed with 503 when organization authority cannot be fetched; cached
membership never grants access during an outage.

The snapshot becomes transaction-local database context. Migration 0053 adds
group grants and discovery functions; current memberships are resolved through
the permission view. Database row security still confines ordinary queries to
one drive. Group IDs are never treated as Accounts UUIDs during UUID backfill.

Set `BRIEFCASE_ORGANIZATIONS_URL` to the Organizations origin (HTTPS, or loopback
HTTP for development). Deploy the Organizations companion first. See the
[release procedure](../deploy/RELEASE-organizations.md).

IAM login, workspace and tag authority are retired. Historical migrations and
data-conversion tools remain to preserve existing files and immutable migration
checksums. No historical database, credentials, or object storage is deleted by
this change.
