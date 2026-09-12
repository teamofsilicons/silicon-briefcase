# Sharing and audit logs

Share a file or folder with a current organization member using their Carbon ID, Silicon ID, verified email, or IAM tag. IAM controls membership; an invitation never adds someone to the organization.

## Independent permissions

| Permission | Files | Folders |
| --- | --- | --- |
| Read | Preview and download | List authorized children and download an archive |
| Write | Not grantable | Create new files and folders |
| Update | Replace content, rename, move | Rename or move |
| Delete | Creator, organization admin, or owner | Creator, organization admin, or owner |
| Manage permissions | Creator, organization admin, or owner | Creator, organization admin, or owner |

Every invitation includes read. Update does not imply write or delete. Delete cannot be included in an invitation. Inherited invitations apply to descendants; a tag invitation follows current IAM tag membership instead of creating permanent individual grants. Removing the tag or membership removes that route to access.

```bash
briefcase share private/me:tos/report.pdf carbon:alex:tos --access read,update
briefcase share private/me:tos/reports email:alex@example.com --inherit
briefcase share private/me:tos/reports tag:engineering --access read,write,update --inherit
briefcase shares private/me:tos/reports --json
briefcase unshare private/me:tos/reports <grant-id>
```

The HTTP equivalents are `GET/POST /entries/{entry_id}/invitations` and `DELETE /entries/{entry_id}/invitations/{grant_id}`. Supply an `Idempotency-Key` for invitation writes. A create body is:

```json
{"principal":{"type":"tag","id":"engineering"},"access":["read","update"],"inherit":true}
```

## Anyone with the link

Use the browser's Access tab, `briefcase link <path> --enabled true`, or `PUT /entries/{entry_id}/link-access` with `{"enabled":true}` and an idempotency key. The response reports the entry's explicit `enabled` value, effective access, and nearest `inherited_from` folder ID.

Anyone can view and download a shared entry without signing in. Folder sharing applies dynamically to every descendant, including new children. Turning off a child's explicit setting does not override a shared parent. Disable the parent setting to remove inherited public access; descendants with their own setting remain public.

The organization private container and per-member private roots, including the app-private containers, cannot be publicly shared. Ordinary files and folders beneath them can be. Reserved public/tag roots may be shared only by organization administrators. Link sharing grants no upload, update, invitation, or delete authority.

Use the permanent application URL for people:

```text
https://briefcase.teamofsilicons.com/org/tos/private/me:tos/reports/
```

For unauthenticated programs, use `GET /public/{org_id}/{path}`. `view=metadata` returns safe metadata; `view=contents` lists one folder page with `cursor`; `view=inline` renders a file; `view=attachment` downloads a file or `.tar.zst` folder archive. A revoked or hidden link returns 404. Responses are not cacheable. Public metadata does not expose owners, permission grants, or activity.

## Invitation emails

The worker sends transactional invitations through Postmark from **briefcase@teamofsilicons.com**. Configure `BRIEFCASE_POSTMARK_SERVER_TOKEN` and verify that sender or domain in Postmark. The API commits the permission, notification, audit event, and email outbox record together. Mail delivery is asynchronous, bounded to four concurrent deliveries, leased, retried with backoff, and dead-lettered after the configured attempt limit. An interrupted delivery can be retried, so mail is at least once.

IAM 1.7 exposes an application's subject's own verified email through `self.email.read`; it does not expose other members' email addresses to this application. Briefcase records the authenticated subject's verified contact during login-status inspection. Email invitations resolve only against those known contacts and still verify current membership. A member who has never provided a contact through an authenticated session cannot yet be resolved by email: invite by member ID instead. A Silicon without an IAM email receives the in-app notification; its email outbox record reports `invitation_contact_unavailable`.

Testing environments exercise notifications and the durable email outbox without sending messages to real addresses. Never put Postmark credentials in the browser, CLI profile, or Rust client.

## Logs and versions

`GET /entries/{id}/logs` and `briefcase logs <path>` return up to 100 events per page for the preceding **365 days**. Continue with `cursor` / `--cursor`. Logs include permissions, invitation recipients, link settings, content updates, version restores, moves and deletion. Folder logs include descendant changes. A reader with partial access cannot use folder logs to inspect hidden children.

`GET /entries/{id}/activity` and `briefcase history <path>` provide the latest 100 activity events. This presentation limit does not discard the retained year of logs.

File versions are a separate immutable history, retained until the file is permanently purged from the 45-day bin. Uploading the same name updates the same file ID with a monotonically increasing version. Versions report SHA-256, byte length, actor, time and source (`initial_upload`, `upload`, `restore`). Restoring creates another version and consumes storage; it does not rewrite history.
