//! The wire contract this build serves, and how a client agrees to it.
//!
//! A client and a server disagree in two ways. They can disagree about the
//! whole surface — a client built for a later API talking to an older
//! deployment — and they can disagree about one operation, where a request or
//! response shape changed under a name that stayed the same. Both are settled
//! before the first real call: [`select`] negotiates the API major, and the
//! per-operation catalog below lets a client verify that every operation it
//! intends to call is the revision it was built against.
//!
//! An operation's version is bumped whenever its request or response shape
//! changes in a way a client can observe. Adding an operation leaves every
//! other version alone.

use http::HeaderName;
use serde::Serialize;

/// Request header carrying the API majors a client supports, newest first.
pub const SUPPORTED_VERSIONS_HEADER: HeaderName =
    HeaderName::from_static("briefcase-supported-api-versions");
/// Response header naming the API major this response was served under.
pub const SELECTED_VERSION_HEADER: HeaderName = HeaderName::from_static("briefcase-api-version");

/// API majors this build serves, newest first.
pub const SUPPORTED_API_VERSIONS: [&str; 1] = ["v1"];

/// Version of the published contract document, matching `openapi.yaml`.
pub const CONTRACT_VERSION: &str = "0.1.0";

/// Service identity a client checks before trusting anything else it reads.
pub const SERVICE_NAME: &str = "silicon-briefcase";

/// One contracted operation and the revision of its request and response.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct OperationVersion {
    /// `operationId` from the published contract.
    pub id: &'static str,
    /// Revision of this operation's request and response shape.
    pub version: &'static str,
    /// HTTP method.
    pub method: &'static str,
    /// Path below the versioned API base.
    pub path: &'static str,
}

/// Every contracted operation, with the revision this build serves.
pub const OPERATIONS: [OperationVersion; 27] = [
    operation("readApiVersion", "1.0.0", "GET", "/version"),
    operation("listEntries", "1.0.0", "GET", "/entries"),
    operation("createFolder", "1.0.0", "POST", "/entries"),
    operation("getEntry", "1.0.0", "GET", "/entries/{entry_id}"),
    operation("updateEntry", "1.0.0", "PATCH", "/entries/{entry_id}"),
    operation("moveEntryToBin", "1.0.0", "DELETE", "/entries/{entry_id}"),
    operation(
        "readEntryContent",
        "1.0.0",
        "GET",
        "/entries/{entry_id}/content",
    ),
    operation(
        "downloadEntry",
        "1.0.0",
        "GET",
        "/entries/{entry_id}/download",
    ),
    operation(
        "resolvePermanentUrl",
        "1.0.0",
        "GET",
        "/org/{org_id}/{path}",
    ),
    operation("uploadFile", "1.0.0", "POST", "/uploads"),
    operation("createFileOnBehalfOfMember", "1.0.0", "POST", "/obo/files"),
    operation(
        "listPermissions",
        "1.0.0",
        "GET",
        "/entries/{entry_id}/permissions",
    ),
    operation(
        "grantPermission",
        "1.0.0",
        "POST",
        "/entries/{entry_id}/permissions",
    ),
    operation(
        "revokePermission",
        "1.0.0",
        "DELETE",
        "/entries/{entry_id}/permissions/{grant_id}",
    ),
    operation(
        "inspectEffectivePermissions",
        "1.0.0",
        "POST",
        "/permissions/effective",
    ),
    operation(
        "requestAccess",
        "1.0.0",
        "POST",
        "/entries/{entry_id}/access-requests",
    ),
    operation(
        "decideAccessRequest",
        "1.0.0",
        "POST",
        "/access-requests/{request_id}/decision",
    ),
    operation("searchFiles", "1.0.0", "GET", "/search"),
    operation("listNotifications", "1.0.0", "GET", "/notifications"),
    operation("readNotifications", "1.0.0", "POST", "/notifications/read"),
    operation(
        "listEntryActivity",
        "1.0.0",
        "GET",
        "/entries/{entry_id}/activity",
    ),
    operation(
        "listVersions",
        "1.0.0",
        "GET",
        "/entries/{entry_id}/versions",
    ),
    operation(
        "restoreVersion",
        "1.0.0",
        "POST",
        "/entries/{entry_id}/versions/{version_id}/restore",
    ),
    operation("readOrganizationUsage", "1.0.0", "GET", "/usage"),
    operation("listBin", "1.0.0", "GET", "/bin"),
    operation("restoreEntry", "1.0.0", "POST", "/bin/{entry_id}/restore"),
    operation(
        "configureOrganizationBucket",
        "1.0.0",
        "PUT",
        "/storage/configuration",
    ),
];

const fn operation(
    id: &'static str,
    version: &'static str,
    method: &'static str,
    path: &'static str,
) -> OperationVersion {
    OperationVersion {
        id,
        version,
        method,
        path,
    }
}

/// Chooses the newest API major both sides support.
///
/// A client advertises its majors newest-first in
/// [`SUPPORTED_VERSIONS_HEADER`]. The server walks its own list in the same
/// order and takes the first the client also named, so the outcome depends on
/// neither side's ordering being trusted. A client that advertises nothing is
/// answered with the newest major, which is what a browser or a curl session
/// asking for the version document wants.
///
/// Returns `None` when the client named majors and none of them is served,
/// which is a hard incompatibility rather than a request to guess.
#[must_use]
pub fn select(advertised: Option<&str>) -> Option<&'static str> {
    let Some(advertised) = advertised else {
        return SUPPORTED_API_VERSIONS.first().copied();
    };
    let requested: Vec<&str> = advertised
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect();
    if requested.is_empty() {
        return SUPPORTED_API_VERSIONS.first().copied();
    }
    SUPPORTED_API_VERSIONS
        .into_iter()
        .find(|served| requested.contains(served))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{OPERATIONS, SUPPORTED_API_VERSIONS, select};

    #[test]
    fn every_operation_is_named_once_and_carries_a_revision() {
        let ids: BTreeSet<&str> = OPERATIONS.iter().map(|operation| operation.id).collect();
        assert_eq!(
            ids.len(),
            OPERATIONS.len(),
            "an operation id is listed twice"
        );
        for operation in OPERATIONS {
            assert_eq!(
                operation.version.split('.').count(),
                3,
                "{} must carry a three-part revision",
                operation.id
            );
            assert!(
                operation.path.starts_with('/'),
                "{} must name a path below the API base",
                operation.id
            );
        }
    }

    #[test]
    fn negotiation_takes_the_newest_shared_major_and_refuses_the_rest() {
        assert_eq!(select(None), Some("v1"));
        assert_eq!(select(Some("")), Some("v1"));
        assert_eq!(select(Some("v1")), Some("v1"));
        assert_eq!(select(Some("v2, v1")), Some("v1"));
        assert_eq!(select(Some(" v1 ")), Some("v1"));
        assert_eq!(select(Some("v2")), None);
        assert_eq!(select(Some("nonsense")), None);
    }

    #[test]
    fn the_served_majors_are_ordered_newest_first() {
        assert_eq!(SUPPORTED_API_VERSIONS, ["v1"]);
    }
}
