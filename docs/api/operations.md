# Operation inventory

Contract **4.1.0**, API namespace **v1** (`/api/v1`). Generated from [openapi.yaml](../../openapi.yaml), which is
the wire reference; the service serves the same list at `GET /api/version`, and a client checks every operation's
id, method, path and revision there before its first call.

- **bearer**: a Silicon Accounts access token issued to `briefcase` (`Authorization: Bearer …`).
- **proof**: a User verification proof from another app (`Authorization: Proof sap_…`) carrying the operation's
  scope. The app acts for the account inside its own folder `apps/{app}/` of that account's drive; first-party apps
  the operator lists reach everything the account can, through these routes only.
- **upload capability**: the one-time capability a reservation returned (`X-Briefcase-Upload-Capability`), and no
  `Authorization` header.
- **none**: public (discovery, version, anyone-with-the-link reads).

Every account has one drive. Operations on an entry act in the drive that holds it; listings, uploads, the bin,
usage and storage settings take `drive` (the owner's `c:`/`si:` id or uuid) and default to the caller's own.
Permanent addresses read `https://briefcase.teamofsilicons.com/{owner id}/{path}` and resolve at
`GET /paths/{owner}/{path}`; an id the owner gave up answers `308`, and addresses from before personal drives
(`/org/{org}/{path}`) keep resolving.

| Operation | Method and path | Revision | Credential |
| --- | --- | --- | --- |
| `readApiVersion` | `GET /version` | 1.0.0 | none |
| `readAccountsInfo` | `GET /accounts` | 1.0.0 | none |
| `readAccount` | `GET /me` | 1.0.0 | bearer |
| `listDrives` | `GET /drives` | 1.0.0 | bearer |
| `listSharedWithMe` | `GET /shared` | 1.0.0 | bearer |
| `listSiliconAllowedAccounts` | `GET /silicons/{silicon}/allowed-accounts` | 1.0.0 | bearer |
| `allowAccountToShareWithSilicon` | `PUT /silicons/{silicon}/allowed-accounts/{account}` | 1.0.0 | bearer |
| `disallowAccountToShareWithSilicon` | `DELETE /silicons/{silicon}/allowed-accounts/{account}` | 1.0.0 | bearer |
| `listEntries` | `GET /entries` | 2.0.0 | bearer |
| `createFolder` | `POST /entries` | 2.0.0 | bearer |
| `getEntry` | `GET /entries/{entry_id}` | 1.0.0 | bearer |
| `updateEntry` | `PATCH /entries/{entry_id}` | 1.1.0 | bearer |
| `moveEntryToBin` | `DELETE /entries/{entry_id}` | 1.0.0 | bearer |
| `readEntryContent` | `GET /entries/{entry_id}/content` | 1.0.0 | bearer |
| `downloadEntry` | `GET /entries/{entry_id}/download` | 1.0.0 | bearer |
| `uploadFile` | `POST /uploads` | 1.1.0 | bearer |
| `listPermissions` | `GET /entries/{entry_id}/permissions` | 1.0.0 | bearer |
| `grantPermission` | `POST /entries/{entry_id}/permissions` | 1.1.0 | bearer |
| `revokePermission` | `DELETE /entries/{entry_id}/permissions/{grant_id}` | 1.0.0 | bearer |
| `inspectEffectivePermissions` | `POST /permissions/effective` | 1.1.0 | bearer |
| `searchFiles` | `GET /search` | 1.0.0 | bearer |
| `submitTelemetry` | `POST /telemetry` | 1.0.0 | none (retired: 410; Briefcase records no telemetry) |
| `submitReport` | `POST /reports` | 1.0.0 | bearer |
| `listNotifications` | `GET /notifications` | 1.1.0 | bearer |
| `readNotifications` | `POST /notifications/read` | 1.0.0 | bearer |
| `listEntryActivity` | `GET /entries/{entry_id}/activity` | 1.0.0 | bearer |
| `createFileOnBehalfOfMember` | `POST /obo/files` | 3.0.0 | none (retired: 410) |
| `reserveDelegatedUpload` | `POST /obo/uploads/reserve` | 3.0.0 | proof |
| `commitDelegatedUpload` | `POST /obo/uploads/commit` | 3.0.0 | proof |
| `getDelegatedUploadStatus` | `POST /obo/uploads/status` | 3.0.0 | proof |
| `cancelDelegatedUpload` | `POST /obo/uploads/cancel` | 3.0.0 | proof |
| `transferDelegatedUpload` | `PUT /obo/uploads/{upload_id}/content` | 2.1.0 | upload capability |
| `createFolderOnBehalfOfMember` | `POST /obo/folders/create` | 3.0.0 | proof |
| `listEntriesOnBehalfOfMember` | `POST /obo/entries/list` | 3.1.0 | proof |
| `readFileOnBehalfOfMember` | `POST /obo/files/read` | 3.0.0 | proof |
| `trashEntryOnBehalfOfMember` | `POST /obo/entries/trash` | 3.1.0 | proof |
| `listVersions` | `GET /entries/{entry_id}/versions` | 1.0.0 | bearer |
| `restoreVersion` | `POST /entries/{entry_id}/versions/{version_id}/restore` | 1.0.0 | bearer |
| `readOrganizationUsage` | `GET /usage` | 2.0.0 | bearer |
| `listBin` | `GET /bin` | 1.1.0 | bearer |
| `restoreEntry` | `POST /bin/{entry_id}/restore` | 1.0.0 | bearer |
| `configureOrganizationBucket` | `PUT /storage/configuration` | 2.0.0 | bearer |
| `resolvePermanentUrl` | `GET /paths/{owner}/{path}` | 2.0.0 | bearer, or none for link-shared entries |
| `resolveDriveTop` | `GET /paths/{owner}` | 1.0.0 | bearer |
| `resolveLegacyPermanentUrl` | `GET /org/{org_id}/{path}` | 1.0.0 | bearer, or none for link-shared entries (deprecated) |
| `listArchiveContents` | `GET /entries/{entry_id}/archive` | 1.0.0 | bearer |
| `listInvitations` | `GET /entries/{entry_id}/invitations` | 1.0.0 | bearer |
| `createInvitation` | `POST /entries/{entry_id}/invitations` | 2.0.0 | bearer |
| `changeExpiringShare` | `PATCH /entries/{entry_id}/invitations/{grant_id}` | 1.0.0 | bearer |
| `revokeInvitation` | `DELETE /entries/{entry_id}/invitations/{grant_id}` | 1.0.0 | bearer |
| `makeEntryPermanent` | `DELETE /entries/{entry_id}/self-destruct` | 1.0.0 | bearer |
| `readLinkAccess` | `GET /entries/{entry_id}/link-access` | 1.1.0 | bearer |
| `setLinkAccess` | `PUT /entries/{entry_id}/link-access` | 1.2.0 | bearer |
| `listEntryLogs` | `GET /entries/{entry_id}/logs` | 2.0.0 | bearer |
| `readPublicEntry` | `GET /public/{owner}/{path}` | 2.0.0 | none |
| `inviteOnBehalfOfMember` | `POST /obo/invitations` | 3.0.0 | proof |
| `setLinkAccessOnBehalfOfMember` | `POST /obo/link-access` | 3.0.0 | proof |
