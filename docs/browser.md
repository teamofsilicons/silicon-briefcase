# Browser file manager

Briefcase's browser app provides an organisation-scoped file manager. IAM owns
authentication; the Briefcase API checks access for each operation. Browser
controls reflect the rights returned by the API, rather than defining those rights.

## Sign in and open a file

Choose **Continue with IAM**. No organisation ID is required. Authenticate on
`auth.iam.teamofsilicons.com`; IAM returns you to Briefcase automatically.
The production callback is `https://briefcase.teamofsilicons.com/auth/callback`.
Briefcase adds a per-attempt browser-bound state value to that callback and
starts a normal, organisation-free IAM login. Opening an existing organisation
file link supplies its workspace context automatically.
You do not paste tokens, verification codes, or application secrets into Briefcase.

IAM asks you which organisations Briefcase may access. Login is always
unscoped, but only those explicitly granted, active organisations are returned.
New memberships are not automatically included. If there is
only one, Briefcase opens it directly; otherwise choose a workspace. Use
**Workspaces & access** in the sidebar to choose another without signing in
again. The login remains unscoped when its tokens refresh. Organisation choices
are updated at login and token refresh; IAM checks live membership on every
file operation. Workspace selection does not change or enlarge IAM consent.

Choose **Create or manage organisations in IAM** from the organisation picker
or **Organisation settings** to open IAM in a new tab. Organisation creation,
membership, and invitations stay in IAM. The link is also available when your
session has no active grants. Choose **Review organisation access in IAM** to
authorise organisations, including any you have newly created or joined.
An empty grant list requires reauthorisation, not cached workspace access.

A file link has the form `/org/{org_id}/{path}` on the website's origin. Opening
one retains the target through the IAM callback without supplying an organisation
to IAM. After authentication, Briefcase selects that workspace only if IAM
included it in your grants. Otherwise review access in IAM or open a granted
workspace; a file link cannot grant access.

The address bar follows folder navigation and opened files. **Copy link** copies
the clean file URL without a token. Browser Back and Forward resolve their target
again through Briefcase. Renamed or moved paths may no longer resolve; copy the
entry's current link after changing its location.

## Browse and manage files

- **All files** shows the organisation's top-level containers.
- **Public**, **My files**, and team spaces open their respective folders.
- **Recent** shows the 20 newest matching entries.
- **Search** searches names and indexed contents. The **Filter** field accepts
  the expressions documented in the [API guide](api/README.md).
- Folder listings load 100 entries at a time; **Load more** retrieves the next page.
- **Upload file** and **New folder** are available inside writable folders.
  From **All files**, **New folder** creates a top-level folder and asks for its
  Public, Private, or tag access boundary. The new folder stays alongside the
  reserved containers; its type does not move it inside one. To create inside
  Public or your Private folder, open that destination first. Nested folders
  inherit their parent's type.
- Entry actions include download, rename, move, share, and move to bin, according
  to the rights returned for that entry.
- **Bin** supports restoring deleted entries during their retention period.

If a Bin restore response is lost, retry **Restore** on the same entry while the
page remains open. The browser keeps the operation identity for that deletion,
so recovery confirms the original restore instead of repeating it. Deleting
the entry again starts a new restore operation.

The details panel provides a preview, access information, version history, and
activity history. Markdown previews format headings, lists, tables, and fenced
code; embedded HTML appears as source, and links and external images remain
inactive. Code and data formats such as JSON, XML, HTML, CSS, JavaScript,
TypeScript, Python, Java, SQL, and YAML use syntax highlighting. Previewing code
does not execute it.

CSV and TSV previews show a grid with spreadsheet column labels and row numbers,
up to 200 rows and 50 columns. Values remain text, including formulas, leading
zeroes, and large integers. Malformed or incomplete quoting is identified.

Text-based previews read at most 1 MiB and clearly identify truncated content;
downloading retrieves the complete file. UTF-16 byte-order marks and supported
declared character encodings are recognised. Parsing runs in a cancellable
worker with a time limit, and the result appears in a script-disabled sandbox
with no external-resource access. If formatting cannot finish, the preview shows
escaped source instead. Preview requests and workers are cancelled when the
selection changes. File content is never inserted as executable page markup.

If an upload response is lost, choose **Retry same upload**. Briefcase retains the
file, destination, and operation identity for that retry while the page remains
open. Do not replace the pending file if you intend to recover the same operation.

