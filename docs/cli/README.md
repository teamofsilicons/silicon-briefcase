# Using the `briefcase` CLI

The CLI is the `briefcase-client` package with a memory and a face. Every
command is a call on the package — the CLI has no capability the package
lacks — and what it adds is remembering which deployment you meant, printing
answers you can read, and giving a script an exit code it can branch on.

```bash
cargo install briefcase-cli      # installs the `briefcase` binary
briefcase --help                 # every command, every option
briefcase ls --help              # one command in detail
```

This guide targets CLI **1.0.1** and the first official API contract **1.0.0**.
Development 0.x releases are unsupported. For local development, install with
`cargo install --path clients/rust/crates/briefcase-cli`.

## Signing in

First use Silicon IAM to sign in and request a short-lived token for the
canonical Briefcase Application ID. Production sign-in never asks for your IAM
password, verification code, or Application secret, and it never redirects a
terminal login:

```bash
iam login --app-id 'tos>briefcase'
briefcase login <slt>
# Or run `briefcase login` and paste only the SLT at the hidden prompt.
```

The positional form exchanges the supplied SLT directly. To use the hidden
prompt instead, run `briefcase login` with no token. A normal production login
is always unscoped: IAM prompts you to select the organizations Briefcase may
access, and Briefcase keeps that one token family for the selected set.
For noninteractive IAM use, explicitly supply `--grant-org tos,my-team` or
`--all-orgs` to IAM (the latter grants only memberships shown now).
Briefcase's `--org` selects a workspace, never consent or a scoped login.
When several granted organizations are available, commands that
touch files ask you to choose with `--org`; no second IAM login is required.

The CLI connects to the hosted Briefcase service automatically. You do not
need to find or enter a backend URL for normal use.

For automation, pipe that one-use SLT instead of putting it in argv:

```bash
printf '%s\n' "$BRIEFCASE_SLT" | briefcase login --slt-stdin
```

The deployment and current workspace preference are saved in
`{home_dir}/.briefcase/config.json`.
Briefcase exchanges the SLT for an access/refresh pair and stores the rotating
session in `{home_dir}/.briefcase/credentials.json` with owner-only permissions. It
preserves unscoped sessions and the IAM-selected organization list. A workspace
preference cannot add grants. If IAM returns no active grants, revisit IAM to
select organizations and sign in again; do not reuse cached access. It
refreshes one minute before expiry and persists the new refresh token before
sending the requested command. It records a refresh idempotency key before the
network call, so an uncertain outcome reuses the exact token/key pair. A
successful refresh invalidates the previous refresh token.

For local development or a separate deployment, `--url` is an optional
override. URL selection is, in order: an explicit `--url`, `BRIEFCASE_URL`,
the saved profile URL, then `https://backend.briefcase.teamofsilicons.com/api/v1/`.
This default also applies to commands in a fresh, unconfigured profile;
organization and authentication requirements still apply. An override must
use the exact `/api/v1/` base. HTTPS is required except on `localhost` or a
loopback IP. Before presenting a single-use refresh token,
the CLI performs the anonymous contract handshake unless `--no-verify` was
explicitly selected, so an incompatible deployment cannot consume it.

State updates are atomic; credential, pending-mutation, and configuration
read/modify/write transactions share a cross-process lock. Login, refresh,
and environment mutations keep that lock through their one-time
exchange, so two CLI processes cannot consume the same rotating credential or
overwrite the newer result. Before an SLT exchange the CLI also stores a hash
of the complete login intent and its idempotency key. If the response is lost
or the process stops, rerun the exact command with the same SLT; the CLI reuses
the key and recovers the original server response. Raw SLTs and IAM secrets are
never written into the retry record.

Stored credentials remain bound to their canonical deployment origin. A scoped
session also stays bound to its organization; an unscoped production session can
select another currently reachable organization with `--org`. A test root
always retains its owning organization, even with an unscoped IAM session.
An incompatible `--url` or scoped/root `--org` override therefore fails before
forwarding a stored credential. An explicit `--token` authorizes its own
production destination override; it never authorizes forwarding a stored test
root.

Save a named hosted profile, or explicitly select a local deployment:

```bash
briefcase login --org tos --save-as work
briefcase --profile work ls
briefcase login --url http://127.0.0.1:8080/api/v1/ --org tos --save-as local
```

