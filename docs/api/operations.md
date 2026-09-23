# Operation inventory

> Shared testing lifecycle has moved to Honeycomb. Legacy management routes beneath
> `/organizations/{org_id}/testing-environments` and the selected-plane cleaning route
> now return `testing_environment_managed_by_honeycomb`. Their historical shapes below
> are compatibility references, not supported lifecycle operations. Use
> [Honeycomb management](../testing-environments.md) and retain ordinary app-secret
> data-plane selection. The protected [participant contract](../honeycomb-integration.md)
> is a separate service integration.

Contract **2.0.0**, API route namespace **v1**. The 60 operations carry the revisions listed below. Identity-bearing IAM, session, testing, delegated, and log operations use revision **2.0.0** for bare application IDs and typed actor IDs. Unchanged operations retain their previous revisions. Paths are relative to `/api/v1` except permanent `/org/…` URLs. See the [HTTP guide](README.md), [Rust client](../client/README.md), and [CLI](../cli/README.md).

| Operation | Method and path | Revision |
| --- | --- | --- |
| `readIamInfo` | `GET /iam` | 2.0.0 |
| `readLoginStatus` | `GET /auth/status` | 2.0.0 |
| `readApiVersion` | `GET /version` | 1.0.0 |
| `exchangeShortLivedToken` | `POST /auth/slt` | 2.0.0 |
| `refreshApplicationSession` | `POST /auth/refresh` | 2.0.0 |
| `listTestingEnvironments` | `GET /organizations/{org_id}/testing-environments` | 2.0.0 |
| `createTestingEnvironment` | `POST /organizations/{org_id}/testing-environments` | 2.0.0 |
| `getTestingEnvironment` | `GET /organizations/{org_id}/testing-environments/{environment_id}` | 2.0.0 |
| `updateTestingEnvironment` | `PATCH /organizations/{org_id}/testing-environments/{environment_id}` | 1.0.0 |
| `deleteTestingEnvironment` | `DELETE /organizations/{org_id}/testing-environments/{environment_id}` | 1.0.0 |
| `getTestingEnvironmentKey` | `GET /organizations/{org_id}/testing-environments/{environment_id}/key` | 1.0.0 |
| `replaceTestingEnvironmentIamPairing` | `POST /organizations/{org_id}/testing-environments/{environment_id}/iam-pairings` | 2.0.0 |
| `cleanTestingEnvironment` | `POST /organizations/{org_id}/testing-environments/{environment_id}/cleanings` | 1.0.0 |
| `restoreTestingEnvironment` | `POST /organizations/{org_id}/testing-environments/{environment_id}/restorations` | 1.0.0 |
| `describeCurrentTestingEnvironment` | `GET /testing-environment` | 2.0.0 |
| `cleanCurrentTestingEnvironment` | `POST /testing-environment/cleanings` | 1.0.0 |
| `listEntries` | `GET /entries` | 1.0.0 |
| `createFolder` | `POST /entries` | 1.0.0 |
| `getEntry` | `GET /entries/{entry_id}` | 1.0.0 |
| `updateEntry` | `PATCH /entries/{entry_id}` | 1.0.0 |
| `moveEntryToBin` | `DELETE /entries/{entry_id}` | 1.0.0 |
| `readEntryContent` | `GET /entries/{entry_id}/content` | 1.0.0 |
| `downloadEntry` | `GET /entries/{entry_id}/download` | 1.0.0 |
| `uploadFile` | `POST /uploads` | 1.0.0 |
| `listPermissions` | `GET /entries/{entry_id}/permissions` | 1.0.0 |
| `grantPermission` | `POST /entries/{entry_id}/permissions` | 1.0.0 |
| `revokePermission` | `DELETE /entries/{entry_id}/permissions/{grant_id}` | 1.0.0 |
| `inspectEffectivePermissions` | `POST /permissions/effective` | 1.0.0 |
| `searchFiles` | `GET /search` | 1.0.0 |
| `submitReport` | `POST /reports` | 1.0.0 |
| `listNotifications` | `GET /notifications` | 1.0.0 |
| `readNotifications` | `POST /notifications/read` | 1.0.0 |
| `listEntryActivity` | `GET /entries/{entry_id}/activity` | 1.0.0 |
| `createFileOnBehalfOfMember` | `POST /obo/files` | 2.0.0 |
| `reserveDelegatedUpload` | `POST /obo/uploads/reserve` | 2.0.0 |
| `commitDelegatedUpload` | `POST /obo/uploads/commit` | 2.0.0 |
| `getDelegatedUploadStatus` | `POST /obo/uploads/status` | 2.0.0 |
| `cancelDelegatedUpload` | `POST /obo/uploads/cancel` | 2.0.0 |
| `transferDelegatedUpload` | `PUT /obo/uploads/{upload_id}/content` | 2.0.0 |
| `createFolderOnBehalfOfMember` | `POST /obo/folders/create` | 2.0.0 |
| `listEntriesOnBehalfOfMember` | `POST /obo/entries/list` | 2.0.0 |
| `readFileOnBehalfOfMember` | `POST /obo/files/read` | 2.0.0 |
| `trashEntryOnBehalfOfMember` | `POST /obo/entries/trash` | 2.0.0 |
| `listVersions` | `GET /entries/{entry_id}/versions` | 1.0.0 |
| `restoreVersion` | `POST /entries/{entry_id}/versions/{version_id}/restore` | 1.0.0 |
| `readOrganizationUsage` | `GET /usage` | 1.0.0 |
| `listBin` | `GET /bin` | 1.0.0 |
| `restoreEntry` | `POST /bin/{entry_id}/restore` | 1.0.0 |
| `configureOrganizationBucket` | `PUT /storage/configuration` | 1.0.0 |
| `resolvePermanentUrl` | `GET /org/{org_id}/{path}` | 1.1.0 |
| `listInvitations` | `GET /entries/{entry_id}/invitations` | 1.0.0 |
| `createInvitation` | `POST /entries/{entry_id}/invitations` | 1.0.0 |
| `revokeInvitation` | `DELETE /entries/{entry_id}/invitations/{grant_id}` | 1.0.0 |
| `readLinkAccess` | `GET /entries/{entry_id}/link-access` | 1.1.0 |
| `setLinkAccess` | `PUT /entries/{entry_id}/link-access` | 1.1.0 |
| `listEntryLogs` | `GET /entries/{entry_id}/logs` | 2.0.0 |
| `readPublicEntry` | `GET /public/{org_id}/{path}` | 1.1.0 |
| `inviteOnBehalfOfMember` | `POST /obo/invitations` | 2.0.0 |
| `setLinkAccessOnBehalfOfMember` | `POST /obo/link-access` | 2.0.0 |
| `submitTelemetry` | `POST /telemetry` | 1.0.0 |
