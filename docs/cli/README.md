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

See the [documentation index](../README.md) and [testing guide](../testing-environments.md). Example paths containing `cos:tos` or `cos:test` are placeholders for actual IAM public member IDs; use `ls` to discover your roots.

Set uppercase shell variables such as `VERSION_ID`, `ENTRY_ID`, `GRANT_ID`,
and `REQUEST_ID` from the corresponding listing/creation response before using
those examples. Lifecycle, delete, grant and restore examples change state;
run them individually and use a disposable sandbox for experimentation.

## Signing in

First use Silicon IAM to sign in and request a short-lived token for the
canonical Briefcase Application ID. Briefcase never asks for your IAM
password, verification code, or Application secret, and it never redirects a
terminal login:

```bash
iam --org tos login --email you@example.com --app-id 'tos>briefcase'
briefcase login --org tos
# Paste only the IAM short-lived token at the hidden prompt.
```

The CLI connects to the hosted Briefcase service automatically. You do not
need to find or enter a backend URL for normal use.

For automation, pipe that one-use SLT instead of putting it in argv:

```bash
printf '%s\n' "$BRIEFCASE_SLT" | briefcase login --org tos --slt-stdin
```

The deployment and organization are saved in `~/.briefcase/config.json`.
Briefcase exchanges the SLT for an access/refresh pair and stores the rotating
session in `~/.briefcase/credentials.json` with owner-only permissions. It
rejects an exchange response whose session is unscoped or bound to an
organization other than `--org`, before either token can be stored or used. It
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

Stored production sessions and every test root/session are bound to the
canonical deployment origin and organization where they were acquired.
Changing `--url` or `--org` therefore fails before a request rather than
forwarding a stored credential. Older state is migrated only against that
profile's existing saved destination. An explicit `--token` authorizes its own
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
shape for CI. `BRIEFCASE_HOME` moves the state directory itself.

```bash
briefcase status     # profile, deployment, token, and contract agreement
briefcase logout     # forget the token, keep the profile
```

`--token`/`BRIEFCASE_TOKEN` remains an explicit ephemeral override for CI and
does not replace the stored rotating session. `logout` forgets only the
production or `--test` session currently selected.

## Disposable testing environments

A Briefcase test plane must be paired with an IAM test plane; Briefcase test
data can never call production IAM. Bootstrap in this order:

1. From production IAM, run `iam env create` and securely keep its public UUID
   and 32-character root key.
2. Enter that IAM plane with `iam --test <iam-uuid> ...`, create/sign in a test
   Carbon or Silicon, then import the production Briefcase Application with
   `iam --test <iam-uuid> -o json app import 'tos>briefcase'`. Import
   preserves the canonical `org>handle` ID but returns a fresh test-only
   `ask_...` Application secret.
3. Sign in to production Briefcase, then create its paired plane. The two IAM
   secrets are hidden prompts by default:

```bash
briefcase env create checkout-e2e \
  --description 'ephemeral integration run' \
  --iam-environment-id "$IAM_TEST_ID" \
  --iam-app-id 'tos>briefcase'
# prompts: IAM environment root key; IAM Application secret
```

IAM 1.2 online authorization snapshots now bootstrap the caller's projection on first use. Do not change profile metadata to force a webhook. Obtain an organization-bound SLT inside the paired IAM test plane, then:

```bash
briefcase --test "$BRIEFCASE_TEST_ID" login --org tos
briefcase --test "$BRIEFCASE_TEST_ID" ls
```

Paste only the test SLT at the hidden prompt. A successful `ls` verifies the test bearer and projection; it does not by itself prove webhook delivery.

`BRIEFCASE_IAM_ENVIRONMENT_KEY` and `BRIEFCASE_IAM_APP_SECRET` are available
for a secret-injected CI environment. Supplying those values as flags also
works but can expose them through the process list. The create response never
echoes either IAM secret; it returns a distinct Briefcase root key. The CLI
stores that key under the public Briefcase UUID in the owner-only credentials
file.

Every production command then works unchanged in the isolated plane:

```bash
briefcase --test "$BRIEFCASE_TEST_ID" login --org tos
briefcase --test "$BRIEFCASE_TEST_ID" mkdir reports --type private
briefcase --test "$BRIEFCASE_TEST_ID" put report.pdf private/cos:test/reports
briefcase --test "$BRIEFCASE_TEST_ID" ls --all
```

`--test` accepts a hyphenated public UUID only, never a root key. It also reads
`BRIEFCASE_TEST`. Production and every test environment have independent
stored sessions. The CLI looks up the secret key locally and sends it as
`X-Testing-Environment-Key` alongside, not instead of, the test IAM bearer.
An unknown UUID fails before a network request and tells you to retrieve the
key.

Environment management mirrors IAM's command grammar:

