//! Upload limits, multipart sizing, completion validation, and state policy.
//!
//! Part sizing follows the written formula in the product contract: divide the
//! declared size by a thousand parts, round up to a whole mebibyte, clamp to
//! 8 MiB and 5 GiB, then take as many parts as that size needs. Callers never
//! see any of it: one upload endpoint takes the whole file and this module
//! decides whether the bytes travel as a single request or as parts.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One binary mebibyte.
pub const MIB: u64 = 1_048_576;
/// One binary gibibyte.
pub const GIB: u64 = 1_073_741_824;
/// One binary tebibyte.
pub const TIB: u64 = 1_099_511_627_776;
/// Largest file accepted by the single-request upload route.
pub const SINGLE_UPLOAD_MAX_BYTES: u64 = 100 * MIB;
/// Smallest declared size accepted by multipart initialization.
pub const MULTIPART_MIN_FILE_BYTES: u64 = SINGLE_UPLOAD_MAX_BYTES + 1;
/// Desired number of parts before applying S3 part-size limits.
pub const MULTIPART_TARGET_PART_COUNT: u64 = 1_000;
/// Minimum multipart part size, except for the final part.
pub const MULTIPART_MIN_PART_BYTES: u64 = 8 * MIB;
/// Maximum S3 multipart part size.
pub const MULTIPART_MAX_PART_BYTES: u64 = 5 * GIB;
/// Maximum accepted object size.
pub const MAX_UPLOAD_BYTES: u64 = 5 * TIB;
/// Maximum S3 multipart part count.
pub const MULTIPART_MAX_PART_COUNT: u32 = 10_000;
/// Multipart sessions expire after 24 hours.
pub const MULTIPART_SESSION_TTL_SECONDS: i64 = 24 * 60 * 60;

/// Upload route selected from a declared byte size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UploadStrategy {
    /// Stream the body through the single-request route.
    SingleRequest,
    /// Use the calculated multipart plan.
    Multipart(MultipartPlan),
}

impl UploadStrategy {
    /// Selects the required route and multipart plan for a declared file size.
    ///
    /// # Errors
    ///
    /// Returns [`MultipartPlanError`] when a multipart file is too large or a
    /// valid plan cannot be represented.
    pub fn for_file_size(file_size: u64) -> Result<Self, MultipartPlanError> {
        if file_size <= SINGLE_UPLOAD_MAX_BYTES {
            Ok(Self::SingleRequest)
        } else {
            MultipartPlan::for_file_size(file_size).map(Self::Multipart)
        }
    }
}

/// Canonical multipart sizing calculated from the declared file size.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MultipartPlan {
    file_size: u64,
    part_size: u64,
    part_count: u32,
}

impl MultipartPlan {
    /// Calculates the written product formula with checked integer arithmetic.
    ///
    /// # Errors
    ///
    /// Returns [`MultipartPlanError`] when multipart is unnecessary, the file
    /// exceeds the service limit, or the plan cannot be represented within the
    /// provider limits.
    pub fn for_file_size(file_size: u64) -> Result<Self, MultipartPlanError> {
        if file_size < MULTIPART_MIN_FILE_BYTES {
            return Err(MultipartPlanError::MultipartNotRequired {
                file_size,
                threshold: SINGLE_UPLOAD_MAX_BYTES,
            });
        }
        if file_size > MAX_UPLOAD_BYTES {
            return Err(MultipartPlanError::FileTooLarge {
                file_size,
                maximum: MAX_UPLOAD_BYTES,
            });
        }

        let calculated_part_size = checked_ceil_div(file_size, MULTIPART_TARGET_PART_COUNT)?;
        let whole_mib = checked_ceil_div(calculated_part_size, MIB)?;
        let rounded_part_size = whole_mib
            .checked_mul(MIB)
            .ok_or(MultipartPlanError::ArithmeticOverflow)?;
        let part_size = rounded_part_size.clamp(MULTIPART_MIN_PART_BYTES, MULTIPART_MAX_PART_BYTES);
        let part_count_u64 = checked_ceil_div(file_size, part_size)?;
        let part_count =
            u32::try_from(part_count_u64).map_err(|_| MultipartPlanError::ArithmeticOverflow)?;
        if part_count > MULTIPART_MAX_PART_COUNT {
            return Err(MultipartPlanError::TooManyParts {
                part_count,
                maximum: MULTIPART_MAX_PART_COUNT,
            });
        }

        Ok(Self {
            file_size,
            part_size,
            part_count,
        })
    }

    /// Returns the declared complete file size.
    #[must_use]
    pub const fn file_size(self) -> u64 {
        self.file_size
    }

