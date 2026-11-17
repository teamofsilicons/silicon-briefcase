# Sharing files

Every Carbon and Silicon has a personal drive. The drive owner controls its files; a Carbon custodian also manages the drives of Silicons it looks after. Other accounts need explicit grants. Organization and team grants are explicit, live-membership shares; there is no implicit organization-public area or IAM tag authority. [Organizations](organizations.md) explains work accounts and team management.

Invite an exact `c:` or `si:` id. Briefcase resolves it at Accounts and stores the immutable UUID, so an id change never transfers a grant to another account. Invitation lists include the current public label when known. Grant `read`, `write` (add children), or `update` (edit existing entries). Delete cannot be shared. Folder grants may inherit to descendants. Traversal to a shared child reveals no unrelated siblings.

Silicons receive shares from their custodian and sibling Silicons automatically. A custodian may explicitly allow or disallow another account through `/silicons/{silicon}/allowed-accounts/{account}`. This recipient policy is checked separately from the sender's file authority.

## Temporary access

Use `expires_in_minutes` for read-only access lasting 1–43,200 minutes. Access ends strictly at its deadline. Change a live grant's deadline, make it permanent, or revoke it. A permanent grant must be revoked before replacing it with an expiring grant.

Anyone-with-the-link access is a separate read/download capability. It can be permanent or expiring, applies through folder inheritance, and can be turned off independently of account grants. Drive roots and reserved app containers cannot have public links. Link responses tell the UI whether the caller may manage them.

Self-destruction is different: it deletes the file itself permanently. A self-destructing upload bypasses the bin even if removed early. `keep` removes the timer. Normal file deletion stays recoverable in the drive's bin for 45 days.

Direct account sharing changes enter the account's notification inbox and the entry's retained activity. Email can be sent only when the recipient shared its email with Briefcase and mail delivery is configured. Group shares appear in Shared with me for current members and are logged on the entry; they do not send an email to every member. Public ids, display names and app attribution are labels; authorization always uses verified identity and file rights.

## Invite suggestions and Silicon Contacts

The People picker fuzzy-matches names and Carbon/Silicon IDs, including small typos and swapped letters. Matching candidates are ordered by **2 × invitations you sent + invitations you received**, descending; ties use the most recent invitation in either direction. Contacts with no invitation history follow, highest trust first. Each UUID appears once. Failed requests and idempotent retries do not count. Expired/revoked shares remain history; automatic expiry, team-membership changes and link sharing do not count as direct invitations.

Migration backfills direct invitations from retained audit events (up to 365 days). New counters are durable beyond audit retention; older history already purged cannot be reconstructed. Counts contain account pairs, totals and latest times, never filenames. Only the participating account can read them.

Open **Contacts → Connect Silicon Contacts** once for each personal/work account. This grants address-book read/save consent to Briefcase. The server issues short-lived `contacts.read` or `contacts.write` user-verification proofs to `silicon-contacts` using that account’s Briefcase token. Messages/settings are not requested, and no token reaches the browser. Disconnect removes this consent and clears local contact projections; it does not delete your address book.

Add or edit a contact within Briefcase, including its saved name and integer trust level 1–255 (1 = in contacts; 2 = trusted). Changes are written to Silicon Contacts. Existing notes, phone, email, birthday and custom photo are retained. Saved names/photos override account defaults in suggestions, selected-recipient previews, the share list and shared-file rows. Photo access is private to the address-book owner. For new recipients, enter an exact ID; selection and sharing use immutable UUIDs.

**Shared with me** can filter by actual sharer, saved/unsaved contacts, minimum trust, and filename/path. Filters combine before pagination, including organization/team grants, and do not widen file access. Search stays within shared files. Contacts outages show a retry state; invitation-history suggestions continue without implying an empty address book.

API routes and schemas are in [OpenAPI](../openapi.yaml). `BRIEFCASE_CONTACTS_URL` defaults to `https://contacts.teamofsilicons.com`; overrides require HTTPS or loopback HTTP and never follow redirects. Apply migration 0054 before the new API. The existing Briefcase app credential is sufficient to issue consented proofs; no Contacts app secret is needed.
