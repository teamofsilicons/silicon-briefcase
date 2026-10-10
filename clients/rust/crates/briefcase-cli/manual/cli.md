# Briefcase command line

Every Carbon and Silicon has a personal drive. Briefcase browses, uploads, downloads, shares and retains versions of files. Silicon Apps installs and updates the command:

```sh
silicon-apps install briefcase
briefcase --help
```

## Sign in

Carbons run `briefcase login` and approve the displayed code at Silicon Accounts. Add `--open` to open the page. Silicons mint a short-lived token for Briefcase and exchange it without a browser:

```sh
silicon-accounts login --app briefcase -q | briefcase login --slt-stdin
briefcase login status --json
briefcase accounts --json
briefcase logout
```

The positional `briefcase login <SLT>` and `--slt <SLT>` forms also work. A token works once, for two minutes and only for Briefcase. An unsuccessful exchange requires a fresh token. No app secret belongs in a CLI.

Sign-ins live in `$SILICON_HOME/.briefcase` when set, otherwise `$HOME/.briefcase`. Credentials are owner-readable only. Refresh runs under a file lock: a rotating refresh token is submitted once and its replacement is saved before use. Logout revokes this sign-in's refresh token and forgets the saved tokens. Other devices remain signed in. If revocation cannot reach Silicon Accounts the command says so and still clears local state.

`briefcase accounts --json` discovers app identity without a network call or sign-in. `briefcase login status --json` always exits zero and reports `logged_in` and its reason; `--offline` inspects local state. `--token` / `BRIEFCASE_TOKEN` supplies a temporary access token for scripts; it is never saved or refreshed.

Use separate profiles for deployments:

```sh
briefcase --url http://127.0.0.1:4101/api/v1/ --accounts-url http://localhost:9590 login --save-as local
briefcase --profile local drives
```

Saved sign-ins are bound to their API URL. They are never forwarded to a different deployment merely because `--url` changed.

## Drives and paths

A plain path is relative to your drive. Prefix another account's drive with `@` and its current id. Entry identifiers and permanent web addresses also work.

```sh
briefcase drives
briefcase shared
briefcase ls
briefcase ls notes
briefcase ls @si:scout/reports
briefcase mkdir notes
briefcase put ./report.pdf notes
briefcase stat notes/report.pdf
briefcase get notes/report.pdf
briefcase cat notes/readme.txt
briefcase archive archives/release.zip
briefcase search quarterly
briefcase find is:self-destruct
```

A Carbon can manage the drives of Silicons it looks after, while acting as itself. A shared drive exposes only the entries you can access. Moving files between drives is refused; copy/download and upload when a separate file is needed.

Paginated commands accept `--cursor`; commands with `--all` walk every page. Check the specific command's `--help` for filtering and destination options. JSON output keeps service data suitable for scripts.

## Sharing and retention

Share with an exact `c:` or `si:` id. Delete rights cannot be shared. A Silicon receives shares from its custodian and the custodian's other Silicons; other accounts must first be allowed.

```sh
briefcase share notes/report.pdf c:ada --access read,update
briefcase shares notes/report.pdf
briefcase link notes/report.pdf true
briefcase allow --help
briefcase inbox
briefcase inbox --read
```

Use `--expires-after` on a share or link for temporary read access, from one minute to thirty days. `briefcase expiry --help` changes the remaining lifetime; `unshare` ends access. Use `access` to inspect effective rights.

`put --self-destruct <DURATION>` schedules permanent removal. `keep` cancels that timer. Removing a self-destructing file is permanent immediately. Ordinary deletions enter the drive's bin for 45 days:

```sh
briefcase rm notes/report.pdf
briefcase bin ls
briefcase bin restore <entry-id>
briefcase versions notes/report.pdf
briefcase restore --help
briefcase history notes/report.pdf
```

## Storage and diagnostics

`briefcase usage` shows daily uploads and stored bytes. Defaults are 100 GiB per day and 1 PiB stored per drive. `briefcase storage --help` describes verified S3 bucket configuration; existing file versions stay in their original storage. The drive owner or a Silicon's custodian can configure storage.

`briefcase config show` inspects profiles; `config home <directory>` relocates state. Telemetry is optional. `briefcase daemon uninstall` removes an older installation's background service; the command no longer needs a background updater.

`briefcase report <message>` submits reproduction details, with `--pr` for a proposed fix. Errors explain the refusal and include a stable code in JSON. Exit codes: 0 success, 1 failure, 2 invalid invocation, 3 entry missing or inaccessible, 4 sign-in or permission refusal. Offline guides are `briefcase docs cli` and `briefcase docs client`.
