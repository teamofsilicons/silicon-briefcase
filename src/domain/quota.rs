//! Organization upload and storage limits.
//!
//! Two bounds apply to every organization: how many bytes it may upload within
//! one UTC day, and how many bytes it may keep stored at once. The daily bound
//! counts uploads and returns at midnight; the storage bound counts what is
//! currently kept, so deleting a file returns capacity as soon as the bytes are
//! actually gone. Each organization may carry its own ceiling; where it does
//! not, the platform default applies.

use std::fmt;

use super::multipart::{GIB, TIB};

/// One binary pebibyte.
pub const PIB: u64 = 1_024 * TIB;

/// Bytes an organization may upload within one UTC day by default.
pub const DEFAULT_DAILY_UPLOAD_LIMIT_BYTES: u64 = 100 * GIB;
/// Bytes an organization may keep stored by default.
pub const DEFAULT_STORAGE_LIMIT_BYTES: u64 = PIB;

/// The limit an upload would exceed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UploadLimit {
    /// The daily upload allowance, which returns at midnight UTC.
    DailyUpload,
    /// The stored-bytes ceiling, which returns when content is deleted.
    Storage,
}

impl UploadLimit {
    /// Returns the stable machine-readable reason for this limit.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::DailyUpload => "daily_upload_limit_exhausted",
            Self::Storage => "storage_limit_exhausted",
        }
    }

    /// Reports whether merely waiting restores the capacity.
    #[must_use]
    pub const fn resets(self) -> bool {
        matches!(self, Self::DailyUpload)
    }
}

impl fmt::Display for UploadLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

/// What one organization currently consumes, and the ceilings it consumes against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrganizationUsage {
    /// Bytes uploaded so far within the current UTC day.
    pub daily_upload_bytes: u64,
    /// Bytes currently stored across every retained version.
    pub stored_bytes: u64,
    /// This organization's daily allowance, when it overrides the default.
    pub daily_upload_limit: Option<u64>,
    /// This organization's storage ceiling, when it overrides the default.
    pub storage_limit: Option<u64>,
}

impl Default for OrganizationUsage {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl OrganizationUsage {
    /// An organization that has never uploaded and carries no override.
    pub const EMPTY: Self = Self {
        daily_upload_bytes: 0,
        stored_bytes: 0,
        daily_upload_limit: None,
        storage_limit: None,
    };

    /// Returns the daily allowance in force.
    #[must_use]
    pub const fn daily_upload_allowance(&self) -> u64 {
        match self.daily_upload_limit {
            Some(limit) => limit,
            None => DEFAULT_DAILY_UPLOAD_LIMIT_BYTES,
        }
    }

    /// Returns the storage ceiling in force.
    #[must_use]
    pub const fn storage_allowance(&self) -> u64 {
        match self.storage_limit {
            Some(limit) => limit,
            None => DEFAULT_STORAGE_LIMIT_BYTES,
        }
    }

    /// Returns the bytes still available today.
    #[must_use]
    pub const fn daily_upload_remaining(&self) -> u64 {
        self.daily_upload_allowance()
            .saturating_sub(self.daily_upload_bytes)
    }

    /// Returns the bytes still available to store.
    #[must_use]
    pub const fn storage_remaining(&self) -> u64 {
        self.storage_allowance().saturating_sub(self.stored_bytes)
    }

    /// Decides whether one more upload of `bytes` fits within both bounds.
    ///
    /// The daily allowance is reported first: it is the one a caller can wait
    /// out, so naming it is more useful than naming the ceiling it also meets.
    ///
    /// # Errors
    ///
    /// Returns the [`UploadLimit`] the upload would exceed.
    pub const fn admits_upload(&self, bytes: u64) -> Result<(), UploadLimit> {
        if self.daily_upload_bytes.saturating_add(bytes) > self.daily_upload_allowance() {
            return Err(UploadLimit::DailyUpload);
        }
        self.admits_storage(bytes)
    }

    /// Decides whether `bytes` more may be stored, ignoring the daily allowance.
    ///
    /// Restoring a historical version consumes storage without uploading
    /// anything, so it answers to this bound alone.
    ///
    /// # Errors
    ///
    /// Returns [`UploadLimit::Storage`] when the organization has no room.
    pub const fn admits_storage(&self, bytes: u64) -> Result<(), UploadLimit> {
        if self.stored_bytes.saturating_add(bytes) > self.storage_allowance() {
            return Err(UploadLimit::Storage);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_DAILY_UPLOAD_LIMIT_BYTES, DEFAULT_STORAGE_LIMIT_BYTES, OrganizationUsage,
        UploadLimit,
    };
    use crate::domain::multipart::{GIB, MIB};

    #[test]
    fn an_unused_organization_admits_an_ordinary_upload() {
        assert_eq!(OrganizationUsage::EMPTY.admits_upload(64 * MIB), Ok(()));
    }

    #[test]
    fn the_daily_allowance_is_exact_at_its_boundary() {
        let usage = OrganizationUsage {
            daily_upload_bytes: DEFAULT_DAILY_UPLOAD_LIMIT_BYTES - MIB,
            ..OrganizationUsage::EMPTY
        };
        assert_eq!(usage.admits_upload(MIB), Ok(()));
        assert_eq!(usage.admits_upload(MIB + 1), Err(UploadLimit::DailyUpload));
        assert_eq!(usage.daily_upload_remaining(), MIB);
    }

    #[test]
    fn a_full_organization_reports_the_storage_ceiling() {
        let usage = OrganizationUsage {
            stored_bytes: DEFAULT_STORAGE_LIMIT_BYTES,
            ..OrganizationUsage::EMPTY
        };
        assert_eq!(usage.admits_upload(1), Err(UploadLimit::Storage));
        assert_eq!(usage.admits_storage(1), Err(UploadLimit::Storage));
        assert_eq!(usage.storage_remaining(), 0);
    }

    #[test]
    fn a_configured_organization_uses_its_own_ceilings() {
        let usage = OrganizationUsage {
            daily_upload_bytes: 200 * GIB,
            stored_bytes: 0,
            daily_upload_limit: Some(500 * GIB),
            storage_limit: Some(GIB),
        };
        assert_eq!(usage.admits_upload(GIB), Ok(()));
        assert_eq!(usage.admits_upload(GIB + 1), Err(UploadLimit::Storage));
        assert_eq!(usage.daily_upload_allowance(), 500 * GIB);
        assert_eq!(usage.storage_allowance(), GIB);
    }

    #[test]
    fn the_waitable_limit_is_named_before_the_permanent_one() {
        let usage = OrganizationUsage {
            daily_upload_bytes: DEFAULT_DAILY_UPLOAD_LIMIT_BYTES,
            stored_bytes: DEFAULT_STORAGE_LIMIT_BYTES,
            ..OrganizationUsage::EMPTY
        };
        assert_eq!(usage.admits_upload(1), Err(UploadLimit::DailyUpload));
        assert!(UploadLimit::DailyUpload.resets());
        assert!(!UploadLimit::Storage.resets());
    }

    #[test]
    fn an_absurd_request_cannot_wrap_around_the_counters() {
        let usage = OrganizationUsage {
            daily_upload_bytes: u64::MAX,
            stored_bytes: u64::MAX,
            ..OrganizationUsage::EMPTY
        };
        assert_eq!(usage.admits_upload(u64::MAX), Err(UploadLimit::DailyUpload));
        assert_eq!(usage.admits_storage(u64::MAX), Err(UploadLimit::Storage));
    }
}
