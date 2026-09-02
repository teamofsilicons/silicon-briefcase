//! Interfaces implemented by infrastructure adapters.

use std::{fmt, path::Path};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::Bytes;
use futures::stream::BoxStream;
use thiserror::Error;

use crate::domain::storage::EncryptionMode;

/// Algorithm attached to a provider-verified object checksum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectChecksumAlgorithm {
    /// SHA-256 encoded by the provider as Base64.
    Sha256,
}

/// Whether an object checksum covers bytes directly or combines part digests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectChecksumType {
    /// Digest of the complete object byte stream.
    FullObject,
    /// Digest composed from an exact ordered multipart set.
    Composite,
}

/// Provider-verified integrity metadata for an object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectChecksum {
    algorithm: ObjectChecksumAlgorithm,
    checksum_type: ObjectChecksumType,
    encoded_value: String,
}

impl ObjectChecksum {
    /// Creates a validated checksum descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`ObjectChecksumError`] when the encoded provider value is
    /// empty, unreasonably large, or contains unsafe characters.
    pub fn new(
        algorithm: ObjectChecksumAlgorithm,
        checksum_type: ObjectChecksumType,
        encoded_value: impl Into<String>,
    ) -> Result<Self, ObjectChecksumError> {
        let encoded_value = encoded_value.into();
        let valid = match checksum_type {
            ObjectChecksumType::FullObject => valid_sha256_base64(&encoded_value),
            ObjectChecksumType::Composite => encoded_value
                .rsplit_once('-')
                .and_then(|(digest, part_count)| {
                    part_count
                        .parse::<u32>()
                        .ok()
                        .filter(|count| (1..=10_000).contains(count))
                        .map(|_| digest)
                })
                .is_some_and(valid_sha256_base64),
        };
        if !valid {
            return Err(ObjectChecksumError);
        }
        Ok(Self {
            algorithm,
            checksum_type,
            encoded_value,
        })
    }

    /// Returns the checksum algorithm.
    #[must_use]
    pub const fn algorithm(&self) -> ObjectChecksumAlgorithm {
        self.algorithm
    }

    /// Returns whether this checksum is full-object or composite.
    #[must_use]
    pub const fn checksum_type(&self) -> ObjectChecksumType {
        self.checksum_type
    }

    /// Returns the provider's encoded checksum value.
    #[must_use]
    pub fn encoded_value(&self) -> &str {
        &self.encoded_value
    }
}

fn valid_sha256_base64(value: &str) -> bool {
    STANDARD
        .decode(value)
        .is_ok_and(|decoded| decoded.len() == 32)
}

/// Invalid provider checksum metadata.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("object checksum metadata is invalid")]
pub struct ObjectChecksumError;

/// Fully resolved object-storage destination for one operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageTarget {
    /// S3 bucket.
    pub bucket: String,
    /// AWS region used for both signing and service routing.
    pub region: String,
    /// Tenant-owned prefix without a leading slash.
    pub prefix: String,
    /// Optional customer IAM role for BYO storage.
    pub role_arn: Option<String>,
    /// Confused-deputy protection value supplied to STS.
    pub external_id: Option<String>,
    /// Required server-side encryption mode.
    pub encryption: EncryptionMode,
    /// Customer KMS key when `encryption` is SSE-KMS.
    pub kms_key_arn: Option<String>,
}

/// Opaque key relative to a storage target's tenant prefix.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ObjectKey(String);

impl ObjectKey {
    /// Constructs a safe server-generated relative key.
    ///
    /// # Errors
    ///
    /// Rejects empty, absolute, traversal, or control-character values.
    pub fn new(value: impl Into<String>) -> Result<Self, ObjectKeyError> {
        let value = value.into();
        if value.is_empty()
            || value.starts_with('/')
            || value.ends_with('/')
            || value.contains("//")
            || value
                .split('/')
                .any(|segment| matches!(segment, "" | "." | ".."))
            || value.chars().any(char::is_control)
        {
            return Err(ObjectKeyError);
        }
        Ok(Self(value))
    }

    /// Borrows the relative key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Invalid server-generated object key.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("object key is not a safe relative key")]
pub struct ObjectKeyError;

/// Result of an object write or multipart completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredObject {
    /// Exact key used within the target.
    pub key: ObjectKey,
    /// Provider entity tag; it is not treated as a cryptographic checksum.
    pub etag: Option<String>,
    /// Provider-native object version, when bucket versioning is enabled.
    pub provider_version_id: Option<String>,
    /// Confirmed final byte count.
    pub size: u64,
    /// Provider-verified integrity metadata, when returned by the backend.
    pub checksum: Option<ObjectChecksum>,
}

