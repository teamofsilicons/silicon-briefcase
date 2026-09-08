# Migrating to client and CLI 0.2

This guide describes the client/CLI 0.2 release and API contract 0.5. Use them
together. The API remains under `/api/v1`; individual operation revisions are
reported by the anonymous `/api/version` endpoint.

## Top-level folders

The `createFolder` operation is now revision **2.0.0**. A folder created without
a parent stays at the organisation base, with `parent_id: null`. Its required
Public, Private, or tag type defines its access boundary, not a destination
container. Nested folder creation is unchanged.

| Intent | CLI | Rust client |
| --- | --- | --- |
| Private folder at `/notes` | `briefcase mkdir notes --type private` | `NewFolder::at_base("notes", RootType::Private)` |
| Folder inside your existing private directory | `briefcase mkdir private/YOUR_ID/notes` | `NewFolder::in_folder("notes", Destination::path("private/YOUR_ID"))` |
| Folder inside Public | `briefcase mkdir public/handbook` | `NewFolder::in_folder("handbook", Destination::path("public"))` |
| Top-level folder governed by a tag | `briefcase mkdir specs --type tag --tag engineering` | `NewFolder::in_tag("specs", "engineering")` |

Replace `YOUR_ID` and the sample tag with your actual IAM values. Creating a tag
root requires current authority for that tag space. Names at the same level
must be unique, including names reserved for the standard containers.

No existing folder is moved. Existing IDs, content, grants, and paths remain
unchanged. Update scripts that assumed a container-prefixed result from
`at_base`, `in_tag`, or a single-segment `mkdir`: either supply the intended
parent explicitly or use the returned entry's actual `path` and `permanent_url`.

The default connection check rejects mismatched `createFolder` revisions before
performing file operations. Upgrade the client and deployment together; do not
use `--no-verify` or `Client::new_unchecked` to hide this mismatch.

## Rust dependency and error handling

Change the dependency requirement to `briefcase-client = "0.2"` and rebuild.
A `0.1` requirement does not opt into this compatibility-breaking release.
The stateless SDK does not replace a running executable.

`ApiError` now includes `unsatisfied_range_length: Option<u64>`. It preserves the
authorised resource length from a valid `Content-Range: bytes */N` response to
an unsatisfiable range request. It is `None` when that header is absent or
invalid, or the response is not HTTP 416.

If your code constructs an `ApiError` with a struct literal, add
`unsatisfied_range_length: None` unless it actually has the response metadata.
When destructuring only selected fields, include `..`. Code matching errors
through `Error::code`, `is_not_found`, or other existing accessors needs no
change for this field.

## Deployment compatibility

Check `/api/version` for `createFolder` revision `2.0.0` before directing 0.2
clients to a deployment. Rebuild the browser gateway with the matching SDK;
updating frontend assets alone does not update its server-side client.

Plan the backend, gateway, and CLI rollout as one compatibility change. During
a mixed-version rollout, the connection check intentionally refuses mismatched
pairs. If rolling back, restore the matching backend and clients together;
rollback does not relocate folders created at the top level.
