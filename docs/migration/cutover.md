# Briefcase cutover to Silicon Accounts: runbook notes

Notes for the production switch, collected by each stage. Nothing here has been run against production.

## From service part 1 (identity), 2026-10-10

Before deploying the new service:

1. **Silicon Accounts app `briefcase`** (with the app's own credentials, never from the Briefcase CLI):
   sign-in setup allows `https://briefcase.teamofsilicons.com/auth/callback`, `device_flow: true`,
   `public_client: true`, optional field `email`. Set the webhook:
   `PUT /v1/apps/briefcase/webhook {"url": "https://backend.briefcase.teamofsilicons.com/webhook/"}` and keep
   the returned `whsec_…` secret (a PUT keeps an existing secret and returns `null`).
2. **Secrets and settings** on the base-tier host (`/etc/silicon-briefcase/*.env` / Secrets Manager
   `silicon-briefcase/production`): add `BRIEFCASE_APP_SECRET`, `BRIEFCASE_ACCOUNTS_WEBHOOK_SECRET`,
   `BRIEFCASE_PROOF_ISSUERS` (production value in decisions.md S4); set `ACCOUNTS_URL=https://accounts.teamofsilicons.com`
   and `BRIEFCASE_APP_ID=briefcase`; remove every `BRIEFCASE_IAM_*`, `BRIEFCASE_HONEYCOMB_*`,
   `BRIEFCASE_TEST_ENVIRONMENT_ENCRYPTION_KEY`, `BRIEFCASE_TEST_DATABASE_URL` and
   `BRIEFCASE_TEST_WORKER_DATABASE_URL`. The API refuses to start without the app secret or webhook secret.
3. **Migrate** with `briefcase-migrate` (adds migration 0048; no existing data changes).
4. **Drain delegated uploads** made under IAM (`delegated_uploads` rows in `reserved`/`receiving`/`staged`):
   their identity keys will not match proof-authenticated calls after the switch. Same for
   `idempotency_records` still `in_progress`.

Switch together (they break otherwise):

- **Interface** (outside this migration) must switch its Files feature to User verification proofs at the same
  time: it uses the removed `/api/v1/auth/slt` + `/auth/refresh` broker and `X-Org-ID` bearer calls today. With
  `BRIEFCASE_PROOF_ISSUERS` it may use `briefcase.entries.list`, `briefcase.files.read`,
  `briefcase.folders.create`, `briefcase.uploads.*`, `briefcase.invitations.create`,
  `briefcase.link_access.update` (confined to `apps/interface/` until part 3 widens it). Otherwise disable
  Interface Files at the cutover.
- **Waveform, Extend, Silicon Browser** call `/api/v1/obo/*` with IAM OBO headers through the vendored
  `briefcase-client` 3.x: they must send `Authorization: Proof sap_…` (User verification for receiving app
  `briefcase` with the scopes above) and, for the byte transfer, `X-Org-ID: <the proof user's uuid>`.
- **Stemcell** checks `briefcase iam --json` and installs through Honeycomb (CLI stage handles the alias).

After the switch: IAM webhook deliveries to `/webhook/` answer 401 (expected); remove the IAM webhook
registration. The testing-plane database and the Honeycomb/testing tables are no longer used; back them up and
decommission with the Carbon's approval.

Open for parts 2 and 3: legacy org tenants (`tos`) are not reachable until the identity re-keying and drive
restructure run; do not cut over production before those parts land.