`--url`, `--org`, `--token`, and `--profile` also read `BRIEFCASE_URL`,
`BRIEFCASE_ORG`, `BRIEFCASE_TOKEN`, and `BRIEFCASE_PROFILE`, which is the usual
shape for CI. State defaults to `$SILICON_HOME/.briefcase` when `SILICON_HOME`
is present, otherwise `$HOME/.briefcase`. Missing state directories are created
on the first write; an empty `SILICON_HOME` or a file path is an error. `BRIEFCASE_HOME` remains
an explicit override of the **complete state directory**, with highest priority.
You can also choose a persistent parent directory:

```bash
briefcase config home /path/to/existing-directory
```

The configured location must already be a directory; otherwise the command
fails with `not a directory`. The choice is recorded in
`$SILICON_HOME/.briefcase-home` (or `$HOME/.briefcase-home` when `SILICON_HOME`
is absent), and takes precedence over that default parent. Changing
`SILICON_HOME` selects a separate configuration and credential store; existing
files are not moved automatically. `BRIEFCASE_HOME` still overrides the pointer.

```bash
briefcase status     # profile, deployment, token, and contract agreement
briefcase logout     # forget the token, keep the profile
```

`--token`/`BRIEFCASE_TOKEN` remains an explicit ephemeral override for CI and
does not replace the stored rotating session. `logout` forgets only the
production or `--test` session currently selected.

## Help, IAM discovery, and authentication status

```bash
briefcase --help
briefcase login --help
briefcase iam --json
briefcase login status --json
```

`--help` lists all top-level commands and shared options, with getting-started
examples. Use `briefcase <command> --help` for its complete syntax. Help works
without a home directory, saved profile, network connection, or login.

`iam --json` reads the selected deployment's public IAM configuration without
sending a member access token. It returns `app_id`, `test_environment_id`, and
`iam_environment_id`. Use the `app_id` when requesting the single-use SLT from
IAM. Production returns null for both environment IDs. No app secret or
app secret is printed. URL/profile overrides apply as usual.

`login status --json` checks the session with IAM and returns one JSON object:

```json
{
  "authenticated": true,
  "actor": {
    "principal_id": "01990a9d-86f1-7000-8000-000000000001",
    "type": "carbon",
    "public_id": "cos:tester"
  },
  "organizations": ["tos"],
  "expires_at": 1789000000,
  "profile": "default",
  "url": "https://backend.briefcase.teamofsilicons.com/api/v1/",
  "test_environment_id": null
}
```

The actor type is `carbon` or `silicon`. IAM supplies the public identifier
through active organization snapshots; if no organizations are granted,
`public_id` is null and the stable principal UUID still identifies the actor.
No `--org` is required, including for logins with several granted organizations.
A saved session refreshes before expiry using the normal atomic rotation flow.
An explicit `--token` is checked directly and never refreshes the saved session.
Missing, expired, or revoked member credentials report `authenticated: false`,
`actor: null`, empty organizations, and `expires_at: null`. This is a successful
status query (exit 0); scripts should inspect `authenticated`. Network failures,
IAM outages, contract mismatches, and invalid test-plane configuration remain
errors, not a false successful authentication result.

Both commands accept `--test <environment_id>` with the saved paired test key.
The existing `briefcase status` remains the profile/deployment/contract summary;
use `briefcase login status` for live authentication and identity.

## Limits and test-plane boundaries

Regular uploads use one endpoint for every supported file size. Briefcase
uses one storage request through 100 MiB and automatically switches to its
durable multipart path above that threshold; callers do not need a delegated
upload flow for ordinary member uploads.
Each paired testing environment is isolated from production IAM, limited to
2 GiB of aggregate content and at most 10 active environments per deployment;
deleted environments remain recoverable for two days. A Briefcase test bearer
and its IAM testing app secret are both required for test-plane requests.

## Testing environments

Create IAM and Briefcase together: `briefcase env create integration`.
The result is stored as a UUID-to-IAM-test-app-secret mapping. Use
`briefcase --test <environment-id> login <test-slt>` and then the same file
commands with `--test`. Alternatively, set `BRIEFCASE_APP_SECRET` or pass
`--app-secret`; the CLI resolves the environment and keeps its login separate.
The testing footer is always printed to stderr, including on errors.

Use `env list`, `env show`, `env key`, `env edit`, `env clean`, `env delete`, and
`env restore` to manage the local Briefcase plane. Rotate credentials in IAM
and run `env pair-iam` with the replacement pairing. There is no independent
Briefcase key rotation. IAM test actors retain their real role/tag permissions.
See [Testing environments](../testing-environments.md) for provisioning,
lifecycle, storage isolation, and exact HTTP/Rust equivalents.

