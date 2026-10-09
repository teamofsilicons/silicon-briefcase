//! The scopes another app may hold to act for an account at Briefcase.
//!
//! An app acts for an account with a Silicon Accounts User verification proof
//! (`Authorization: Proof sap_…`). The proof's scopes are plain strings the
//! apps agree on; Briefcase honours exactly the scopes below, one per
//! delegated route, and each is allowed only for the issuing apps the operator
//! lists in `BRIEFCASE_PROOF_ISSUERS`.

/// Create a folder in the app's folder of the account's files.
pub const FOLDERS_CREATE: &str = "briefcase.folders.create";
/// List entries the account can see in the app's folder.
pub const ENTRIES_LIST: &str = "briefcase.entries.list";
/// Read a file the account can read in the app's folder.
pub const FILES_READ: &str = "briefcase.files.read";
/// Move an entry the app created to the bin.
pub const ENTRIES_TRASH: &str = "briefcase.entries.trash";
/// Reserve a delegated upload.
pub const UPLOADS_RESERVE: &str = "briefcase.uploads.reserve";
/// Commit a delegated upload.
pub const UPLOADS_COMMIT: &str = "briefcase.uploads.commit";
/// Read a delegated upload's status.
pub const UPLOADS_STATUS: &str = "briefcase.uploads.status";
/// Cancel a delegated upload.
pub const UPLOADS_CANCEL: &str = "briefcase.uploads.cancel";
/// Share an entry with another account.
pub const INVITATIONS_CREATE: &str = "briefcase.invitations.create";
/// Turn "anyone with the link can view" on or off.
pub const LINK_ACCESS_UPDATE: &str = "briefcase.link_access.update";

/// Every scope Briefcase honours, in catalogue order.
pub const DELEGATED_SCOPES: [&str; 10] = [
    FOLDERS_CREATE,
    ENTRIES_LIST,
    FILES_READ,
    ENTRIES_TRASH,
    UPLOADS_RESERVE,
    UPLOADS_COMMIT,
    UPLOADS_STATUS,
    UPLOADS_CANCEL,
    INVITATIONS_CREATE,
    LINK_ACCESS_UPDATE,
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::DELEGATED_SCOPES;

    #[test]
    fn scopes_are_unique_and_namespaced() {
        let unique = DELEGATED_SCOPES.iter().collect::<BTreeSet<_>>();
        assert_eq!(unique.len(), DELEGATED_SCOPES.len());
        assert!(
            DELEGATED_SCOPES
                .iter()
                .all(|scope| scope.starts_with("briefcase.") && scope.split('.').count() == 3)
        );
    }
}
