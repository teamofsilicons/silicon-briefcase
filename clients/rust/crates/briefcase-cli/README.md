# briefcase

The command-line client for [Silicon Briefcase][service]: browse, upload,
download, and share organization files.

```bash
cargo install briefcase-cli
# Ask IAM for an unscoped SLT; choose the organization grants in IAM:
iam login --app-id 'tos>briefcase'
briefcase login <slt>

briefcase ls private/cos:tos/notes --long
briefcase put ./report.pdf private/cos:tos/notes
briefcase share private/cos:tos/notes/report.pdf carbon:cos:tos --access read,update
briefcase find "is:md location:'public'"
briefcase usage
```

Every command is a call on the [`briefcase-client`][package] package — the CLI
adds the parts a person needs and a program does not: a saved profile, a
rotating session and test root keys stored with owner-only permissions under
`{home_dir}/.briefcase/` (default `$HOME`), readable tables,
`--json` for scripts, and exit codes that separate "not found, or not yours to
see" (`3`) from "signed in, but not allowed" (`4`).

The hosted Briefcase URL is automatic, including on first login. URL selection
uses an explicit `--url`, then `BRIEFCASE_URL`, then the saved profile URL,
then `https://backend.briefcase.teamofsilicons.com/api/v1/`. Use the optional
override only for local development or another deployment.

For a normal production login the organization is optional: IAM returns the
member's available organizations. Use `briefcase login <slt>` for a direct
exchange, or omit the token to use the hidden prompt. Configure the state
parent with `briefcase config home /path/to/existing-directory`; the path must
already be a directory.

Paginated `ls`, `find`, and `bin list` commands return `items` plus the opaque
`next_cursor` in JSON. Continue with `--cursor`, or use `--all` to follow every
page without a silent entry cap.

`briefcase request <hidden-path>` uses the path-addressed access-request route
directly, so requesting access never depends on already being able to resolve
the entry.

State replacement is atomic and cross-process locked. Stored credentials are
bound to their canonical deployment origin. Login is unscoped; `--org` selects
each workspace operation without granting access. Legacy scoped credentials and
test roots retain their original tenant boundary. One-time login,
refresh, testing-environment mutations, folder creation, upload, move, and
version restore persist their idempotency identity before the request and
retain it after an uncertain result, so rerunning the exact command recovers
the original answer instead of applying it twice.

`briefcase app request <operation> --body request.json --describe` prepares an
exact-body IAM binding entirely locally. Send that same body with a fresh proof
to create folders, list/read/trash entries, or reserve/commit/check/cancel staged
uploads, without loading a member session or automatically retrying. File reads
require `--output` and protect existing files unless `--force` is explicit.

For large delegated uploads, `app prepare-upload` hashes a file locally,
`app request upload-reserve` saves its narrow capability to a new owner-only
`--capability-file`, and `app transfer` streams private bytes using that
capability. A separate fresh-proof commit publishes them. Capabilities never
appear in normal output; retain the logical operation UUID for status recovery.

`briefcase --help` lists every command. Full guide: [docs/cli.md][guide].

Every ordinary command also works in an isolated plane as
`briefcase --test <environment-uuid> <command>`. `briefcase env` manages those
planes and remembers UUID-to-key mappings without putting root keys on the
command line. The CLI checks crates.io at most hourly by default; use
`briefcase config set auto-update off` to opt out. Login and application
commands defer that check to the next ordinary invocation so a short-lived,
single-use IAM credential is sent immediately.

[service]: https://briefcase.teamofsilicons.com
[package]: https://crates.io/crates/briefcase-client
[guide]: https://github.com/teamofsilicons/silicon-briefcase/blob/main/docs/cli/README.md
