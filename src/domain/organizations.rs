//! Verified organization membership and group sharing recipients.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A group that the caller may select as a sharing recipient.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ShareTarget {
    /// `organization` or `team`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Organization handle, or `organization/team-uuid`.
    pub id: String,
    /// Current display name.
    pub name: String,
    /// Owning organization.
    pub org_id: String,
}

/// Authority fetched for this request, never a persistent membership cache.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct OrganizationContext {
    /// Exact account whose Briefcase token was verified by Organizations.
    pub account_uuid: String,
    /// Organizations visible to this work account, not linked personal accounts.
    pub organizations: Vec<Value>,
    /// Live membership keys used by the permission view.
    pub groups: Vec<String>,
    /// Current recipients the caller is allowed to discover.
    pub share_targets: Vec<ShareTarget>,
}
