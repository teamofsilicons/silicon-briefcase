# briefcase

The command-line client for [Silicon Briefcase][service]: browse, upload,
download, and share organization files.

```bash
cargo install briefcase-cli
# First ask IAM for an organization-bound SLT for the canonical app:
iam --org tos login --app-id 'tos>silicon-briefcase'
briefcase login --url https://backend.briefcase.teamofsilicons.com/api/v1/ --org tos

briefcase ls private/cos:tos/notes --long
briefcase put ./report.pdf private/cos:tos/notes
briefcase share private/cos:tos/notes/report.pdf carbon:cos:tos --access read,update
briefcase find "is:md location:'public'"
briefcase usage
```

Every command is a call on the [`briefcase-client`][package] package — the CLI
adds the parts a person needs and a program does not: a saved profile, a
rotating session and test root keys stored with owner-only permissions under
`~/.briefcase/`, readable tables,
`--json` for scripts, and exit codes that separate "not found, or not yours to
see" (`3`) from "signed in, but not allowed" (`4`).

Paginated `ls`, `find`, and `bin list` commands return `items` plus the opaque
`next_cursor` in JSON. Continue with `--cursor`, or use `--all` to follow every
page without a silent entry cap.

`briefcase request <hidden-path>` uses the path-addressed access-request route
directly, so requesting access never depends on already being able to resolve
the entry.

State replacement is atomic and cross-process locked. Stored credentials are
bound to their canonical deployment origin and organization. One-time login,
refresh, testing-environment mutations, folder creation, upload, move, and
version restore persist their idempotency identity before the request and
retain it after an uncertain result, so rerunning the exact command recovers
the original answer instead of applying it twice.

`briefcase --help` lists every command. Full guide: [docs/cli.md][guide].

Every ordinary command also works in an isolated plane as
`briefcase --test <environment-uuid> <command>`. `briefcase env` manages those
planes and remembers UUID-to-key mappings without putting root keys on the
command line. The CLI checks crates.io at most daily by default; use
`briefcase config set auto-update off` to opt out. Login and application
commands defer that check to the next ordinary invocation so a short-lived,
single-use IAM credential is sent immediately.

[service]: https://briefcase.teamofsilicons.com
[package]: https://crates.io/crates/briefcase-client
[guide]: https://github.com/teamofsilicons/silicon-briefcase/blob/main/docs/cli/README.md