## Addressing entries

Anywhere a command takes an entry, it takes the path its permanent URL shows —
`private/cos:tos/notes/report.pdf` — or the entry's identifier. A leading slash
is fine.

## Browsing

```bash
briefcase ls                                    # the organization base
briefcase ls private/cos:tos/notes --long       # size, owner, what you may do
briefcase ls public/handbook --all              # follow every page
briefcase ls public/handbook --cursor "$NEXT"   # resume a previous page
briefcase stat private/cos:tos/notes/report.pdf # one entry in full

briefcase find "is:md location:'public' after:01-01-2026"
briefcase find "permissions:delete" --all
briefcase find "is:md" --cursor "$NEXT"
briefcase search "quarterly revenue"
```

`find` takes the service's filter language; `search` looks inside filenames and
extracted document text and says which one matched. Without `--all`, `ls` and
`find` return one page and preserve the service's opaque `next_cursor`: human
output prints a continuation hint, while JSON contains `items` and
`next_cursor`. Pass that value back unchanged with `--cursor`. `--all` follows
pages until the service returns no next cursor; it has no hidden entry cap and
fails explicitly if a broken deployment repeats a cursor.

## Files

```bash
briefcase mkdir notes --type private            # /notes, a Private root
briefcase mkdir handbook --type public          # /handbook, a Public root
briefcase mkdir specs --type tag --tag engineering
briefcase mkdir private/cos:tos/notes            # explicitly inside your folder
briefcase mkdir public/handbook                  # explicitly inside Public
briefcase mkdir private/cos:tos/notes/quarterly # inside an existing folder
briefcase mkdir shared --type private --invite carbon:cos:tos=read,write

briefcase put report.pdf figures.csv private/cos:tos/notes/quarterly
briefcase put report.pdf public/handbook --name q3-report.pdf

briefcase get private/cos:tos/notes/quarterly/report.pdf -o ./local.pdf
briefcase cat private/cos:tos/notes/quarterly/notes.md

briefcase mv private/cos:tos/notes/a.md private/cos:tos/notes/b.md   # rename
briefcase mv private/cos:tos/notes/b.md public/handbook/b.md         # move
briefcase rm private/cos:tos/notes/quarterly/draft.md                # to the bin
```

`mkdir`, every individual file in `put`, `mv`, and version `restore` also
persist an intent fingerprint and idempotency key before sending. Upload
fingerprints include the source file's SHA-256 digest. Path-addressed rename,
move, and restore retain the resolved entry UUID; nested folder creation,
uploads, and moves also retain the resolved destination-folder UUID. Rerunning
after a lost success response therefore replays the original request without
requiring either old path to remain visible. Pending state is cleared only
after the CLI observes success.

Uploading a name that an active file already carries publishes that file's next
version. Every version is retained:

```bash
briefcase versions private/cos:tos/notes/quarterly/report.pdf
briefcase restore private/cos:tos/notes/quarterly/report.pdf "$VERSION_ID"
```

Deleting is recoverable for 45 days:

```bash
briefcase bin list
briefcase bin list --cursor "$NEXT"
briefcase bin list --all
briefcase bin restore "$ENTRY_ID"
```

`bin list` uses the same cursor, JSON page, and exhaustive `--all` behavior as
`ls` and `find`.

`bin restore` saves its operation key before sending. If the response is lost,
rerun the exact command with the same profile and environment to recover that
restore. The pending operation is cleared after success; restoring a later
deletion uses a new key.

## Sharing

```bash
briefcase share private/cos:tos/notes carbon:cos:tos --access read,write --inherit
briefcase shares private/cos:tos/notes
briefcase unshare private/cos:tos/notes "$GRANT_ID"

briefcase access private/cos:tos/notes public/handbook   # what may I do here?
```

Members are written `kind:id` — `carbon:cos:tos`, `silicon:atlas` — and rights
are a comma-separated set of `read`, `write`, `update`. Delete cannot be granted. They are
independent: `write` adds files to a folder, `update` changes a file that is
already there, and neither conveys `delete`.

Read and clear sharing notifications:

```bash
briefcase inbox
briefcase inbox --read                 # clear the unread badge
```

## Public links, audit logs, and complete versions

