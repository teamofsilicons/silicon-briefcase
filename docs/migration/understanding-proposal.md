# Proposed changes to Briefcase's UNDERSTANDING.md

UNDERSTANDING.md is changed only by Carbons. These are proposals, in the contract's own plain style, for the
Carbon to paste or rewrite. Each stage adds to this file.

## From service part 1 (identity), 2026-10-10

### Replace "How Does login/signup work"

> Signing in and signing up are handled entirely by Silicon Accounts (accounts.teamofsilicons.com). Every
> Carbon and Silicon has one personal account: a permanent uuid (what Briefcase keys everything on) and a public
> id (`c:handle` or `si:handle`) that can change and is only shown. Briefcase has an app id (`briefcase`) and an
> app secret from Silicon Apps, kept on the server only.
>
> Carbons sign in on the website through the Silicon Accounts sign-in pages, and in the CLI with the device flow
> (the CLI shows a code and a link). Silicons never see a page: they get a short-lived token with
> `silicon-accounts login --app briefcase -q` and give it to Briefcase (`briefcase login <slt>`). Either way
> Briefcase receives a Silicon Accounts access token issued to Briefcase and verifies it on every request.
> Use the official Silicon Accounts client everywhere (https://crates.io/crates/silicon-accounts-client).
>
> Silicon Accounts calls the webhook endpoint (`backend.briefcase.teamofsilicons.com/webhook/`) whenever an
> account changes its id, its name or photo, signs out, removes Briefcase's access, is deleted, or (for a
> Silicon) gets a new custodian. When an account signs out or removes Briefcase's access, its earlier tokens
> stop working at once.

### Replace "How organisations are defined"

> There are no organisations. Every account is personal. A Silicon always has exactly one custodian (a Carbon);
> the custodian can see and manage that Silicon's files, but never acts as the Silicon. (The file-system
> sections are proposed with the drive model in a later stage.)

### Replace "How other apps would use Briefcase"

> Other apps act for an account at Briefcase with a Silicon Accounts User verification proof, sent as
> `Authorization: Proof sap_…`. The proof names the account and the scopes the app may use; Briefcase checks it
> with Silicon Accounts on every call and only lets each app use the scopes its operator allowed for that app.
> The scopes are: create a folder (`briefcase.folders.create`), list entries (`briefcase.entries.list`), read a
> file (`briefcase.files.read`), move an app-created entry to the bin (`briefcase.entries.trash`), reserve,
> commit, check or cancel an upload (`briefcase.uploads.*`), share an entry (`briefcase.invitations.create`)
> and turn "anyone with the link can view" on or off (`briefcase.link_access.update`). Everything an app
> creates records which app created it.

### Remove "Testing Environment" (all subsections)

Honeycomb testing worlds are retired with Honeycomb; Briefcase is tested against a local Silicon Accounts stack
instead. Suggested one-line replacement: "Development and testing use a local Silicon Accounts stack; there is
no separate test environment inside Briefcase."

### CLI section

Replace `iam --json` with `accounts --json` (the CLI stage keeps a hidden `iam --json` alias for one release),
and "short lived tokens that the user can generate from the official iam cli" with "from `silicon-accounts
login --app briefcase -q`".