    /// Returns the canonical non-final part size.
    #[must_use]
    pub const fn part_size(self) -> u64 {
        self.part_size
    }

    /// Returns the exact expected part count.
    #[must_use]
    pub const fn part_count(self) -> u32 {
        self.part_count
    }

    /// Returns the exact expected byte size for a numbered part.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidPartNumber`] when `part_number` is outside the plan's
    /// one-based part range or the stored plan cannot be evaluated safely.
    pub fn expected_part_size(self, part_number: u32) -> Result<u64, InvalidPartNumber> {
        if part_number == 0 || part_number > self.part_count {
            return Err(InvalidPartNumber {
                part_number,
                part_count: self.part_count,
            });
        }
        if part_number < self.part_count {
            return Ok(self.part_size);
        }

        let preceding_part_count = u64::from(self.part_count - 1);
        let preceding_bytes =
            preceding_part_count
                .checked_mul(self.part_size)
                .ok_or(InvalidPartNumber {
                    part_number,
                    part_count: self.part_count,
                })?;
        self.file_size
            .checked_sub(preceding_bytes)
            .ok_or(InvalidPartNumber {
                part_number,
                part_count: self.part_count,
            })
    }
}

fn checked_ceil_div(dividend: u64, divisor: u64) -> Result<u64, MultipartPlanError> {
    let quotient = dividend
        .checked_div(divisor)
        .ok_or(MultipartPlanError::ArithmeticOverflow)?;
    let remainder = dividend
        .checked_rem(divisor)
        .ok_or(MultipartPlanError::ArithmeticOverflow)?;
    quotient
        .checked_add(u64::from(remainder != 0))
        .ok_or(MultipartPlanError::ArithmeticOverflow)
}

/// Failure to calculate a valid multipart plan.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum MultipartPlanError {
    /// The single-request route must be used at or below the threshold.
    #[error("multipart is not required for {file_size} bytes; threshold is {threshold}")]
    MultipartNotRequired {
        /// Declared file size.
        file_size: u64,
        /// Inclusive single-request threshold.
        threshold: u64,
    },
    /// The declared size exceeds the 5 TiB service limit.
    #[error("file size {file_size} exceeds maximum {maximum}")]
    FileTooLarge {
        /// Declared file size.
        file_size: u64,
        /// Inclusive service maximum.
        maximum: u64,
    },
    /// A checked arithmetic operation could not be represented.
    #[error("multipart size calculation overflowed")]
    ArithmeticOverflow,
    /// A calculated plan would exceed S3's part-count limit.
    #[error("calculated {part_count} parts; maximum is {maximum}")]
    TooManyParts {
        /// Calculated number of parts.
        part_count: u32,
        /// S3 maximum number of parts.
        maximum: u32,
    },
}

/// A part number outside a multipart plan's exact range.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("part number {part_number} is outside 1..={part_count}")]
pub struct InvalidPartNumber {
    /// Supplied one-based part number.
    pub part_number: u32,
    /// Exact expected part count.
    pub part_count: u32,
}

/// Validated metadata for one uploaded S3 part.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletedPart {
    part_number: u32,
    etag: String,
    byte_size: u64,
    checksum_sha256: [u8; 32],
}

impl CompletedPart {
    /// Constructs part metadata while preserving the provider's exact `ETag`.
    ///
    /// # Errors
    ///
    /// Returns [`CompletedPartError`] when the part number is zero or the
    /// provider `ETag` is empty.
    pub fn new(
        part_number: u32,
        etag: impl Into<String>,
        byte_size: u64,
        checksum_sha256: [u8; 32],
    ) -> Result<Self, CompletedPartError> {
        if part_number == 0 {
            return Err(CompletedPartError::ZeroPartNumber);
        }
        let etag = etag.into();
        if etag.trim().is_empty() {
            return Err(CompletedPartError::EmptyEtag);
        }
        Ok(Self {
            part_number,
            etag,
            byte_size,
            checksum_sha256,
        })
    }

    /// Returns the one-based part number.
    #[must_use]
    pub const fn part_number(&self) -> u32 {
        self.part_number
    }

    /// Returns the provider's exact `ETag`.
    #[must_use]
    pub fn etag(&self) -> &str {
        &self.etag
    }

    /// Returns the bytes received for this part.
    #[must_use]
    pub const fn byte_size(&self) -> u64 {
        self.byte_size
    }

    /// Returns the part SHA-256 that the provider verified during upload.
    #[must_use]
    pub const fn checksum_sha256(&self) -> &[u8; 32] {
        &self.checksum_sha256
    }
}

/// Invalid stored metadata for an uploaded part.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CompletedPartError {
    /// Multipart part numbers are one-based.
    #[error("part number must start at one")]
    ZeroPartNumber,
    /// Completion requires a non-empty provider `ETag`.
    #[error("part ETag cannot be empty")]
    EmptyEtag,
}