```bash
briefcase share private/me:tos/report.pdf email:alex@example.com --access read,update
briefcase share private/me:tos/reports tag:engineering --access read,write --inherit
briefcase link private/me:tos/reports --enabled true
briefcase link private/me:tos/reports
briefcase logs private/me:tos/reports --json
briefcase logs private/me:tos/reports --cursor '<next-cursor>' --json
briefcase versions private/me:tos/report.pdf --cursor '<next-cursor>' --json
briefcase get private/me:tos/reports --output reports.tar.zst
```

Folder downloads are streamed `.tar.zst` archives of authorized contents.
Versions are retained without a count limit; logs retain 365 days, while
`history` shows the latest 100 events. Read is always included, write adds new
children to folders, and update changes existing content. Only creators and
organization administrators can delete or manage sharing. See [Sharing](../sharing.md)
for verified-email limitations, tag membership, and inherited link settings.

## Everything else

```bash
briefcase history private/cos:tos/notes/report.pdf   # who did what, when
briefcase usage                                      # storage and today's uploads
briefcase version                                    # client and server contracts
briefcase storage configure --bucket … --region … --role-arn … --account …
briefcase app upload --app-id 'tos>app-notes' ./generated.md # hidden proof prompt
briefcase app upload --app-id 'tos>app-notes' --proof-stdin ./generated.md < proof.txt
```

`storage configure` prints its operation UUID to stderr before submitting the
configuration. After a lost response, repeat the exact same arguments with
`--operation-id <that-uuid>` to recover the result. A completed failed probe needs
a new UUID after fixing the bucket or role; reusing the old UUID returns that
failed result without running a new probe. Validation failure exits with status
1. With `--json`, stdout still contains the single validation-status object;
the operation ID and diagnostics go to stderr.

Application IDs on client-facing operations are always canonical
`{org_id}>{handle}`. A local handle such as `app-notes` is rejected before a
request so it cannot silently target the wrong organization. Explicit
`--proof "$PROOF"` remains available, but can expose the proof through the
process list; prefer the hidden prompt or `--proof-stdin`.

### Delegated application requests

Applications can create folders, list entries, read files, move entries to the
bin, and stage uploads without a saved member session. Prepare the operation's
JSON file, then describe it locally before asking IAM for a proof:

```bash
briefcase app request folder-create --body folder.json --describe
briefcase app request folder-create --body folder.json --app-id 'tos>app-notes'
briefcase app request file-read --body read.json --app-id 'tos>app-notes' --output ./download.bin
```

