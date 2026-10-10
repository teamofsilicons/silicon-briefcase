# Briefcase in your browser

The Next.js application uses Silicon Accounts hosted sign-in. Open Briefcase and select **Sign in**. Approve the account and requested details at Accounts. The server seals the sign-in in an httpOnly cookie; file requests go through the same site's API proxy.

## Files

**My files** opens your drive. Create a folder, drop files on the page, or choose **Upload files**. A transfer can be cancelled and retried; each retry retains the original idempotency key. Uploading the same name into the same folder publishes a new immutable version. Upload options can schedule permanent deletion after 1 minute to 30 days.

Use list or grid view, folder breadcrumbs, the search box, and file-type filters. Recent shows the latest changes. Search includes accessible file content. Entries with more pages offer **Load more**. A shared folder can show only the children you may access.

Choose a file to preview or **Details** for either a file or folder. Previews support images, audio, video, PDFs, text and code, Office/OpenDocument text and tables, and archive listings. Text reads are capped at 256 KiB; document inputs at 10 MiB, with bounded decompression. Download originals for full Office formatting, charts and embedded media. Unsupported types remain downloadable. Uploaded HTML and SVG never execute as site code.

## Sharing and history

The **Sharing** tab grants read, add or edit access to an exact `c:` or `si:` id. Optional expiry is read-only. Remove a grant, change an expiring grant's lifetime, or keep it permanently. The owner controls deletion. Silicon recipient allowances are managed in Settings by its custodian.

Enable **Anyone with the link** for anonymous reading or downloads. Disabling it closes anonymous access; invitation grants are independent. Folders pass inherited access to descendants. Public links reveal only accessible children.

**Versions** lists stored versions and restores a selected version. **Activity** records retained changes. **Details** renames, moves within the drive, cancels self-destruction, or moves an ordinary entry to the bin. The bin restores entries within 45 days. Removing a self-destructing file deletes it permanently.

**Shared with me** lists explicit shares. **My Silicons** opens drives of Silicons you look after while you remain the acting Carbon. **Notifications** lists sharing updates and can mark the inbox read.

## Settings

Choose a drive to see storage and daily upload usage. Connect an S3 bucket by supplying its region, role ARN, prefix, account id and encryption. Copy the drive's STS External ID into the role trust policy and use the deployment's Briefcase service role as its trusted principal. Briefcase verifies bucket operations before using the configuration. Existing versions remain in their original storage.

The account menu switches light/dark/system themes and signs out. Sign-out revokes this sign-in and other open tabs follow. Problems can be reported from Settings; only the text you submit is included.

Local setup and browser checks are documented in [web/README.md](../web/README.md).
