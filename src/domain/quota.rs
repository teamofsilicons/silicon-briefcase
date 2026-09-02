//! Organization upload limits.
//!
//! Two limits bound what an organization may upload: a daily allowance that
//! resets at midnight UTC, and a total that never resets. Both count uploaded
//! bytes rather than stored bytes, so deleting a file frees storage but not
//! allowance — an upload is spent once it happens.

use std::fmt;

use super::multipart::{GIB, TIB};

/// Bytes one organization may upload within a single UTC day.
pub const DAILY_UPLOAD_LIMIT_BYTES: u64 = 100 * GIB;
/// Bytes one organization may upload in total.
pub const ORGANIZATION_UPLOAD_LIMIT_BYTES: u64 = 100 * TIB;

/// The limit an upload would exceed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UploadLimit {
    /// The daily allowance, which resets at midnight UTC.
    Daily,
    /// The organization's total allowance, which never resets.
    Organization,
}

impl UploadLimit {
    /// Returns the stable machine-readable reason for this limit.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Daily => "daily_upload_limit_exhausted",
            Self::Organization => "organization_upload_limit_exhausted",
        }
    }

    /// Returns the byte allowance this limit enforces.
    #[must_use]
    pub const fn allowance(self) -> u64 {
        match self {
            Self::Daily => DAILY_UPLOAD_LIMIT_BYTES,
            Self::Organization => ORGANIZATION_UPLOAD_LIMIT_BYTES,
        }
    }

    /// Reports whether waiting restores the allowance.
    #[must_use]
    pub const fn resets(self) -> bool {
        matches!(self, Self::Daily)
    }
}

impl fmt::Display for UploadLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

/// Bytes already charged to one organization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UploadUsage {
    /// Bytes uploaded within the current UTC day.
    pub daily_bytes: u64,
    /// Bytes uploaded since the organization existed.
    pub total_bytes: u64,
}

impl UploadUsage {
    /// Decides whether one more upload of `bytes` fits within both limits.
    ///
    /// The daily limit is reported first: it is the one a caller can wait out,
    /// so naming it is more useful than naming the total it also exceeds.
    ///
    /// # Errors
    ///
    /// Returns the [`UploadLimit`] the upload would exceed.
    pub const fn admits(self, bytes: u64) -> Result<(), UploadLimit> {
        if self.daily_bytes.saturating_add(bytes) > DAILY_UPLOAD_LIMIT_BYTES {
            return Err(UploadLimit::Daily);
        }
        if self.total_bytes.saturating_add(bytes) > ORGANIZATION_UPLOAD_LIMIT_BYTES {
            return Err(UploadLimit::Organization);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DAILY_UPLOAD_LIMIT_BYTES, ORGANIZATION_UPLOAD_LIMIT_BYTES, UploadLimit, UploadUsage,
    };
    use crate::domain::multipart::MIB;

    #[test]
    fn an_unused_organization_admits_an_ordinary_upload() {
        assert_eq!(UploadUsage::default().admits(64 * MIB), Ok(()));
    }

    #[test]
    fn the_daily_allowance_is_exact_at_its_boundary() {
        let usage = UploadUsage {
            daily_bytes: DAILY_UPLOAD_LIMIT_BYTES - MIB,
            total_bytes: 0,
        };
        assert_eq!(usage.admits(MIB), Ok(()));
        assert_eq!(usage.admits(MIB + 1), Err(UploadLimit::Daily));
    }

    #[test]
    fn a_spent_total_is_reported_even_when_the_day_is_empty() {
        let usage = UploadUsage {
            daily_bytes: 0,
            total_bytes: ORGANIZATION_UPLOAD_LIMIT_BYTES,
        };
        assert_eq!(usage.admits(1), Err(UploadLimit::Organization));
    }

    #[test]
    fn the_waitable_limit_is_named_before_the_permanent_one() {
        let usage = UploadUsage {
            daily_bytes: DAILY_UPLOAD_LIMIT_BYTES,
            total_bytes: ORGANIZATION_UPLOAD_LIMIT_BYTES,
        };
        assert_eq!(usage.admits(1), Err(UploadLimit::Daily));
        assert!(UploadLimit::Daily.resets());
        assert!(!UploadLimit::Organization.resets());
    }

    #[test]
    fn an_absurd_request_cannot_wrap_around_the_counter() {
        let usage = UploadUsage {
            daily_bytes: u64::MAX,
            total_bytes: u64::MAX,
        };
        assert_eq!(usage.admits(u64::MAX), Err(UploadLimit::Daily));
    }
}