/// Verifies an exact, ordered multipart completion set and byte total.
///
/// # Errors
///
/// Returns [`CompletionValidationError`] when part count, order, sizes, or byte
/// total do not exactly match `plan`, or when checked arithmetic fails.
pub fn validate_completion(
    plan: MultipartPlan,
    parts: &[CompletedPart],
) -> Result<(), CompletionValidationError> {
    let actual_count = u32::try_from(parts.len())
        .map_err(|_| CompletionValidationError::PartCountNotRepresentable)?;
    if actual_count != plan.part_count() {
        return Err(CompletionValidationError::WrongPartCount {
            expected: plan.part_count(),
            actual: actual_count,
        });
    }

    let mut total_bytes = 0_u64;
    for (index, part) in parts.iter().enumerate() {
        let expected_number = u32::try_from(index)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or(CompletionValidationError::PartCountNotRepresentable)?;
        if part.part_number() != expected_number {
            return Err(CompletionValidationError::UnexpectedPartNumber {
                position: expected_number,
                actual: part.part_number(),
            });
        }
        let expected_size = plan
            .expected_part_size(expected_number)
            .map_err(|_| CompletionValidationError::PartCountNotRepresentable)?;
        if part.byte_size() != expected_size {
            return Err(CompletionValidationError::WrongPartSize {
                part_number: part.part_number(),
                expected: expected_size,
                actual: part.byte_size(),
            });
        }
        total_bytes = total_bytes
            .checked_add(part.byte_size())
            .ok_or(CompletionValidationError::ByteTotalOverflow)?;
    }

    if total_bytes != plan.file_size() {
        return Err(CompletionValidationError::WrongByteTotal {
            expected: plan.file_size(),
            actual: total_bytes,
        });
    }
    Ok(())
}

/// Why a multipart completion set cannot be published.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CompletionValidationError {
    /// The in-memory part count cannot be represented by the contract type.
    #[error("part count cannot be represented")]
    PartCountNotRepresentable,
    /// Completion did not include the plan's exact number of parts.
    #[error("expected {expected} parts, received {actual}")]
    WrongPartCount {
        /// Plan part count.
        expected: u32,
        /// Supplied part count.
        actual: u32,
    },
    /// Parts are missing, duplicated, or out of order.
    #[error("part at position {position} has number {actual}")]
    UnexpectedPartNumber {
        /// Required one-based number at this position.
        position: u32,
        /// Supplied part number.
        actual: u32,
    },
    /// A part's stored byte count differs from the canonical plan.
    #[error("part {part_number} has {actual} bytes; expected {expected}")]
    WrongPartSize {
        /// One-based part number.
        part_number: u32,
        /// Canonical part byte count.
        expected: u64,
        /// Stored part byte count.
        actual: u64,
    },
    /// Summing stored part byte counts overflowed.
    #[error("part byte total overflowed")]
    ByteTotalOverflow,
    /// The exact part set does not reconstruct the declared file size.
    #[error("parts total {actual} bytes; expected {expected}")]
    WrongByteTotal {
        /// Declared file size.
        expected: u64,
        /// Summed part bytes.
        actual: u64,
    },
}

/// Durable multipart-upload lifecycle state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MultipartUploadState {
    /// Storage initialization has not completed.
    Initializing,
    /// The session accepts numbered parts.
    Active,
    /// Storage is assembling the verified parts.
    Completing,
    /// The entry and initial version have been published.
    Completed,
    /// Storage abortion is in progress.
    Aborting,
    /// Uploaded parts were removed without publishing an entry.
    Aborted,
    /// A non-retryable lifecycle operation failed.
    Failed,
    /// The 24-hour active-session deadline elapsed.
    Expired,
}

impl MultipartUploadState {
    /// Applies a valid forward-only lifecycle transition.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMultipartStateTransition`] when `next` is not a valid
    /// forward transition from the current state.
    pub const fn transition(self, next: Self) -> Result<Self, InvalidMultipartStateTransition> {
        let valid = matches!(
            (self, next),
            (Self::Initializing, Self::Active | Self::Failed)
                | (
                    Self::Active,
                    Self::Completing | Self::Aborting | Self::Expired | Self::Failed
                )
                | (Self::Completing, Self::Completed | Self::Failed)
                | (Self::Aborting, Self::Aborted | Self::Failed)
        );
        if valid {
            Ok(next)
        } else {
            Err(InvalidMultipartStateTransition {
                current: self,
                next,
            })
        }
    }

    /// Returns whether no further domain transition is permitted.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Aborted | Self::Failed | Self::Expired
        )
    }
}

