# Briefcase session retention release — 22 September 2026

The browser gateway and CLI session fixes are deployed and published. The API and
worker were not changed by this release.

## Released source and artifacts

- Browser source: `e33e20a1f5f9f901ff234c6ba9da817cc9a205ae`.
- Client/CLI source: `2e3a5429277b6528182f8dd8c907c77f77623984`, tag `v1.1.2`.
- Crates: `briefcase-client` and `briefcase-cli` 1.1.2. Anonymous registry downloads
  matched the published local package bytes and embedded source revision.
- [GitHub release](https://github.com/teamofsilicons/silicon-briefcase/releases/tag/v1.1.2).
  Its archive, checksum files and native CI receipt were downloaded anonymously
  and checked against the uploaded bytes and GitHub SHA256 metadata.
- Honeycomb `tos>briefcase` production 1.1.2: accepted/public release
  `d1278364-8cfd-473f-8010-0017bfcc0d16`.
- Archive SHA256: `606291f26176cd7752d61f293c0cb616a7273fde3170d8617457ed4627f7c88b`,
  21,093,275 bytes. Fresh anonymous installation selected 1.1.2; its Apple ARM64
  executable matched `aa44144ca31d2a35b65a204656dae8e9c38a52d83d0cee5710ba4e35b9a424d9`.
  Version/help checks passed without creating a service or modifying shell setup.

The release used an isolated checkout. The preexisting working-tree edits were
excluded. A small committed `honeycomb-managed` build guard preserves the updater
behavior already present in the published 1.1.0 native package; broader pending
updater removal was not included. The new native workflow and packaging support
produce six real platform binaries and check Linux against glibc 2.28.

## Production rollout and retention proof

The immutable ARM64 browser image is:

```
234951665042.dkr.ecr.us-east-1.amazonaws.com/silicon-briefcase-production@sha256:7a672f0a89cf946d122d884e1db904a27b179e1417cc47208605eca9c1ca5299
```

SSM command `74aff381-e25e-4735-806c-c1f7867c1e19` updated only
`silicon-briefcase-web` on `i-00c2b0c4968b57186`. Its effective systemd drop-in pins
that digest and mounts `/var/lib/silicon-briefcase/web-sessions` at the same path
with `BRIEFCASE_WEB_SESSION_DIRECTORY` configured. The directory is mode 0700,
owned by UID/GID 65532; all state files checked were mode 0600. API/worker container
IDs and browser environment-file bytes were unchanged. Readiness passed and the
new browser container had zero unexpected restarts.

A normal IAM sign-in created a separate verification browser family for the
existing `chef:bricks` identity. It read the organization's three existing entries.
After restarting the gateway, the same browser cookie still authenticated the
same actor and read those three entries. Logout persisted: after another restart,
the old cookie returned 401. The verification family was then revoked directly
through IAM's application-authenticated revocation route; introspection confirmed
its access token inactive. No CLI credential was copied. This revocation path was
later found to invalidate sibling access tokens for the same IAM session and app,
even while their separate refresh families remained active. The initial cleanup
therefore does not prove sibling isolation. The separately coordinated IAM repair
binds each access token to its own refresh family.

- Retention restart: `49c4c053-6fa8-4f8d-bd59-bd83c52238d7`.
- Logout restart: `d8c6d3b8-af30-4b4a-b003-f37d9ed84c4a`.
- Verification-family cleanup: `5aea147e-8366-47ae-90d8-d67989cc4fdc`.

The first upgrade from the preceding memory-only gateway requires one fresh login;
that previous process had no durable sessions to recover. Further restarts now
retain saved sessions. The live proof covered restart/logout persistence and real
reads; delayed refresh replies and storage-failure recovery are regression-tested.

## CLI early-invalidation recovery

The 1.1.2 follow-up (`2e3a5429277b6528182f8dd8c907c77f77623984`) validates saved
access through auth/status before starting a command. Inactive access, including
HTTP 401 before advertised expiry, forces the existing locked durable refresh.
This runs before uploads or multi-step writes, so those operations are not replayed.
Explicit bearer overrides and OBO flows retain their original credential behavior.
Transient failures retain the family, original dispatch time and refresh key.
Twenty-four process-level security tests and the CLI unit/daemon tests pass, with
strict Clippy; regression coverage includes both inactive status and early 401,
proving a mutation starts once with renewed access, and transient-outage retention.

The published 1.1.2 archive and all GitHub assets were anonymously downloaded and
checksum verified. A fresh anonymous Honeycomb install selected 1.1.2, matched the
native Apple ARM64 executable, and passed version/help checks. All six executable
and manifest bytes match the CI package; tar ownership/timestamp metadata differs.

Maharaj's supported Honeycomb update was dispatched, but the host stalled opening
files under Documents before the launcher switched from 1.1.1. Even its existing
`briefcase --version` process is blocked in the macOS dynamic loader's `__open`,
before application code. No fresh login or credential replacement was attempted.
Thus final installed 1.1.2 authentication/three-entry verification remains pending
local filesystem recovery. The earlier 1.1.1 retained login proof and deployed
browser restart proof remain valid; they do not prove this pending local upgrade.

The IAM family-isolation repair was verified independently in production through
two normal Commit app families: the selected family's logout revoked only its own
access while the sibling stayed active with automatic refresh disabled. The
nonsecret receipt is `commit-carbon-sibling-isolation.json`.

## Validation and recovery

[Browser image CI](https://github.com/teamofsilicons/silicon-briefcase/actions/runs/35662265771)
passed CLI/BFF tests, strict Clippy, browser asset build and exact-source ARM64
image construction. [Native release CI](https://github.com/teamofsilicons/silicon-briefcase/actions/runs/35666275478)
passed all seven jobs. The release checkout's managed CLI unit tests also passed
locally. The generic backend CI initially detected a root lockfile entry still
naming client 1.1.0; commit `8aba062` updates that test dependency to 1.1.1 without
changing backend code or runtime artifacts. That exposed seven existing imported-world
fixture failures: their mocked membership IDs still used private UUIDs after the
canonical IAM contract change. Test-only commit `4387b8d` updates those fixtures to
`test-carbon[org_id]`, preserving the decoder's strict identity checks. [General CI](https://github.com/teamofsilicons/silicon-briefcase/actions/runs/35666876918)
then passed all four jobs, including real-database backend tests. Production
backend behavior is unchanged.

Previous units and rollout evidence remain under
`/var/lib/silicon-briefcase/releases/sessions-1.1.1-e33e20a`. Rollback restores the
previous browser drop-in, reloads systemd and restarts only the browser; retain the
session directory for recovery. No database migration or rollback is involved.

A fallback temporary EC2 build attempt hit the account's on-demand vCPU quota
before an instance was launched. Its temporary role, instance profile, policy,
security group and source handoff object version were deleted once GitHub's ARM64
runner became available. No builder instance or disk remains.

Detailed nonsecret local receipts are in `/tmp/session-release-20260922` on the
release operator's host. Credentials are excluded from this document.
