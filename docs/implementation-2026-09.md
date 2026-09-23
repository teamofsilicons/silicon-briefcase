# September requirements implementation

Initial scope: implement the September changes in `UNDERSTANDING.md`, initially excluding Space Station. The later telemetry request supersedes that exclusion; see the telemetry verification below. The human-maintained specification is not edited. This checklist records implementation and verification, not just existing claims in documentation.

## Completion gates

- [x] Test app-secret discovery and IAM SLT/active actor-ID login; invalid secrets never select production.
- [x] Test users retain real permissions; separate administrative actions.
- [x] Website entry from sign-in/settings, named environment and identity banner, independent sessions and exit.
- [x] CLI test footer includes environment identity on success, runtime failure, and argument errors without corrupting stdout.
- [x] Test isolation for data, versions, bin, search, caches, jobs, notifications, audit and sharing links.
- [x] Concurrent 2 GiB accounting including reservations, bin and versions; ten active environments including restore races.
- [x] Signed raw-body IAM webhook routing, duplicate/out-of-order handling, no real test delivery or secret persistence in logs/payloads.
- [x] CLI/SDK parity, SLT-only login, SILICON_HOME and optional ISI metadata.
- [x] Durable daemon, service installation and lifecycle controls. Independent CLI and SDK updaters were subsequently removed; Honeycomb owns CLI updates.
- [x] Realtime WebSocket prewarming and outgoing webhook/unhook are excluded by the user clarification: Briefcase uses request/response operations and a pulled inbox. Incoming IAM webhooks remain in scope.
- [x] Bug reports through SDK and CLI, optional PR and contribution guidance.
- [x] Bundled traversable command documentation, examples, actionable errors and repository/docs/package links.
- [x] One-command technical installation and usage-first documentation for users and developers.
- [x] Configurable defaults and organization-owned storage across API, SDK, CLI and website, with existing versions remaining readable.
- [x] Appropriate unit, process, database, integration, UI and documentation checks pass.

## Decisions

The new daemon update requirement supersedes the old command-triggered schedule; the existing update opt-out remains. Shared daemon transport must not merge credentials, identities or production/testing subscriptions. Existing versioned S3 locations remain authoritative for old objects when an organization changes its active storage configuration.

The user clarified that Briefcase does not need outgoing webhook prewarming. The daemon originally provided hourly updates; that updater has since been removed. No outgoing realtime relay is being added.

## Verification evidence

- Backend: all-target/all-feature tests pass against disposable production and test PostgreSQL databases. Sandbox discovery and public-link routing also run with the restricted API database roles. Contract tests cover the 59-operation inventory; sharing/permanent-link revisions are updated to 1.1.0.
- Client workspace: 131 unit, wire, CLI process and gateway tests pass. These cover secret discovery, standalone test sign-in, cookie/environment separation, stderr footers, report idempotency, anonymous sandbox-link routing, daemon singleton/restart behavior and bundled manuals.
- Quotas and lifecycle: storage publication and reservations serialize on the same organization usage row; stored versions remain charged through binning until purge. Create/restore serialize the deployment-wide ten-environment limit with one advisory lock. Existing PostgreSQL usage, version-retention, bin, sandbox lifecycle and limit tests pass.
- IAM and delivery: raw-body signature tests and existing projection ordering/idempotency tests pass. Test email delivery exits through the simulation branch before the production provider is used.
- Website: TypeScript, lint and production build checks pass. The local sign-in UI and invalid-secret feedback were inspected in the browser; IAM sign-in behavior is verified with a mock upstream.
- Distribution/docs: shell syntax, three installer simulations, bundled-manual synchronization, Cargo package contents, Rust formatting/lints and documentation links checked. Service installation is tested with a mocked supervisor; no user login service is installed by verification.

Live AWS object-store tests and the two credentialed client smoke tests were not run. No release was published, deployed or installed globally. These initial checks preceded the telemetry follow-up. Outgoing WebSocket/webhook prewarming remains excluded. The user's edits to `UNDERSTANDING.md` are preserved.


## Telemetry follow-up

The separate telemetry request adds default-on collection across the backend,
worker, CLI, daemon, SDK and browser. The private Space Station table is
`tos/siliconbriefcase`; the key is configured locally outside the repository.
Production rollout still requires provisioning that secret in backend/worker
services and deploying these changes.

- The official Rust client spools and ships events. An explicit rustls provider
  avoids a runtime conflict between Space Station and the existing HTTP client.
- CLI settings and browser settings provide persistent opt-out. SDK callers use
  `Config::with_telemetry(false)`; server processes also support the environment
  override. Opt-out follows login, discovery, refresh and ordinary requests.
- Browser analytics and custom events use one sanitized relay and table.
  Backend tracing uses an allowlist of fixed operational fields. Neither path
  sends filenames, user text, credentials, raw errors or file contents.
- Contract inventory is now 60 operations. Tests cover anonymous relay intake,
  unknown-field rejection, SDK wire headers, CLI persistence, sandbox login,
  tracing correlation/opt-out and browser queue behavior.
- Live check: the actual Briefcase relay and official Space Station collector
  delivered SDK, CLI, daemon and browser-source integration events. Space Station
  acknowledged delivery in 2.15 seconds; the table UI showed eight events,
  including the four spooled during the initial TLS failure. IDs and sources in
  the table matched the test output. These are harmless, labelled test events.

Repeat the live check explicitly with `cargo test --lib telemetry_live_delivery
-- --ignored --nocapture` when the private collector configuration is present.
The normal test suite does not send to Space Station.

Final telemetry checks: backend unit/OpenAPI tests and focused route-name tests
pass; all 134 client workspace tests pass (two credentialed smoke tests remain
explicitly ignored). Both Rust workspaces pass strict Clippy and formatting.
Web TypeScript, lint, two telemetry transport/privacy tests and production build
pass. The actual local browser control starts enabled, switches off and stays
off in a new tab. Docs build and 727 local links/assets pass; bundled manuals
match their source. Production UserData shell syntax and spool mounts were
checked without deployment. The private key is mode 0600 and was absent from
401 scanned source/build files. No production release or secret was modified.