/// An invalid multipart lifecycle transition.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("cannot transition multipart upload from {current:?} to {next:?}")]
pub struct InvalidMultipartStateTransition {
    /// Current durable state.
    pub current: MultipartUploadState,
    /// Requested next state.
    pub next: MultipartUploadState,
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{
        CompletedPart, CompletionValidationError, GIB, MAX_UPLOAD_BYTES, MIB,
        MULTIPART_MAX_PART_BYTES, MULTIPART_MIN_FILE_BYTES, MULTIPART_MIN_PART_BYTES,
        MultipartPlan, MultipartUploadState, SINGLE_UPLOAD_MAX_BYTES, TIB, UploadStrategy,
        validate_completion,
    };

    fn plan(file_size: u64) -> MultipartPlan {
        match MultipartPlan::for_file_size(file_size) {
            Ok(plan) => plan,
            Err(error) => panic!("test size must produce a plan: {error}"),
        }
    }

    fn part(number: u32, size: u64) -> CompletedPart {
        match CompletedPart::new(number, format!("etag-{number}"), size, [7; 32]) {
            Ok(part) => part,
            Err(error) => panic!("test part must be valid: {error}"),
        }
    }

    #[test]
    fn route_threshold_is_inclusive_for_single_upload() {
        assert_eq!(
            UploadStrategy::for_file_size(SINGLE_UPLOAD_MAX_BYTES),
            Ok(UploadStrategy::SingleRequest)
        );
        assert!(matches!(
            UploadStrategy::for_file_size(MULTIPART_MIN_FILE_BYTES),
            Ok(UploadStrategy::Multipart(_))
        ));
    }

    #[test]
    fn canonical_examples_follow_the_written_formula() {
        let two_hundred_mib = plan(200 * MIB);
        assert_eq!(two_hundred_mib.part_size(), 8 * MIB);
        assert_eq!(two_hundred_mib.part_count(), 25);

        let ten_gib = plan(10 * GIB);
        assert_eq!(ten_gib.part_size(), 11 * MIB);
        assert_eq!(ten_gib.part_count(), 931);

        let one_tib = plan(TIB);
        assert_eq!(one_tib.part_size(), 1_049 * MIB);
        assert_eq!(one_tib.part_count(), 1_000);

        let five_tib = plan(5 * TIB);
        assert_eq!(five_tib.part_size(), 5 * GIB);
        assert_eq!(five_tib.part_count(), 1_024);
    }

    #[test]
    fn completion_requires_exact_order_sizes_and_total() {
        let plan = plan(200 * MIB);
        let parts: Vec<_> = (1..=plan.part_count())
            .map(|number| part(number, 8 * MIB))
            .collect();
        assert_eq!(validate_completion(plan, &parts), Ok(()));

        let mut wrong_order = parts.clone();
        wrong_order.swap(0, 1);
        assert!(matches!(
            validate_completion(plan, &wrong_order),
            Err(CompletionValidationError::UnexpectedPartNumber { .. })
        ));
    }

    #[test]
    fn state_machine_is_forward_only() {
        assert_eq!(
            MultipartUploadState::Initializing.transition(MultipartUploadState::Active),
            Ok(MultipartUploadState::Active)
        );
        assert_eq!(
            MultipartUploadState::Active.transition(MultipartUploadState::Completing),
            Ok(MultipartUploadState::Completing)
        );
        assert!(
            MultipartUploadState::Completed
                .transition(MultipartUploadState::Active)
                .is_err()
        );
        assert!(MultipartUploadState::Completed.is_terminal());
        assert!(!MultipartUploadState::Active.is_terminal());
    }

    proptest! {
        #[test]
        fn valid_sizes_always_produce_bounded_exact_plans(
            file_size in MULTIPART_MIN_FILE_BYTES..=MAX_UPLOAD_BYTES
        ) {
            if let Ok(plan) = MultipartPlan::for_file_size(file_size) {
                prop_assert!(plan.part_size() >= MULTIPART_MIN_PART_BYTES);
                prop_assert!(plan.part_size() <= MULTIPART_MAX_PART_BYTES);
                prop_assert!(plan.part_count() > 0);
                let preceding = u64::from(plan.part_count() - 1)
                    .checked_mul(plan.part_size());
                let last = plan.expected_part_size(plan.part_count());
                if let (Some(preceding), Ok(last)) = (preceding, last) {
                    prop_assert_eq!(preceding.checked_add(last), Some(file_size));
                } else {
                    prop_assert!(false, "valid plan arithmetic must remain representable");
                }
            } else {
                prop_assert!(false, "in-range size must produce a multipart plan");
            }
        }
    }
}
