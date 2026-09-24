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

Add `expires_in_minutes` to make it an [expiring share](#expiring-shares) that ends by itself.

## Anyone with the link

Use the browser's Access tab, `briefcase link <path> --enabled true`, or `PUT /entries/{entry_id}/link-access` with `{"enabled":true}` and an idempotency key. The response includes `url`, the shareable website address for the file or folder whenever effective link access is enabled (including inherited access), or `null` when it is private. The CLI returns this URL in its JSON result, and the browser Access tab displays it with a **Copy share link** button. Testing-environment links require the recipient to select the same testing environment; secrets are never embedded in URLs.

The response reports the entry's explicit `enabled` value, effective access, and nearest `inherited_from` folder ID.

Add `expires_in_minutes` to make it an [expiring link](#expiring-links) that turns itself off. The response's `expires_at` is when this entry's own expiring link ends, or `null` when its link is permanent or off.

Anyone can view and download a shared entry without signing in. Folder sharing applies dynamically to every descendant, including new children. Turning off a child's explicit setting does not override a shared parent. Disable the parent setting to remove inherited public access; descendants with their own setting remain public.

The organization private container and per-member private roots, including the app-private containers, cannot be publicly shared. Ordinary files and folders beneath them can be. Reserved public/tag roots may be shared only by organization administrators. Link sharing grants no upload, update, invitation, or delete authority.

Use the permanent application URL for people:

```text
https://briefcase.teamofsilicons.com/org/tos/private/me:tos/reports/
```

For unauthenticated programs, use `GET /public/{org_id}/{path}`. `view=metadata` returns safe metadata; `view=contents` lists one folder page with `cursor`; `view=inline` renders a file; `view=attachment` downloads a file or `.tar.zst` folder archive. A revoked or hidden link returns 404. Responses are not cacheable. Public metadata does not expose owners, permission grants, or activity.

## Expiring shares

This is the **Expires after** feature in UNDERSTANDING.md and the web app: any share can be set to expire after a chosen time.

An expiring share ends by itself. Add `expires_in_minutes` when you share, a whole number from 1 to 43,200 (one minute to 30 days), and the access it gives stops working the instant its `expires_at` passes. Use one when a Carbon, a Silicon or an outside reader needs a file or folder for a while and should not keep it: nobody has to remember to revoke it.

- **Any share.** Invitations to a member ID, verified email or tag (`POST /entries/{entry_id}/invitations`), explicit member grants (`POST /entries/{entry_id}/permissions`), and anyone with the link (`PUT /entries/{entry_id}/link-access`), on files and folders.
- **Read only.** An expiring share gives view and download. Asking for `write` or `update` with `expires_in_minutes` is refused with 422 `expiring_share_is_read_only`.
- **Its own grant.** An expiring share is a separate grant, even when the same member or tag already holds a permanent one on the entry. When it ends, only the access it gave goes. Access from another invitation, a tag, or a public folder stays.
- **Strict expiry.** Every request checks `expires_at`: permission checks, listings, filters, downloads and public link reads. Nothing waits for a cleanup job. An expired share disappears from the invitation and permission listings.
- **Quiet ending.** Creating an expiring invitation sends the usual notification and email. Nothing is sent when an expiring share expires or is ended early.

```json
{"principal":{"type":"carbon","id":"alex:tos"},"access":["read"],"inherit":true,"expires_in_minutes":90}
```

Invitations and permission grants report `expires_at`, an RFC 3339 time for an expiring share and `null` for a permanent grant.

### Change or end an expiring share

`PATCH /entries/{entry_id}/invitations/{grant_id}` (`changeExpiringShare`) changes a live expiring share. It needs manage-permissions authority on the entry and an `Idempotency-Key`. Send exactly one field:

| Body | Effect |
| --- | --- |
| `{"expires_in_minutes":N}` | Restarts the clock from now: the share now ends N minutes after this request. Use it to extend or to shorten. |
| `{"permanent":true}` | Keeps the access for good. If the recipient already holds a permanent grant on this entry, the expiring share folds into it, that grant gains the expiring share's inheritance, and the response is the permanent grant. |

The 200 response is the grant as it now stands, in the invitation shape. Both fields or neither is 422 `expires_in_minutes_or_permanent`. A share that has already expired no longer exists and returns 404. A permanent grant returns 409 `not_an_expiring_share`.

To end an expiring share early, revoke it like any other grant: `DELETE /entries/{entry_id}/invitations/{grant_id}` or `DELETE /entries/{entry_id}/permissions/{grant_id}`. No notification is sent.

### Expiring links

`PUT /entries/{entry_id}/link-access` with `{"enabled":true,"expires_in_minutes":N}` turns on anyone-with-the-link access for N minutes. On a folder it covers everything inside until it expires. After that, public reads return 404 unless another link setting still covers the entry.

| Request | Current link | Result |
| --- | --- | --- |
| `enabled: true` + `expires_in_minutes` | Off | Expiring link for N minutes |
| `enabled: true` + `expires_in_minutes` | Live expiring link | Clock restarts from now |
| `enabled: true` + `expires_in_minutes` | Permanent link already on | 409 `link_already_permanent`; an expiring link never shortens a permanent one |
| `enabled: true`, no minutes | Live expiring link | Link becomes permanent |
| `enabled: false` | Any | Link ends now |
| `enabled: false` + `expires_in_minutes` | Any | 422 `expiring_link_requires_enabled` |

Apps can create both kinds of expiring share through the critical OBO endpoints; see [OBO](obo.md#critical-sharing-operations).

## Invitation emails

The worker sends transactional invitations through Postmark from **briefcase@teamofsilicons.com**. Configure `BRIEFCASE_POSTMARK_SERVER_TOKEN` and verify that sender or domain in Postmark. The API commits the permission, notification, audit event, and email outbox record together. Mail delivery is asynchronous, bounded to four concurrent deliveries, leased, retried with backoff, and dead-lettered after the configured attempt limit. An interrupted delivery can be retried, so mail is at least once.

IAM 1.7 exposes an application's subject's own verified email through `self.email.read`; it does not expose other members' email addresses to this application. Briefcase records the authenticated subject's verified contact during login-status inspection. Email invitations resolve only against those known contacts and still verify current membership. A member who has never provided a contact through an authenticated session cannot yet be resolved by email: invite by member ID instead. A Silicon without an IAM email receives the in-app notification; its email outbox record reports `invitation_contact_unavailable`.

Testing environments exercise notifications and the durable email outbox without sending messages to real addresses. Never put Postmark credentials in the browser, CLI profile, or Rust client.

## Logs and versions

`GET /entries/{id}/logs` and `briefcase logs <path>` return up to 100 events per page for the preceding **365 days**. Continue with `cursor` / `--cursor`. Logs include permissions, invitation recipients, link settings, content updates, version restores, moves and deletion. Folder logs include descendant changes. A reader with partial access cannot use folder logs to inspect hidden children.

Expiring shares and self-destructing files add these actions. A parent folder receives the usual `child.`-prefixed copy of each.

| Action | Written when |
| --- | --- |
| `permission.expiring_share_granted.v1`, `permission.tag_expiring_share_granted.v1` | A member or tag expiring share is created |
| `permission.expiring_share_changed.v1` | An expiring share's clock is restarted |
| `permission.expiring_share_made_permanent.v1` | An expiring share is made permanent |
| `permission.expiring_share_revoked.v1`, `permission.tag_expiring_share_revoked.v1` | An expiring share is ended early |
| `permission.expiring_share_expired.v1`, `permission.tag_expiring_share_expired.v1` | An expiring share expired |
| `entry.expiring_link_enabled.v1`, `entry.expiring_link_changed.v1` | An expiring link is turned on, or its clock restarted |
| `entry.expiring_link_made_permanent.v1`, `entry.expiring_link_revoked.v1` | An expiring link is made permanent, or turned off early |
| `entry.expiring_link_expired.v1` | An expiring link expired |
| `entry.self_destruct_set.v1` | A file is uploaded with a timer; metadata has `minutes` and `self_destruct_at` |
| `entry.made_permanent.v1` | A self-destructing file is kept |
| `entry.self_destructed.v1` | The timer ran out and the file was deleted |
| `entry.self_destruct_deleted.v1` | A self-destructing file was deleted by hand |

The worker writes expiry and self-destruct events with metadata `automatic: true` and `request_id` `worker:share-expiry` or `worker:self-destruct`. An expiry event is dated at the instant the share ended, not when the worker noticed, and is attributed to the member who shared it; for a link, the entry's owner. `entry.self_destructed.v1` is attributed to the file's creator.

`GET /entries/{id}/activity` and `briefcase history <path>` provide the latest 100 activity events. This presentation limit does not discard the retained year of logs.

File versions are a separate immutable history, retained until the file is permanently purged from the 45-day bin. A [self-destructing file](api/README.md#self-destructing-files) skips the bin and loses its versions when it is deleted. Uploading the same name updates the same file ID with a monotonically increasing version. Versions report SHA-256, byte length, actor, time and source (`initial_upload`, `upload`, `restore`). Restoring creates another version and consumes storage; it does not rewrite history.