`--describe` only reads the bounded JSON file (at most 1 MiB), validates the
selected operation, and prints its canonical `body` string, HTTP `method`,
`path`, IAM `endpoint_id`, `body_sha256`, and empty `metadata` object. It does
not read profiles or credentials, contact a server, or check for updates.
Give IAM those exact binding values, then repeat the command with the same
JSON file and a fresh proof at the hidden prompt or through `--proof-stdin`.
Unknown JSON fields are rejected. See the [API request schemas](../obo.md#json-controls-and-recoverable-uploads)
for the body fields.

The available operations are `folder-create`, `entries-list`, `file-read`,
`entry-trash`, `upload-reserve`, `upload-commit`, `upload-status`, and
`upload-cancel`. Each request uses fresh IAM authorization; no proof is cached
or retried automatically. Keep a mutation's `operation_id` and body unchanged
when obtaining a fresh proof for a retry. A file read requires `--output`
before any proof or request is sent. Its destination appears only after the
complete response is saved; existing paths are protected unless `--force`
explicitly permits replacement. Ranges and download disposition belong in
the signed JSON body, not separate request headers.

### Staged application uploads

Prepare a reservation before minting its short-lived proof. This local command
hashes the file with bounded memory and prints a credential-free JSON manifest:

```bash
briefcase app prepare-upload ./recording.webm --operation-id "$OPERATION_ID" --parent-path public/recordings > reserve.json
briefcase app request upload-reserve --body reserve.json --describe
briefcase app request upload-reserve --body reserve.json --app-id 'tos>app-notes' --capability-file ./upload.cap
briefcase app transfer "$UPLOAD_ID" ./recording.webm --capability-file ./upload.cap
```

Choose one stable non-nil UUID as `OPERATION_ID`; `UPLOAD_ID` is the reservation
UUID in the response. Keep the source file unchanged between preparation and
transfer. An empty `--parent-path` selects the member's private application
folder. Preparation and transfer require a regular source file: use its actual
path, not a symlink, pipe or device. `prepare-upload` needs no configured
profile, credential, network, or update check.

Reservation requires a new `--capability-file` and creates it with owner-only
Unix permissions before consuming a proof. The capability is never included
in ordinary JSON or terminal output, and an existing file is never replaced.
If the request fails or the server returns no capability, that new file may remain
empty; a local write failure may instead leave incomplete content. Reconcile
with a fresh status proof before retrying, and use a new filename for a later
reservation attempt. Transfer accepts
the private file, a hidden prompt, or `--capability-stdin`. It refuses symlink
capability files, special files, and files accessible to other users. Secret
input is limited to 64 KiB. Capability-file handling
requires Unix file permissions.

Transfer only stores private bytes. Publish with a fresh proof for
`upload-commit`, using a body containing the original `operation_id` and the
returned `upload_id`. `upload-status` and `upload-cancel` take a body containing
the original `operation_id`, also with a fresh proof. After any uncertain
transfer or commit, reconcile with status before retrying; never infer
publication from a completed transfer. Retain the credential-free manifest
and status for recovery, not IAM proofs or parent tokens. Protect the capability
file while needed and remove it when the upload is finished.

## Scripting

`--json` prints the service's own shapes, so nothing has to parse a table:

```bash
briefcase ls public --json | jq -r '.items[] | select(.type == "file") | .path'
NEXT=$(briefcase ls public --json | jq -r '.next_cursor // empty')
briefcase usage --json | jq '.storage.remaining_bytes'
```

Exit codes:

| Code | Meaning |
| --- | --- |
| `0` | the command did what it said |
| `1` | something failed |
| `2` | the command as typed cannot be carried out |
| `3` | not found, or not yours to see |
| `4` | the credential was refused, or the action is not allowed |

Codes `3` and `4` are deliberately distinct, and `3` covers both "there is no
such entry" and "there is, but not for you" — the service never confirms that
a hidden entry exists.

```bash
if briefcase stat "$path" >/dev/null 2>&1; then
  echo "entry is visible"
else
  status=$?
  case "$status" in
    3) echo "not there, or not mine to see" ;;
    4) echo "signed in, but not allowed" ;;
    *) echo "something else went wrong" ;;
  esac
fi
```

## When the client and the deployment disagree

Before its first call the CLI verifies the service identity and selected API
major, then checks every operation it calls by exact ID, revision, method, and
path. Duplicate IDs are refused; a new unknown operation is additive. A
mismatch stops the command and names what moved:

```text
briefcase: briefcase serves a contract this client was not built for (serving v1);
listEntries is 1.0.0 here and 2.0.0 there
briefcase: upgrade the CLI, or pass --no-verify to call it anyway at your own risk
```

Upgrading the CLI is the answer. `--no-verify` exists for a deliberate rollout
where the mismatch is known and accepted.

## Automatic updates

After an ordinary command finishes, the installed CLI performs a best-effort
crates.io check if the last attempt was at least one hour ago. When a newer
stable `briefcase-cli` exists it runs
`cargo install briefcase-cli --bin briefcase --version =<version> --locked
--force`; the next invocation uses the new binary. The command's output and
exit status are preserved even if maintenance fails. Automatic maintenance
never waits for another CLI process's updater; failed attempts are throttled
too. The first eligible command checks when no timestamp has been saved.

`login` and `app` commands deliberately skip automatic maintenance. Their IAM
SLTs and OBO proofs are short-lived, single-use credentials, so the CLI sends
them immediately and defers any due update check to the next ordinary
invocation.

```bash
briefcase config show
briefcase config set auto-update off   # persistent opt-out
briefcase config unset auto-update     # restore default-on behavior
BRIEFCASE_AUTO_UPDATE=off briefcase ls # process-scoped opt-out
briefcase system update                # explicit check, ignoring the throttle
```

The package schedules separate, best-effort hourly maintenance in the
background after an ordinary operation completes. Download streams defer it
until EOF, failure, or abandonment. Clients targeting the same Cargo manifest
share the in-process throttle. Neither updater replaces code already loaded
in a running process.

Invitation, revocation, and link-setting commands save their exact operation identity and entry ID before sending. Repeating an interrupted command reuses that intent; it cannot silently target a new file that moved into the old path. `briefcase shares TARGET --cursor CURSOR --json` retrieves further invitation pages (100 per page).