/// Provider metadata used during reconciliation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectMetadata {
    /// Confirmed object byte count.
    pub size: u64,
    /// Provider entity tag.
    pub etag: Option<String>,
    /// Provider-native object version.
    pub provider_version_id: Option<String>,
    /// Provider-verified integrity metadata, when requested and available.
    pub checksum: Option<ObjectChecksum>,
}

/// One part supplied to S3 multipart completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredPart {
    /// One-based part number.
    pub part_number: u32,
    /// Exact `ETag` returned by the provider.
    pub etag: String,
    /// Locally calculated digest verified by the provider for this part.
    pub checksum_sha256: [u8; 32],
}

/// Exact provider request for one staged multipart part.
#[derive(Clone, Copy, Debug)]
pub struct UploadPartRequest<'a> {
    /// Resolved immutable storage destination.
    pub target: &'a StorageTarget,
    /// Opaque final object key.
    pub key: &'a ObjectKey,
    /// Provider multipart session identifier.
    pub provider_upload_id: &'a str,
    /// One-based part number.
    pub part_number: u32,
    /// Private staged part path.
    pub path: &'a Path,
    /// Exact staged byte count.
    pub size: u64,
    /// SHA-256 calculated while staging.
    pub checksum_sha256: &'a [u8; 32],
}

/// Exact provider request for one immutable object byte range.
#[derive(Clone, Copy, Debug)]
pub struct DownloadRangeRequest<'a> {
    /// Resolved immutable source destination.
    pub target: &'a StorageTarget,
    /// Opaque source object key.
    pub key: &'a ObjectKey,
    /// Exact provider version retained with the content version, when enabled.
    pub provider_version_id: Option<&'a str>,
    /// Private temporary path that is replaced by the downloaded range.
    pub path: &'a Path,
    /// Zero-based first byte in the inclusive range.
    pub offset: u64,
    /// Exact number of bytes required.
    pub size: u64,
    /// Optional source `ETag` used to reject a concurrent replacement.
    pub if_match: Option<&'a str>,
}

/// Exact request for a provider-side immutable object copy.
#[derive(Clone, Copy, Debug)]
pub struct CopyObjectRequest<'a> {
    /// Shared source and destination storage target.
    pub target: &'a StorageTarget,
    /// Historical source object key.
    pub source: &'a ObjectKey,
    /// Exact provider version retained with the historical source, when enabled.
    pub source_provider_version_id: Option<&'a str>,
    /// Newly reserved destination object key.
    pub destination: &'a ObjectKey,
    /// Media type copied onto the destination metadata.
    pub content_type: &'a str,
    /// Persisted source size that provider metadata must match.
    pub expected_size: u64,
    /// Persisted source checksum that provider metadata must match.
    pub expected_checksum: &'a ObjectChecksum,
}

/// An inclusive byte range of one object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteRange {
    /// First byte offset, inclusive.
    pub start: u64,
    /// Last byte offset, inclusive.
    pub end: u64,
}

impl ByteRange {
    /// Returns the number of bytes covered by the range.
    #[must_use]
    pub const fn length(self) -> u64 {
        self.end.saturating_sub(self.start).saturating_add(1)
    }
}

/// An unresolved client range request, before the exact size is known.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RangeRequest {
    /// Everything from an offset to the end of the object.
    From(u64),
    /// An inclusive offset pair.
    Between(u64, u64),
    /// The final N bytes of the object.
    Last(u64),
}

/// Request to open an object for direct relay to an authorized client.
#[derive(Clone, Copy, Debug)]
pub struct OpenObjectRequest<'a> {
    /// Resolved storage destination.
    pub target: &'a StorageTarget,
    /// Immutable object key.
    pub key: &'a ObjectKey,
    /// Exact provider object version, when bucket versioning is enabled.
    pub provider_version_id: Option<&'a str>,
    /// Requested inclusive byte range, or the complete object when absent.
    pub range: Option<ByteRange>,
}

/// An object opened for streaming, without buffering it in the process.
pub struct OpenObject {
    /// Complete object size, independent of the served range.
    pub total_size: u64,
    /// Range actually served, present only for a partial read.
    pub range: Option<ByteRange>,
    /// Provider entity tag, when supplied.
    pub etag: Option<String>,
    /// Provider byte stream in object order.
    pub body: BoxStream<'static, std::io::Result<Bytes>>,
}

impl fmt::Debug for OpenObject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenObject")
            .field("total_size", &self.total_size)
            .field("range", &self.range)
            .finish_non_exhaustive()
    }
}

/// Result of a BYO bucket CRUD probe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageValidation {
    /// AWS account observed through the assumed identity.
    pub account_id: String,
}

