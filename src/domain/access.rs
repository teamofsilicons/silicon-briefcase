//! Access-request decision state.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::permission::GrantedAccess;

/// Durable state of a request for explicit access.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessRequestStatus {
    /// Waiting for an owner or authorized administrator.
    Pending,
    /// Approved and paired with an explicit grant.
    Approved,
    /// Denied without creating a grant.
    Denied,
}

impl AccessRequestStatus {
    /// Applies a single terminal decision to a pending request.
    ///
    /// # Errors
    ///
    /// Returns [`AccessRequestTransitionError`] when this request is no longer
    /// pending.
    pub const fn decide(
        self,
        decision: AccessDecision,
    ) -> Result<Self, AccessRequestTransitionError> {
        if !matches!(self, Self::Pending) {
            return Err(AccessRequestTransitionError::AlreadyDecided { status: self });
        }
        match decision {
            AccessDecision::Approve { .. } => Ok(Self::Approved),
            AccessDecision::Deny => Ok(Self::Denied),
        }
    }

    /// Returns whether a decision has already been recorded.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Approved | Self::Denied)
    }
}

/// An owner or administrator's decision on an access request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AccessDecision {
    /// Approve and create a grant conveying the selected rights.
    Approve {
        /// Rights to grant; they may be no broader than policy permits.
        access: GrantedAccess,
    },
    /// Deny without creating a grant.
    Deny,
}

/// An invalid access-request state transition.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AccessRequestTransitionError {
    /// A terminal request cannot be decided again.
    #[error("access request already has terminal status {status:?}")]
    AlreadyDecided {
        /// Existing terminal status.
        status: AccessRequestStatus,
    },
}

#[cfg(test)]
mod tests {
    use super::{AccessDecision, AccessRequestStatus};
    use crate::domain::permission::GrantedAccess;

    #[test]
    fn pending_requests_receive_one_terminal_decision() {
        let approved = AccessRequestStatus::Pending.decide(AccessDecision::Approve {
            access: GrantedAccess::READ_ONLY,
        });
        assert_eq!(approved, Ok(AccessRequestStatus::Approved));
        assert!(
            AccessRequestStatus::Approved
                .decide(AccessDecision::Deny)
                .is_err()
        );
    }
}