The gateway checks the filename, media type, destination, and current write
permission before staging bytes. It checks the session and destination again
before forwarding the file through the official client. Temporary-storage
capacity is separate from your organisation's quota: an upload may need to wait
if the browser gateway cannot reserve enough disk space. File size remains
limited to 5 TiB, subject to your organisation's configured limits.

## Sharing and access requests

Choose **Share** on an entry you can manage. Identify an existing organisation
member as `carbon:member-id` or `silicon:member-id` and select their permissions.
Read, create new content, update, and delete are distinct permissions. Folder
grants can apply to their contents. The **Access** tab lists explicit grants and
supports revocation; ownership, tags, inherited grants, and organisation roles
may also supply access.

A missing or hidden file produces a generic **File not found** state. If you were
given its link, **Request access** lets you select rights and provide an optional
message. It does not disclose the hidden entry's metadata. The backend decides
whether the request can be created.

## Notifications

The bell shows the unread count. Open it to see your latest 20 notifications,
including grants, revocations, requests, and decisions. **Mark all as read** marks
the entire inbox read, not just those 20 displayed items. The inbox refreshes when
opened, when the window regains focus, or when you select **Refresh**.

Owners and authorised administrators can review incoming requests, select the
permissions to grant, or deny a request. Decisions are validated by the backend;
an already-decided request cannot be settled a second time. Opening a notification
resolves its stable entry identifier, so access is checked again and stale paths
are not trusted.

## Organisation settings

Open **Organisation settings** from the sidebar. **Usage** reports stored bytes,
the daily upload allowance, remaining capacity, and the next reset time. Storage
includes retained versions and items in the bin.

The **Storage configuration** tab is where an organisation owner or authorised
administrator can configure an organisation-owned S3 bucket. Enter its bucket
name, region, AWS account ID, role ARN, prefix, and encryption mode. SSE-KMS also
requires the KMS key ARN in that account and region. Do not enter AWS access keys
or Silicon IAM application secrets.

**Validate and configure bucket** runs the backend's temporary create/read/update/
delete probe. The form reports **S3 bucket configured** only after a successful
response. Failed validation leaves the previous storage selected. Existing file
versions keep their recorded location; newly uploaded versions use the activated
configuration.

If the outcome is uncertain, the form retains the exact request and locks its
fields. **Retry same configuration** recovers that operation's result. A completed
failed probe unlocks the form; **Run a new validation** starts a new operation
after the external bucket or role has been fixed. Keep the page open while
recovering an uncertain request. The operation ID is displayed for reference.

## Test environments

Open **Test environments** from the sidebar while signed in to your production
organisation. This panel manages isolated environments; it does not switch the
file manager's identity or move production files into a test environment.

You can create an environment paired with an IAM test environment, edit its
name and description, and list active or retired environments. Creation requires
the paired IAM environment ID and root key, and that environment's Briefcase
application ID and secret. Enter only the selected test environment's credentials.
The API checks the caller's management authority for every operation.

Root keys are masked when revealed or returned after creation, restoration, or
rotation. **Show** and **Copy** are explicit actions. Closing the key dialog
clears that displayed key; store a needed key in your secret manager.

- **Rotate root key** invalidates the previous Briefcase root key.
- **Clean environment** erases its test data and queues stored-object cleanup.
  It cannot be undone.
- **Retire environment** disables it, with a two-day recovery window.
- **Restore environment** recovers it within that window and returns its new key.
- **Replace IAM pairing** submits the selected pairing to the backend, which
  enforces the cross-IAM migration guard.

Clean, retire, rotation, and re-pairing require typing the selected environment's
name. If a response is uncertain, keep the dialog open and retry the same
operation: its target, inputs, and operation ID remain fixed. Use the
[CLI or client](testing-environments.md) with the paired test identity for file
operations inside an environment.

## Sessions and local operation

Session tokens stay in the Rust browser gateway. The browser stores only an
HttpOnly session cookie. Sessions expire after at most eight hours; gateway
restarts require signing in again. HTTPS uses Secure, host-only cookies.

See the [browser development guide](../web/README.md) for build commands, local
preview, origin configuration, and same-origin gateway deployment.

Browsers that support imperative WebMCP can expose
`read_visible_briefcase_files`: a read-only snapshot of up to 100 entries already
shown in the current listing. It neither fetches file contents nor changes files,
permissions, or navigation. User-supplied names and paths remain untrusted content.
