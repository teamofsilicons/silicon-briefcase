# API / Rust / CLI operation map

This inventory was checked against the 42 entries in `src/api/versioning.rs`
on 2026-09-05. Paths are relative to `/api/v1`; the version handshake is also
available anonymously at host-root `/api/version`. Operation revisions, not
just the crate's release number, determine client compatibility.

The CLI column omits arguments except where they distinguish routes; use the
[CLI guide](../cli/README.md) and command help for full syntax. The Rust column
names the primary method; convenience and durable `_with_key` forms are
explained in the [client guide](../client/README.md). Request/response details
remain in the [API guide](README.md) and [OpenAPI](../../openapi.yaml).

| Operation / revision | HTTP route | Rust method | CLI surface |
| --- | --- | --- | --- |
| `readApiVersion` 1.0.0 | `GET /version` | `version` | `version` |
| `exchangeShortLivedToken` 1.0.0 | `POST /auth/slt` | `login_with_slt` | `login` |
| `refreshApplicationSession` 1.0.0 | `POST /auth/refresh` | `refresh_session` | `automatic session refresh` |
| `listTestingEnvironments` 1.0.0 | `GET /organizations/{org_id}/testing-environments` | `testing_environments` | `env list` |
| `createTestingEnvironment` 1.0.0 | `POST /organizations/{org_id}/testing-environments` | `create_testing_environment` | `env create` |
| `getTestingEnvironment` 1.0.0 | `GET /organizations/{org_id}/testing-environments/{environment_id}` | `testing_environment` | `env show` |
| `updateTestingEnvironment` 1.0.0 | `PATCH /organizations/{org_id}/testing-environments/{environment_id}` | `update_testing_environment` | `env update` |
| `deleteTestingEnvironment` 1.0.0 | `DELETE /organizations/{org_id}/testing-environments/{environment_id}` | `delete_testing_environment` | `env delete` |
| `getTestingEnvironmentKey` 1.0.0 | `GET /organizations/{org_id}/testing-environments/{environment_id}/key` | `testing_environment_key` | `env key` |
| `rotateTestingEnvironmentKey` 1.0.0 | `POST /organizations/{org_id}/testing-environments/{environment_id}/key-rotations` | `rotate_testing_environment_key` | `env rotate-key` |
| `replaceTestingEnvironmentIamPairing` 1.0.0 | `POST /organizations/{org_id}/testing-environments/{environment_id}/iam-pairings` | `replace_testing_environment_iam_pairing` | `env pair-iam` |
| `cleanTestingEnvironment` 1.0.0 | `POST /organizations/{org_id}/testing-environments/{environment_id}/cleanings` | `clean_testing_environment` | `env clean <UUID>` |
| `restoreTestingEnvironment` 1.0.0 | `POST /organizations/{org_id}/testing-environments/{environment_id}/restorations` | `restore_testing_environment` | `env restore` |
| `describeCurrentTestingEnvironment` 1.0.0 | `GET /testing-environment` | `current_testing_environment` | `--test <UUID> env current` |
| `cleanCurrentTestingEnvironment` 1.0.0 | `POST /testing-environment/cleanings` | `clean_current_testing_environment` | `--test <UUID> env clean` |
| `listEntries` 1.1.0 | `GET /entries` | `list_entries` | `ls / find` |
| `createFolder` 1.1.0 | `POST /entries` | `create_folder` | `mkdir` |
| `getEntry` 1.1.0 | `GET /entries/{entry_id}` | `entry` | `stat <UUID>` |
| `updateEntry` 1.0.0 | `PATCH /entries/{entry_id}` | `update_entry` | `mv` |
| `moveEntryToBin` 1.0.0 | `DELETE /entries/{entry_id}` | `delete_entry` | `rm` |
| `readEntryContent` 1.1.0 | `GET /entries/{entry_id}/content` | `read_content` | `cat` |
| `downloadEntry` 1.1.0 | `GET /entries/{entry_id}/download` | `download` | `get` |
| `resolvePermanentUrl` 1.1.0 | `GET /org/{org_id}/{path}` | `entry_at` | `stat <path>` |
| `uploadFile` 1.1.0 | `POST /uploads` | `upload` | `put` |
| `createFileOnBehalfOfMember` 1.0.0 | `POST /obo/files` | `create_file_on_behalf_of` | `app upload` |
| `listPermissions` 1.0.0 | `GET /entries/{entry_id}/permissions` | `permissions` | `shares` |
| `grantPermission` 1.1.0 | `POST /entries/{entry_id}/permissions` | `grant` | `share` |
| `revokePermission` 1.0.0 | `DELETE /entries/{entry_id}/permissions/{grant_id}` | `revoke` | `unshare` |
| `inspectEffectivePermissions` 1.0.0 | `POST /permissions/effective` | `effective_access` | `access` |
| `requestAccess` 1.0.0 | `POST /entries/{entry_id}/access-requests` | `request_access` | `request <UUID>` |
| `requestAccessByPath` 1.0.0 | `POST /access-requests` | `request_access_by_path` | `request <path>` |
| `decideAccessRequest` 1.0.0 | `POST /access-requests/{request_id}/decision` | `decide_access_request` | `decide` |
| `searchFiles` 1.1.0 | `GET /search` | `search` | `search` |
| `listNotifications` 1.0.0 | `GET /notifications` | `notifications` | `inbox` |
| `readNotifications` 1.0.0 | `POST /notifications/read` | `mark_notifications_read` | `inbox --read` |
| `listEntryActivity` 1.0.0 | `GET /entries/{entry_id}/activity` | `activity` | `history` |
| `listVersions` 1.0.0 | `GET /entries/{entry_id}/versions` | `versions` | `versions` |
| `restoreVersion` 1.1.0 | `POST /entries/{entry_id}/versions/{version_id}/restore` | `restore_version` | `restore` |
| `readOrganizationUsage` 1.0.0 | `GET /usage` | `usage` | `usage` |
| `listBin` 1.0.0 | `GET /bin` | `bin` | `bin list` |
| `restoreEntry` 1.0.0 | `POST /bin/{entry_id}/restore` | `restore_from_bin` | `bin restore` |
| `configureOrganizationBucket` 1.0.0 | `PUT /storage/configuration` | `configure_storage` | `storage configure` |

`health` and `ready` are additional public operational methods for host-root
`/healthz` and `/readyz`; signed `/webhook/` delivery is a backend receiver,
not a public member SDK mutation. IAM's privileged approval API is intentionally
absent from the Briefcase catalog. Client-only configuration, credential storage,
logout, update controls, and media-type helpers are not extra backend endpoints.
