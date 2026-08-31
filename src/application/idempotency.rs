//! Idempotency-key validation and deterministic request fingerprints.

use std::fmt;

use serde::Serialize;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

/// A validated client idempotency key.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Validates a header value according to the public contract.
    ///
    /// # Errors
    ///
    /// Returns a classified error when the value violates length or character
    /// constraints.
    pub fn new(value: impl Into<String>) -> Result<Self, IdempotencyKeyError> {
        let value = value.into();
        if !(8..=255).contains(&value.len()) {
            return Err(IdempotencyKeyError::Length);
        }
        if value.chars().any(char::is_control) {
            return Err(IdempotencyKeyError::Characters);
        }
        Ok(Self(value))
    }

    /// Borrows the validated value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Invalid idempotency key.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum IdempotencyKeyError {
    /// Contract requires 8 through 255 bytes.
    #[error("idempotency key must be 8 to 255 bytes")]
    Length,
    /// Header cannot contain control characters.
    #[error("idempotency key contains a control character")]
    Characters,
}

/// Computes the canonical SHA-256 fingerprint for a JSON mutation.
///
/// The operation name is included to prevent accidental cross-route reuse of
/// an identical request object.
///
/// # Errors
///
/// Returns the serializer error if the request cannot be represented as JSON.
pub fn json_fingerprint<T: Serialize>(
    operation: &str,
    request: &T,
) -> Result<[u8; 32], serde_json::Error> {
    let encoded = serde_json::to_vec(request)?;
    Ok(bytes_fingerprint(operation, &encoded))
}

/// Computes a SHA-256 fingerprint over an operation and opaque canonical bytes.
#[must_use]
pub fn bytes_fingerprint(operation: &str, request: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(operation.as_bytes());
    digest.update([0]);
    digest.update(request);
    digest.finalize().into()
}

/// Combines already verified upload metadata and content hashes.
#[must_use]
pub fn upload_fingerprint(
    operation: &str,
    parent_id: &str,
    filename: &str,
    content_type: &str,
    size: u64,
    content_sha256: &[u8; 32],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    for value in [operation, parent_id, filename, content_type] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    digest.update(size.to_be_bytes());
    digest.update(content_sha256);
    digest.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::{IdempotencyKey, bytes_fingerprint, upload_fingerprint};

    #[test]
    fn keys_enforce_the_contract_length() {
        assert!(IdempotencyKey::new("short").is_err());
        assert!(IdempotencyKey::new("request-123").is_ok());
    }

    #[test]
    fn operation_name_separates_identical_payloads() {
        assert_ne!(
            bytes_fingerprint("create-folder", b"{}"),
            bytes_fingerprint("complete-upload", b"{}")
        );
    }

    #[test]
    fn upload_fingerprint_changes_with_content() {
        let first = upload_fingerprint("upload", "parent", "a.txt", "text/plain", 1, &[1; 32]);
        let second = upload_fingerprint("upload", "parent", "a.txt", "text/plain", 1, &[2; 32]);
        assert_ne!(first, second);
    }
}
