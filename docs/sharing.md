# Sharing files

Every Carbon and Silicon has a personal drive. The drive owner controls its files; a Carbon custodian also manages the drives of Silicons it looks after. Other accounts need explicit grants. There is no organization-public area or tag-derived access in Briefcase 4.

Invite an exact `c:` or `si:` id. Briefcase resolves it at Accounts and stores the immutable UUID, so an id change never transfers a grant to another account. Invitation lists include the current public label when known. Grant `read`, `write` (add children), or `update` (edit existing entries). Delete cannot be shared. Folder grants may inherit to descendants. Traversal to a shared child reveals no unrelated siblings.

Silicons receive shares from their custodian and sibling Silicons automatically. A custodian may explicitly allow or disallow another account through `/silicons/{silicon}/allowed-accounts/{account}`. This recipient policy is checked separately from the sender's file authority.

## Temporary access

Use `expires_in_minutes` for read-only access lasting 1–43,200 minutes. Access ends strictly at its deadline. Change a live grant's deadline, make it permanent, or revoke it. A permanent grant must be revoked before replacing it with an expiring grant.

Anyone-with-the-link access is a separate read/download capability. It can be permanent or expiring, applies through folder inheritance, and can be turned off independently of account grants. Drive roots and reserved app containers cannot have public links. Link responses tell the UI whether the caller may manage them.

Self-destruction is different: it deletes the file itself permanently. A self-destructing upload bypasses the bin even if removed early. `keep` removes the timer. Normal file deletion stays recoverable in the drive's bin for 45 days.

Sharing changes enter the account's notification inbox and the entry's retained activity. Email can be sent only when the recipient shared its email with Briefcase and mail delivery is configured. Public ids, display names and app attribution are labels; authorization always uses verified identity and file rights.