```bash
briefcase env list --status all
briefcase env show "$BRIEFCASE_TEST_ID"
briefcase env update "$BRIEFCASE_TEST_ID" --name renamed
briefcase env key "$BRIEFCASE_TEST_ID"          # retrieves and stores it
briefcase env rotate-key "$BRIEFCASE_TEST_ID"   # old key stops immediately
briefcase env pair-iam "$BRIEFCASE_TEST_ID" \
  --iam-environment-id "$REPLACEMENT_IAM_TEST_ID" \
  --iam-app-id 'tos>briefcase'
briefcase env clean "$BRIEFCASE_TEST_ID"        # production management plane
briefcase --test "$BRIEFCASE_TEST_ID" env current
briefcase --test "$BRIEFCASE_TEST_ID" env clean # root-key-only self service
briefcase env delete "$BRIEFCASE_TEST_ID"
briefcase env restore "$BRIEFCASE_TEST_ID"
```

`env current` and UUID-less `env clean` fail locally with `this action is only
possible for a test environment` without `--test`. Retirement is recoverable
until the service's purge deadline; cleaning keeps the environment/key but
erases its disposable contents.

All UUID-addressed management commands run only from the production plane and
fail locally when combined with `--test`. `pair-iam` atomically replaces the
IAM environment UUID/root key and imported Application ID/secret while keeping
the Briefcase UUID, root key, and data. After pairing a new IAM plane, obtain a new test SLT and verify a test bearer request; complete online snapshots populate current authority. The CLI removes the obsolete saved session from the old IAM
plane when re-pairing succeeds.

The CLI persists an idempotency key plus a SHA-256 fingerprint before every
environment mutation. Environment updates also persist the original optimistic
version. A retry of the same command therefore uses the same request, key, and
version; create/restore/rotation responses can recover their generated root
key. The pending record is removed only after the successful result and any
new root key have been atomically saved. Do not manually remove pending records
from `credentials.json` after an uncertain outcome—rerun the exact command.
Root-key retrieval and rotation share that same lock through fetch and save,
so a delayed key read cannot replace a concurrently rotated key.

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
briefcase mkdir notes --type private            # in your own Private folder
briefcase mkdir handbook --type public          # in the Public container
briefcase mkdir specs --type tag --tag engineering
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
version. The previous fifty are kept:

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

## Sharing

```bash
briefcase share private/cos:tos/notes carbon:cos:tos --access read,write --inherit
briefcase shares private/cos:tos/notes
briefcase unshare private/cos:tos/notes "$GRANT_ID"

briefcase access private/cos:tos/notes public/handbook   # what may I do here?
```

Members are written `kind:id` — `carbon:cos:tos`, `silicon:atlas` — and rights
are a comma-separated set of `read`, `write`, `update`, `delete`. They are
independent: `write` adds files to a folder, `update` changes a file that is
already there, and neither conveys `delete`.

When you cannot read something and want to:

```bash
briefcase request public/handbook/private-draft.md --access read --reason "reviewing"
briefcase inbox                        # requests waiting on you, decisions on yours
briefcase decide "$REQUEST_ID" approve --access read
briefcase decide "$REQUEST_ID" deny
briefcase inbox --read                 # clear the badge
```

A path target is sent directly to the privacy-preserving access-request route;
the CLI does not try to inspect or resolve the hidden entry first. A missing
path and one outside the organization remain indistinguishable. A UUID target
continues to use the UUID-addressed route. Path requests persist their
idempotency key before the call, so rerunning the exact path, rights, and reason
after an uncertain outcome recovers the same request instead of duplicating it.

## Everything else

```bash
briefcase history private/cos:tos/notes/report.pdf   # who did what, when
briefcase usage                                      # storage and today's uploads
briefcase version                                    # client and server contracts
briefcase storage configure --bucket … --region … --role-arn … --account …
briefcase app upload --app-id 'tos>app-notes' ./generated.md # hidden proof prompt
briefcase app upload --app-id 'tos>app-notes' --proof-stdin ./generated.md < proof.txt
```

Application IDs on client-facing operations are always canonical
`{org_id}>{handle}`. A local handle such as `app-notes` is rejected before a
request so it cannot silently target the wrong organization. Explicit
`--proof "$PROOF"` remains available, but can expose the proof through the
process list; prefer the hidden prompt or `--proof-stdin`.

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
listEntries is 1.1.0 here and 1.0.0 there
briefcase: upgrade the CLI, or pass --no-verify to call it anyway at your own risk
```

Upgrading the CLI is the answer. `--no-verify` exists for a deliberate rollout
where the mismatch is known and accepted.

## Automatic updates

Before an ordinary command, the installed CLI performs a best-effort crates.io
check at most once per day. When a newer stable `briefcase-cli` exists it runs
`cargo install briefcase-cli --bin briefcase --version =<version> --locked
--force`; the current command finishes on the old process and the next
invocation uses the new binary. Update failures are warnings and never block
the command.

`login` and `app` commands deliberately skip this pre-command work. Their IAM
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

The package uses a separate first-ordinary-request maintenance hook. Neither
updater replaces code already loaded in a running process.