/// Classified storage failure that application services can safely map.
#[derive(Debug, Error)]
pub enum ObjectStoreError {
    /// Object or multipart session is absent.
    #[error("storage resource was not found")]
    NotFound,
    /// Provider state conflicts with the requested transition.
    #[error("storage resource conflicts with the requested operation")]
    Conflict,
    /// Customer storage settings cannot satisfy the required probe.
    #[error("storage configuration is invalid")]
    InvalidConfiguration,
    /// Provider could not serve a valid request before its deadline.
    #[error("object storage is unavailable")]
    Unavailable,
    /// Unexpected adapter failure whose detail must stay inside the service.
    #[error("internal object-storage failure")]
    Internal(#[source] anyhow::Error),
}

/// Object storage behavior required by Briefcase use cases.
#[async_trait]
pub trait ObjectStore: Send + Sync {
    /// Streams a local temporary file into an object.
    async fn put_file(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        path: &Path,
        content_type: &str,
        size: u64,
        checksum_sha256: &[u8; 32],
    ) -> Result<StoredObject, ObjectStoreError>;

    /// Opens an object, or one exact range of it, as a relayable byte stream.
    async fn open_object(
        &self,
        request: OpenObjectRequest<'_>,
    ) -> Result<OpenObject, ObjectStoreError>;

    /// Streams an object into a local temporary file.
    async fn get_to_file(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_version_id: Option<&str>,
        path: &Path,
    ) -> Result<ObjectMetadata, ObjectStoreError>;

    /// Streams one exact, conditional object range into a local temporary file.
    async fn get_range_to_file(
        &self,
        request: DownloadRangeRequest<'_>,
    ) -> Result<(), ObjectStoreError>;

    /// Reads provider metadata without transferring content.
    async fn head(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_version_id: Option<&str>,
    ) -> Result<ObjectMetadata, ObjectStoreError>;

    /// Copies an immutable version to a new key in the same storage target.
    async fn copy(&self, request: CopyObjectRequest<'_>) -> Result<StoredObject, ObjectStoreError>;

    /// Deletes an exact object version when supplied, or the current object
    /// idempotently for an unversioned target.
    async fn delete(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_version_id: Option<&str>,
    ) -> Result<(), ObjectStoreError>;

    /// Creates a provider multipart session.
    async fn create_multipart(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        content_type: &str,
    ) -> Result<String, ObjectStoreError>;

    /// Streams one temporary part file into an active provider session.
    async fn upload_part(&self, request: UploadPartRequest<'_>)
    -> Result<String, ObjectStoreError>;

    /// Atomically assembles the exact ordered part set.
    async fn complete_multipart(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_upload_id: &str,
        parts: &[StoredPart],
        expected_size: u64,
    ) -> Result<StoredObject, ObjectStoreError>;

    /// Aborts a provider multipart session idempotently.
    async fn abort_multipart(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_upload_id: &str,
    ) -> Result<(), ObjectStoreError>;

    /// Performs create/read/overwrite/delete and identity validation.
    async fn validate_configuration(
        &self,
        target: &StorageTarget,
        expected_account_id: &str,
    ) -> Result<StorageValidation, ObjectStoreError>;
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    use super::{ObjectChecksum, ObjectChecksumAlgorithm, ObjectChecksumType, ObjectKey};

    #[test]
    fn object_key_rejects_traversal() {
        assert!(ObjectKey::new("objects/../secret").is_err());
        assert!(ObjectKey::new("/objects/file").is_err());
        assert!(ObjectKey::new("objects//file").is_err());
    }

    #[test]
    fn object_key_accepts_opaque_segments() {
        let result = ObjectKey::new("objects/01961f8d/version/01961f90");
        assert!(result.is_ok());
    }

    #[test]
    fn checksum_requires_a_sha256_shape_matching_its_type() {
        let digest = STANDARD.encode([7_u8; 32]);
        assert!(
            ObjectChecksum::new(
                ObjectChecksumAlgorithm::Sha256,
                ObjectChecksumType::FullObject,
                &digest,
            )
            .is_ok()
        );
        assert!(
            ObjectChecksum::new(
                ObjectChecksumAlgorithm::Sha256,
                ObjectChecksumType::Composite,
                format!("{digest}-3"),
            )
            .is_ok()
        );
        assert!(
            ObjectChecksum::new(
                ObjectChecksumAlgorithm::Sha256,
                ObjectChecksumType::Composite,
                &digest,
            )
            .is_err()
        );
        assert!(
            ObjectChecksum::new(
                ObjectChecksumAlgorithm::Sha256,
                ObjectChecksumType::FullObject,
                format!("{digest}-3"),
            )
            .is_err()
        );
    }
}
