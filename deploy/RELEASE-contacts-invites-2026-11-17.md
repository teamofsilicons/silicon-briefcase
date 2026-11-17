# Briefcase 5.1 — Contacts and invitation suggestions

The API, worker, website and documentation were activated on 2026-11-17.
The live contract reports 5.1.0 with 65 operations. The Rust client 5.1.0 is
published on crates.io. Silicon Apps publishes CLI 5.1.0 for all six targets.
The store-installed macOS ARM64 CLI passed production contract verification.
Release evidence is in
`verification-contacts-invites-production-2026-11-17.json`.

## Behavior

- The invitation picker fuzzy-matches Carbon and Silicon names and IDs. It
  ranks matches by `2 × invitations sent + invitations received`, then most
  recent invitation, then saved-contact trust descending. Selecting or
  entering an exact ID shows the recipient's name and profile picture.
- The Contacts page connects the current account's Silicon Contacts address
  book after an explicit Connect action. Contacts can be added and edited in
  Briefcase, with trust levels 1–255 and the same data as Silicon Contacts.
  Saved names and private contact photos override the ordinary account
  profile; account pictures and initials remain fallbacks.
- Shared with me filters by the actual sharer, saved/unsaved contacts and
  minimum trust. Filtering happens before pagination and includes direct,
  organization and team grants. Contact labels also appear in sharing rows.
- Short-lived proofs restrict access to `contacts.read` or `contacts.write`.
  Browser requests remain behind the existing authenticated BFF. Contact
  photos use an authenticated private proxy, and disconnection clears contact
  projections from the browser cache.

## Deployment

The deployed source is `98a9b154ac601f5685c123f175c634612a29cb93`, merged in
[PR #46](https://github.com/teamofsilicons/silicon-briefcase/pull/46). This
includes the work-account fix from PR #42. Its migration 0054 had already been
applied to production; Contacts therefore uses migration **0055**.

A fresh database and configuration backup was copied to encrypted private
storage. The database restored into an isolated PostgreSQL container with all
136 entries present. Migration 0055 was then rehearsed against that copy,
running as the existing `briefcase_migrator` schema owner. It preserved all
entries and backfilled eight retained sender/recipient history pairs.

The same migration was applied in production. Custom role
`briefcase_accounts_api` received SELECT/INSERT/DELETE on contact connections,
SELECT on invitation history and EXECUTE on the narrow shared-file discovery
function. Reads without an actor saw zero connection/history rows. API and
worker use the same immutable image. Web configuration, session encryption
key and durable session volume were preserved. Docs switched atomically.

Image digests, backup and SSM command IDs are in the production verification
record. The restricted server release directory is
`/var/lib/silicon-briefcase/releases/contacts-5.1.0-98a9b15`.

For rollback, restore the saved API/worker/web service units and docs symlink,
then restart the three services. Preserve migrations 0054 and 0055 and all
stored data; the preceding runtime tolerates their additive changes. Matching
CLI versions matter because `listSharedWithMe` now has operation revision
1.1.0. Do not run an older migration binary against this schema.

## Verification

All four CI jobs and all service image builds passed. The final backend run
reported 244 passing tests with the PostgreSQL suites enabled, including 220
library tests. The optional S3 test skips without configuration; hosted storage
was checked separately. Client/CLI tests, 60 web unit tests, typecheck, lint,
production builds and nine focused browser scenarios passed. Full details and
baseline browser-suite failures are retained in the local verification record.

Live verification used only `si:saket-agents`, with an independent browser
session and the separate shared CLI home. It exercised Contacts connection,
contact creation and editing, trust changes, saved-name suggestions, fuzzy
keyboard selection, an actual profile picture, shared-file filter requests,
desktop/mobile layouts, disconnection, anonymous denial and sign-out. No
browser JavaScript errors or horizontal overflow occurred. The CLI verified
the live contract, uploaded a temporary file to hosted storage and downloaded
identical bytes. The temporary file/contact were removed, the original
Contacts connection state restored, and the isolated browser session revoked.

Production filter requests used an account without incoming shares; populated
direct and group sharing were checked locally. No invitation or notification
was sent in production. Counter updates, retries, weighted ranking and custom
saved-photo proxy behavior were covered by isolated tests. The signed-in live
BFF check did not exercise interactive hosted OAuth completion. Invitation
backfill can recover retained audit events only, not previously purged history.
