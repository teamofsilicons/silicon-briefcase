//! File-content, multipart-upload, delivery, restore, and storage use cases.

use std::{path::Path, path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::Bytes;
use futures::stream::BoxStream;
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
use tracing::warn;

use crate::{
    application::{
        context::ExecutionContext,
        idempotency::IdempotencyKey,
        ports::{
            ByteRange, CopyObjectRequest, DownloadRangeRequest, ObjectChecksum,
            ObjectChecksumAlgorithm, ObjectChecksumType, ObjectKey, ObjectMetadata, ObjectStore,
            ObjectStoreError, OpenObjectRequest, RangeRequest, StorageTarget, StoredObject,
            StoredPart, UploadPartRequest,
        },
    },
    domain::{
        entry::EntryName,
        ids::{EntryId, MultipartUploadId, StorageConfigurationId, VersionId},
        multipart::{
            CompletedPart, MULTIPART_SESSION_TTL_SECONDS, MultipartPlan, MultipartPlanError,
            SINGLE_UPLOAD_MAX_BYTES, UploadStrategy, validate_completion,
        },
        storage::EncryptionMode,
    },
    error::AppError,
};

/// How often a long-running idempotent restore renews its database lease.
const RESTORE_LEASE_RENEWAL_INTERVAL: Duration = Duration::from_mins(1);
/// Lease window left after every successful restore heartbeat.
pub(crate) const RESTORE_LEASE_DURATION: Duration = Duration::from_mins(10);

/// One upload, whatever its size.
#[derive(Clone, Debug)]
pub struct UploadCommand {
    /// Destination folder.
    pub parent_id: EntryId,
    /// File name; an existing file with this name receives a new version.
    pub name: EntryName,
    /// Declared media type.
    pub content_type: String,
    /// Client idempotency key.
    pub idempotency_key: IdempotencyKey,
    /// Canonical request fingerprint.
    pub request_hash: [u8; 32],
}

/// A body already staged and hashed by the HTTP streaming boundary.
#[derive(Clone, Copy, Debug)]
pub struct StagedContent<'a> {
    /// Private temporary path.
    pub path: &'a Path,
    /// First byte of the staged file this content starts at.
    ///
    /// A multipart transfer sends ranges of one staged file rather than
    /// copying each part to its own temporary file.
    pub offset: u64,
    /// Exact received bytes.
    pub size: u64,
    /// SHA-256 computed over exactly those bytes.
    pub sha256: [u8; 32],
}

/// Small file upload command.
#[derive(Clone, Debug)]
pub struct SmallUploadCommand {
    /// Destination folder.
    pub parent_id: EntryId,
    /// Validated final name.
    pub name: EntryName,
    /// Valid media type.
    pub content_type: String,
    /// Required idempotency key.
    pub idempotency_key: IdempotencyKey,
    /// Canonical metadata-and-content request digest.
    pub request_hash: [u8; 32],
}

/// Multipart initialization command.
#[derive(Clone, Debug)]
pub struct InitiateMultipartCommand {
    /// Destination folder.
    pub parent_id: EntryId,
    /// Validated final name.
    pub name: EntryName,
    /// Declared complete byte count.
    pub size: u64,
    /// Valid media type.
    pub content_type: String,
    /// Required idempotency key.
    pub idempotency_key: IdempotencyKey,
    /// Canonical request digest.
    pub request_hash: [u8; 32],
}

/// Multipart completion command.
#[derive(Clone, Debug)]
pub struct CompleteMultipartCommand {
    /// Briefcase multipart identifier.
    pub upload_id: MultipartUploadId,
    /// Client-retained ordered provider `ETags`.
    pub parts: Vec<ClientCompletedPart>,
    /// Required idempotency key.
    pub idempotency_key: IdempotencyKey,
    /// Canonical request digest.
    pub request_hash: [u8; 32],
}

/// Client-reported part completion tuple.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientCompletedPart {
    /// One-based part number.
    pub part_number: u32,
    /// Exact provider `ETag` returned by upload-part.
    pub etag: String,
}

/// Version restore command.
#[derive(Clone, Debug)]
pub struct RestoreVersionCommand {
    /// Current file entry.
    pub entry_id: EntryId,
    /// Retained source version.
    pub version_id: VersionId,
    /// Required idempotency key.
    pub idempotency_key: IdempotencyKey,
    /// Canonical restore request digest.
    pub request_hash: [u8; 32],
}

/// Organization-owned storage configuration command.
#[derive(Clone, Debug)]
pub struct ConfigureStorageCommand {
    /// Bucket name.
    pub bucket_name: String,
    /// AWS region.
    pub region: String,
    /// Cross-account role ARN.
    pub role_arn: String,
    /// Tenant prefix.
    pub prefix: String,
    /// Expected AWS account ID.
    pub aws_account_id: String,
    /// Required server-side encryption.
    pub encryption: EncryptionMode,
    /// KMS key for SSE-KMS.
    pub kms_key_arn: Option<String>,
    /// Optional replay key until the HTTP contract makes it mandatory.
    pub idempotency: Option<(IdempotencyKey, [u8; 32])>,
}

/// Public multipart initialization result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultipartReceipt {
    /// Briefcase session identifier.
    pub upload_id: MultipartUploadId,
    /// Canonical non-final part bytes.
    pub part_size: u64,
    /// Exact part count.
    pub part_count: u32,
    /// Automatic expiration.
    pub expires_at: OffsetDateTime,
}

/// Why an authorized reader is opening file content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentIntent {
    /// Render the bytes in place, inside the client's sandbox.
    Render,
    /// Save the bytes locally as an attachment.
    Download,
}

impl ContentIntent {
    /// Returns the audited action name for this intent.
    #[must_use]
    pub const fn audit_action(self) -> &'static str {
        match self {
            Self::Render => "entry.content_read.v1",
            Self::Download => "entry.downloaded.v1",
        }
    }
}

/// Authorized file bytes opened for direct relay to the caller.
pub struct ContentDelivery {
    /// User-visible file name.
    pub filename: String,
    /// Current media type of the file.
    pub content_type: String,
    /// Complete file size, independent of the served range.
    pub total_size: u64,
    /// Range actually served, present only for a partial read.
    pub range: Option<ByteRange>,
    /// Provider entity tag, when supplied.
    pub etag: Option<String>,
    /// Object byte stream in file order.
    pub body: BoxStream<'static, std::io::Result<Bytes>>,
}

/// Public result of storage validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageConfigurationResult {
    /// Configuration revision.
    pub configuration_id: StorageConfigurationId,
    /// Whether validation succeeded and activated the configuration.
    pub configured: bool,
    /// Probe completion time.
    pub tested_at: OffsetDateTime,
    /// Stable redacted failure category.
    pub failure_reason: Option<String>,
}

/// Result of claiming an idempotent operation.
#[derive(Clone, Debug)]
pub enum Prepared<T, R> {
    /// Caller owns the operation lease and must finish or release it.
    Acquired(T),
    /// A completed prior request is replayed.
    Replay(R),
}

/// Durable preparation for a single-request file upload.
#[derive(Clone, Debug)]
pub struct SmallUploadPreparation {
    /// Reserved entry ID and deterministic object namespace.
    pub entry_id: EntryId,
    /// Resolved immutable storage destination.
    pub target: StorageTarget,
    /// Opaque object key.
    pub key: ObjectKey,
}

/// Durable preparation before creating a provider multipart session.
#[derive(Clone, Debug)]
pub struct MultipartPreparation {
    /// Reserved Briefcase upload ID.
    pub upload_id: MultipartUploadId,
    /// Calculated multipart plan.
    pub plan: MultipartPlan,
    /// Resolved immutable storage destination.
    pub target: StorageTarget,
    /// Opaque final object key.
    pub key: ObjectKey,
    /// Session expiration.
    pub expires_at: OffsetDateTime,
}

/// Authorized target for a part upload.
#[derive(Clone, Debug)]
pub struct MultipartPartTarget {
    /// Calculated plan.
    pub plan: MultipartPlan,
    /// Resolved storage destination.
    pub target: StorageTarget,
    /// Opaque final object key.
    pub key: ObjectKey,
    /// Provider multipart identifier.
    pub provider_upload_id: String,
}

/// Authorized and locked multipart completion state.
#[derive(Clone, Debug)]
pub struct MultipartCompletionPreparation {
    /// File entry that completion will publish.
    pub entry_id: EntryId,
    /// Calculated plan.
    pub plan: MultipartPlan,
    /// Resolved storage destination.
    pub target: StorageTarget,
    /// Opaque final object key.
    pub key: ObjectKey,
    /// Provider multipart identifier.
    pub provider_upload_id: String,
    /// Server-recorded exact part metadata.
    pub parts: Vec<CompletedPart>,
}

/// Authorized multipart abort target.
#[derive(Clone, Debug)]
pub struct MultipartAbortTarget {
    /// Resolved storage destination.
    pub target: StorageTarget,
    /// Opaque final object key.
    pub key: ObjectKey,
    /// Provider multipart identifier.
    pub provider_upload_id: String,
}

/// Authorized current file bytes for delivery.
#[derive(Clone, Debug)]
pub struct DownloadTarget {
    /// Entry being delivered.
    pub entry_id: EntryId,
    /// User-visible download name.
    pub filename: String,
    /// Media type persisted with the current version.
    pub content_type: String,
    /// Exact size of the current version.
    pub size: u64,
    /// Version storage destination.
    pub target: StorageTarget,
    /// Immutable object key.
    pub key: ObjectKey,
    /// Exact provider object version, when bucket versioning is enabled.
    pub provider_version_id: Option<String>,
}

/// Authorized version restore source and reserved destination.
#[derive(Clone, Debug)]
pub struct RestorePreparation {
    /// Current entry.
    pub entry_id: EntryId,
    /// New version identifier.
    pub new_version_id: VersionId,
    /// Source version storage destination.
    pub source_target: StorageTarget,
    /// Source immutable object key.
    pub source_key: ObjectKey,
    /// Exact provider source version, when bucket versioning is enabled.
    pub source_provider_version_id: Option<String>,
    /// Destination storage destination for the new version.
    pub destination_target: StorageTarget,
    /// New immutable object key.
    pub destination_key: ObjectKey,
    /// Content media type.
    pub content_type: String,
    /// Exact content size.
    pub size: u64,
    /// Original verified checksum.
    pub checksum: ObjectChecksum,
}

/// Persisted pending BYO configuration.
#[derive(Clone, Debug)]
pub struct StorageConfigurationPreparation {
    /// Configuration revision.
    pub configuration_id: StorageConfigurationId,
    /// Resolved customer target, including server-derived external ID.
    pub target: StorageTarget,
    /// Expected account ID.
    pub expected_account_id: String,
}

/// Database operations needed by content orchestration.
#[async_trait]
pub trait ContentRepository: Send + Sync {
    /// Authorizes, reserves IDs, and claims small-upload idempotency.
    async fn prepare_small_upload(
        &self,
        context: &ExecutionContext,
        command: &SmallUploadCommand,
        content: &StagedContent<'_>,
    ) -> Result<Prepared<SmallUploadPreparation, EntryId>, AppError>;

    /// Publishes initial file metadata after object storage succeeds.
    async fn commit_small_upload(
        &self,
        context: &ExecutionContext,
        command: &SmallUploadCommand,
        content: &StagedContent<'_>,
        preparation: &SmallUploadPreparation,
        stored: &StoredObject,
    ) -> Result<EntryId, AppError>;

    /// Releases a failed idempotency lease for safe retry.
    async fn release_operation(
        &self,
        context: &ExecutionContext,
        operation: &'static str,
        key: &IdempotencyKey,
        request_hash: &[u8; 32],
    ) -> Result<(), AppError>;

    /// Authorizes, reserves IDs, and claims multipart-init idempotency.
    async fn prepare_multipart(
        &self,
        context: &ExecutionContext,
        command: &InitiateMultipartCommand,
        plan: MultipartPlan,
        expires_at: OffsetDateTime,
    ) -> Result<Prepared<MultipartPreparation, MultipartReceipt>, AppError>;

    /// Persists the provider multipart identifier and active state.
    async fn commit_multipart_initialization(
        &self,
        context: &ExecutionContext,
        command: &InitiateMultipartCommand,
        preparation: &MultipartPreparation,
        provider_upload_id: &str,
    ) -> Result<MultipartReceipt, AppError>;

    /// Authorizes a numbered part against the current session.
    async fn authorize_multipart_part(
        &self,
        context: &ExecutionContext,
        upload_id: MultipartUploadId,
        part_number: u32,
    ) -> Result<MultipartPartTarget, AppError>;

    /// Records a provider-confirmed part, replacing the same number.
    async fn record_multipart_part(
        &self,
        context: &ExecutionContext,
        upload_id: MultipartUploadId,
        part_number: u32,
        etag: &str,
        content: &StagedContent<'_>,
    ) -> Result<(), AppError>;

    /// Claims completion and verifies client `ETags` against stored parts.
    async fn prepare_multipart_completion(
        &self,
        context: &ExecutionContext,
        command: &CompleteMultipartCommand,
    ) -> Result<Prepared<MultipartCompletionPreparation, EntryId>, AppError>;

    /// Publishes file metadata and completes the multipart idempotency record.
    async fn commit_multipart_completion(
        &self,
        context: &ExecutionContext,
        command: &CompleteMultipartCommand,
        preparation: &MultipartCompletionPreparation,
        stored: &StoredObject,
    ) -> Result<EntryId, AppError>;

    /// Atomically prevents further part/completion work and returns cleanup data.
    async fn abort_multipart(
        &self,
        context: &ExecutionContext,
        upload_id: MultipartUploadId,
    ) -> Result<Option<MultipartAbortTarget>, AppError>;

    /// Authorizes current file delivery and records the access intent.
    async fn authorize_download(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
    ) -> Result<DownloadTarget, AppError>;

    /// Records a completed authorized content read in the entry's history.
    async fn record_content_access(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
        intent: ContentIntent,
    ) -> Result<(), AppError>;

    /// Authorizes and reserves a new version for restore.
    async fn prepare_version_restore(
        &self,
        context: &ExecutionContext,
        command: &RestoreVersionCommand,
    ) -> Result<Prepared<RestorePreparation, EntryId>, AppError>;

    /// Extends the lease protecting one in-flight idempotent restore.
    async fn renew_version_restore_lease(
        &self,
        context: &ExecutionContext,
        command: &RestoreVersionCommand,
    ) -> Result<(), AppError>;

    /// Publishes a copied historical version as the new current version.
    async fn commit_version_restore(
        &self,
        context: &ExecutionContext,
        command: &RestoreVersionCommand,
        preparation: &RestorePreparation,
        stored: &StoredObject,
    ) -> Result<EntryId, AppError>;

    /// Authorizes and stores a pending organization storage revision.
    async fn prepare_storage_configuration(
        &self,
        context: &ExecutionContext,
        command: &ConfigureStorageCommand,
    ) -> Result<Prepared<StorageConfigurationPreparation, StorageConfigurationResult>, AppError>;

    /// Activates a successfully validated storage revision.
    async fn activate_storage_configuration(
        &self,
        context: &ExecutionContext,
        command: &ConfigureStorageCommand,
        preparation: &StorageConfigurationPreparation,
        tested_at: OffsetDateTime,
    ) -> Result<StorageConfigurationResult, AppError>;

    /// Persists a redacted failed validation without changing active storage.
    async fn fail_storage_configuration(
        &self,
        context: &ExecutionContext,
        command: &ConfigureStorageCommand,
        preparation: &StorageConfigurationPreparation,
        tested_at: OffsetDateTime,
        reason: &'static str,
    ) -> Result<StorageConfigurationResult, AppError>;
}

/// Orchestrates PostgreSQL metadata and non-transactional object storage.
pub struct ContentService<R: ?Sized, O: ?Sized> {
    repository: Arc<R>,
    objects: Arc<O>,
    temporary_directory: PathBuf,
}

/// Best-effort provider cleanup that remains armed while an object write has
/// no matching committed PostgreSQL state.
struct UnpublishedObjectGuard<O>
where
    O: ObjectStore + ?Sized + 'static,
{
    objects: Arc<O>,
    target: StorageTarget,
    key: ObjectKey,
    provider_upload_id: Option<String>,
    provider_version_id: Option<String>,
    armed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublicationFailureDisposition {
    PreserveProviderState,
    CompensateProviderState,
}

const fn publication_failure_disposition(error: &AppError) -> PublicationFailureDisposition {
    if matches!(error, AppError::DatabaseCommitOutcomeUnknown { .. }) {
        PublicationFailureDisposition::PreserveProviderState
    } else {
        PublicationFailureDisposition::CompensateProviderState
    }
}

impl<O> UnpublishedObjectGuard<O>
where
    O: ObjectStore + ?Sized + 'static,
{
    fn destination(objects: Arc<O>, target: StorageTarget, key: ObjectKey) -> Self {
        Self {
            objects,
            target,
            key,
            provider_upload_id: None,
            provider_version_id: None,
            armed: true,
        }
    }

    fn multipart(
        objects: Arc<O>,
        target: StorageTarget,
        key: ObjectKey,
        provider_upload_id: String,
    ) -> Self {
        Self {
            objects,
            target,
            key,
            provider_upload_id: Some(provider_upload_id),
            provider_version_id: None,
            armed: true,
        }
    }

    fn record_stored_object(&mut self, stored: &StoredObject) {
        self.provider_version_id
            .clone_from(&stored.provider_version_id);
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    async fn cleanup_now(&mut self) {
        cleanup_unpublished_object(
            self.objects.as_ref(),
            &self.target,
            &self.key,
            self.provider_upload_id.as_deref(),
            self.provider_version_id.as_deref(),
        )
        .await;
        self.armed = false;
    }
}

impl<O> Drop for UnpublishedObjectGuard<O>
where
    O: ObjectStore + ?Sized + 'static,
{
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            warn!("cancelled object cleanup could not start outside a Tokio runtime");
            return;
        };
        let objects = Arc::clone(&self.objects);
        let target = self.target.clone();
        let key = self.key.clone();
        let provider_upload_id = self.provider_upload_id.clone();
        let provider_version_id = self.provider_version_id.clone();
        drop(runtime.spawn(async move {
            cleanup_unpublished_object(
                objects.as_ref(),
                &target,
                &key,
                provider_upload_id.as_deref(),
                provider_version_id.as_deref(),
            )
            .await;
        }));
    }
}

impl<R, O> ContentService<R, O>
where
    R: ContentRepository + ?Sized,
    O: ObjectStore + ?Sized + 'static,
{
    /// Constructs content use cases from explicit ports.
    #[must_use]
    pub fn new(repository: Arc<R>, objects: Arc<O>, temporary_directory: PathBuf) -> Self {
        Self {
            repository,
            objects,
            temporary_directory,
        }
    }

    /// Stores and publishes a file through the at-most-100-MiB route.
    ///
    /// # Errors
    ///
    /// Returns a classified application error when validation, authorization,
    /// persistence, or object storage fails.
    pub async fn upload_small(
        &self,
        execution: &ExecutionContext,
        command: &SmallUploadCommand,
        staged: StagedContent<'_>,
    ) -> Result<EntryId, AppError> {
        if staged.size > SINGLE_UPLOAD_MAX_BYTES {
            return Err(AppError::PayloadTooLarge);
        }
        let preparation = match self
            .repository
            .prepare_small_upload(execution, command, &staged)
            .await?
        {
            Prepared::Replay(entry_id) => return Ok(entry_id),
            Prepared::Acquired(preparation) => preparation,
        };

        let mut cleanup = UnpublishedObjectGuard::destination(
            Arc::clone(&self.objects),
            preparation.target.clone(),
            preparation.key.clone(),
        );

        let stored = match self
            .objects
            .put_file(
                &preparation.target,
                &preparation.key,
                staged.path,
                &command.content_type,
                staged.size,
                &staged.sha256,
            )
            .await
        {
            Ok(stored) => {
                cleanup.record_stored_object(&stored);
                stored
            }
            Err(error) => {
                cleanup.cleanup_now().await;
                self.release(
                    execution,
                    "upload_file",
                    &command.idempotency_key,
                    &command.request_hash,
                )
                .await;
                return Err(map_object_error(&error));
            }
        };

        match self
            .repository
            .commit_small_upload(execution, command, &staged, &preparation, &stored)
            .await
        {
            Ok(entry_id) => {
                cleanup.disarm();
                Ok(entry_id)
            }
            Err(error)
                if publication_failure_disposition(&error)
                    == PublicationFailureDisposition::PreserveProviderState =>
            {
                cleanup.disarm();
                Err(error)
            }
            Err(error) => {
                cleanup.cleanup_now().await;
                self.release(
                    execution,
                    "upload_file",
                    &command.idempotency_key,
                    &command.request_hash,
                )
                .await;
                Err(error)
            }
        }
    }

    /// Stores an uploaded file, choosing the storage route from its size.
    ///
    /// The contract has one upload: the caller streams bytes and Briefcase
    /// decides whether they fit in a single provider request or need a
    /// multipart transfer. Uploading over an existing file publishes that
    /// file's next version.
    ///
    /// # Errors
    ///
    /// Returns a classified application error when the size is out of range or
    /// authorization, persistence, or object storage fails.
    pub async fn upload(
        &self,
        context: &ExecutionContext,
        command: &UploadCommand,
        staged: StagedContent<'_>,
    ) -> Result<EntryId, AppError> {
        match UploadStrategy::for_file_size(staged.size).map_err(|error| map_plan_error(&error))? {
            UploadStrategy::SingleRequest => {
                self.upload_small(
                    context,
                    &SmallUploadCommand {
                        parent_id: command.parent_id,
                        name: command.name.clone(),
                        content_type: command.content_type.clone(),
                        idempotency_key: command.idempotency_key.clone(),
                        request_hash: command.request_hash,
                    },
                    staged,
                )
                .await
            }
            UploadStrategy::Multipart(plan) => {
                self.upload_multipart(context, command, staged, plan).await
            }
        }
    }

    /// Transfers one staged file as a multipart upload, part by part.
    ///
    /// Every step reuses the durable session the contracted multipart flow
    /// uses, so an interrupted transfer leaves exactly the state the worker
    /// already knows how to abort and clean up, and a retry with the same
    /// idempotency key resumes rather than duplicating.
    async fn upload_multipart(
        &self,
        context: &ExecutionContext,
        command: &UploadCommand,
        staged: StagedContent<'_>,
        plan: MultipartPlan,
    ) -> Result<EntryId, AppError> {
        let receipt = self
            .initiate_multipart(
                context,
                &InitiateMultipartCommand {
                    parent_id: command.parent_id,
                    name: command.name.clone(),
                    size: staged.size,
                    content_type: command.content_type.clone(),
                    idempotency_key: command.idempotency_key.clone(),
                    request_hash: command.request_hash,
                },
            )
            .await?;

        let mut parts = Vec::with_capacity(plan.part_count() as usize);
        for part_number in 1..=plan.part_count() {
            let size = plan
                .expected_part_size(part_number)
                .map_err(|_| AppError::Internal {
                    category: "multipart_plan",
                })?;
            let offset = u64::from(part_number - 1) * plan.part_size();
            let sha256 = sha256_file_range(staged.path, offset, size).await?;
            let etag = self
                .upload_part(
                    context,
                    receipt.upload_id,
                    part_number,
                    StagedContent {
                        path: staged.path,
                        offset,
                        size,
                        sha256,
                    },
                )
                .await?;
            parts.push(ClientCompletedPart { part_number, etag });
        }

        self.complete_multipart(
            context,
            &CompleteMultipartCommand {
                upload_id: receipt.upload_id,
                parts,
                idempotency_key: command.idempotency_key.clone(),
                request_hash: command.request_hash,
            },
        )
        .await
    }

    /// Creates an S3 multipart session using the canonical plan.
    ///
    /// # Errors
    ///
    /// Returns a classified application error when validation, authorization,
    /// persistence, or object storage fails.
    pub async fn initiate_multipart(
        &self,
        context: &ExecutionContext,
        command: &InitiateMultipartCommand,
    ) -> Result<MultipartReceipt, AppError> {
        let plan =
            MultipartPlan::for_file_size(command.size).map_err(|error| map_plan_error(&error))?;
        let expires_at =
            OffsetDateTime::now_utc() + time::Duration::seconds(MULTIPART_SESSION_TTL_SECONDS);
        let preparation = match self
            .repository
            .prepare_multipart(context, command, plan, expires_at)
            .await?
        {
            Prepared::Replay(receipt) => return Ok(receipt),
            Prepared::Acquired(preparation) => preparation,
        };
        let provider_upload_id = match self
            .objects
            .create_multipart(&preparation.target, &preparation.key, &command.content_type)
            .await
        {
            Ok(identifier) => identifier,
            Err(error) => {
                self.release(
                    context,
                    "initiate_multipart_upload",
                    &command.idempotency_key,
                    &command.request_hash,
                )
                .await;
                return Err(map_object_error(&error));
            }
        };

        let mut cleanup = UnpublishedObjectGuard::multipart(
            Arc::clone(&self.objects),
            preparation.target.clone(),
            preparation.key.clone(),
            provider_upload_id.clone(),
        );
        match self
            .repository
            .commit_multipart_initialization(context, command, &preparation, &provider_upload_id)
            .await
        {
            Ok(receipt) => {
                cleanup.disarm();
                Ok(receipt)
            }
            Err(error)
                if publication_failure_disposition(&error)
                    == PublicationFailureDisposition::PreserveProviderState =>
            {
                cleanup.disarm();
                Err(error)
            }
            Err(error) => {
                cleanup.cleanup_now().await;
                self.release(
                    context,
                    "initiate_multipart_upload",
                    &command.idempotency_key,
                    &command.request_hash,
                )
                .await;
                Err(error)
            }
        }
    }

    /// Uploads and records one exact multipart part.
    ///
    /// # Errors
    ///
    /// Returns a classified application error when the part is invalid or a
    /// persistence or object-storage operation fails.
    pub async fn upload_part(
        &self,
        execution: &ExecutionContext,
        upload_id: MultipartUploadId,
        part_number: u32,
        staged: StagedContent<'_>,
    ) -> Result<String, AppError> {
        let target = self
            .repository
            .authorize_multipart_part(execution, upload_id, part_number)
            .await?;
        let expected_size = target
            .plan
            .expected_part_size(part_number)
            .map_err(|_| AppError::validation("invalid_part_number"))?;
        if staged.size != expected_size {
            return Err(AppError::validation("invalid_part_size"));
        }
        let etag = self
            .objects
            .upload_part(UploadPartRequest {
                target: &target.target,
                key: &target.key,
                provider_upload_id: &target.provider_upload_id,
                part_number,
                path: staged.path,
                offset: staged.offset,
                size: staged.size,
                checksum_sha256: &staged.sha256,
            })
            .await
            .map_err(|error| map_object_error(&error))?;
        self.repository
            .record_multipart_part(execution, upload_id, part_number, &etag, &staged)
            .await?;
        Ok(etag)
    }

    /// Verifies and assembles the exact multipart part set once.
    ///
    /// # Errors
    ///
    /// Returns a classified application error when the part set, persistence,
    /// or provider completion fails.
    pub async fn complete_multipart(
        &self,
        context: &ExecutionContext,
        command: &CompleteMultipartCommand,
    ) -> Result<EntryId, AppError> {
        let preparation = match self
            .repository
            .prepare_multipart_completion(context, command)
            .await?
        {
            Prepared::Replay(entry_id) => return Ok(entry_id),
            Prepared::Acquired(preparation) => preparation,
        };
        validate_completion(preparation.plan, &preparation.parts)
            .map_err(|_| AppError::validation("invalid_multipart_completion"))?;
        let parts = preparation
            .parts
            .iter()
            .map(|part| StoredPart {
                part_number: part.part_number(),
                etag: part.etag().to_owned(),
                checksum_sha256: *part.checksum_sha256(),
            })
            .collect::<Vec<_>>();
        let stored = match self
            .objects
            .complete_multipart(
                &preparation.target,
                &preparation.key,
                &preparation.provider_upload_id,
                &parts,
                preparation.plan.file_size(),
            )
            .await
        {
            Ok(stored) => stored,
            Err(error) => {
                match self
                    .reconcile_completed_multipart(&preparation, parts.len())
                    .await
                {
                    Ok(stored) => stored,
                    Err(_) => return Err(map_object_error(&error)),
                }
            }
        };
        self.repository
            .commit_multipart_completion(context, command, &preparation, &stored)
            .await
    }

    /// Aborts a multipart session and releases provider parts.
    ///
    /// # Errors
    ///
    /// Returns a classified application error when authorization, persistence,
    /// or provider cleanup fails.
    pub async fn abort_multipart(
        &self,
        context: &ExecutionContext,
        upload_id: MultipartUploadId,
    ) -> Result<(), AppError> {
        let Some(target) = self.repository.abort_multipart(context, upload_id).await? else {
            return Ok(());
        };
        self.objects
            .abort_multipart(&target.target, &target.key, &target.provider_upload_id)
            .await
            .map_err(|error| map_object_error(&error))
    }

    /// Authorizes a read and opens the current file bytes for direct relay.
    ///
    /// Briefcase proxies the bytes itself instead of handing out a signed
    /// provider URL, so a permanent URL never becomes a bearer capability and
    /// every read stays bound to a current IAM identity.
    ///
    /// # Errors
    ///
    /// Returns a classified application error when authorization, the
    /// requested range, provider access, or access auditing fails.
    pub async fn open_content(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
        intent: ContentIntent,
        range: Option<RangeRequest>,
    ) -> Result<ContentDelivery, AppError> {
        let target = self
            .repository
            .authorize_download(context, entry_id)
            .await?;
        let range = range
            .map(|request| resolve_range(request, target.size))
            .transpose()?;
        let object = self
            .objects
            .open_object(OpenObjectRequest {
                target: &target.target,
                key: &target.key,
                provider_version_id: target.provider_version_id.as_deref(),
                range,
            })
            .await
            .map_err(|error| map_object_error(&error))?;
        self.repository
            .record_content_access(context, entry_id, intent)
            .await?;
        Ok(ContentDelivery {
            filename: target.filename,
            content_type: target.content_type,
            total_size: object.total_size,
            range: object.range,
            etag: object.etag,
            body: object.body,
        })
    }

    /// Restores historical bytes into a new immutable current version.
    ///
    /// # Errors
    ///
    /// Returns a classified application error when authorization, copying, or
    /// version publication fails.
    pub async fn restore_version(
        &self,
        context: &ExecutionContext,
        command: &RestoreVersionCommand,
    ) -> Result<EntryId, AppError> {
        let preparation = match self
            .repository
            .prepare_version_restore(context, command)
            .await?
        {
            Prepared::Replay(entry_id) => return Ok(entry_id),
            Prepared::Acquired(preparation) => preparation,
        };
        let mut cleanup = UnpublishedObjectGuard::destination(
            Arc::clone(&self.objects),
            preparation.destination_target.clone(),
            preparation.destination_key.clone(),
        );
        let copy = self.copy_version_for_restore(&preparation);
        tokio::pin!(copy);
        let mut renewals = tokio::time::interval(RESTORE_LEASE_RENEWAL_INTERVAL);
        renewals.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        renewals.tick().await;
        let stored = loop {
            tokio::select! {
                result = &mut copy => break result,
                _ = renewals.tick() => {
                    if let Err(error) = self.repository
                        .renew_version_restore_lease(context, command)
                        .await
                    {
                        break Err(error);
                    }
                }
            }
        };
        let stored = match stored {
            Ok(stored) => stored,
            Err(error) => {
                cleanup.cleanup_now().await;
                self.release(
                    context,
                    "restore_version",
                    &command.idempotency_key,
                    &command.request_hash,
                )
                .await;
                return Err(error);
            }
        };
        cleanup.record_stored_object(&stored);
        match self
            .repository
            .commit_version_restore(context, command, &preparation, &stored)
            .await
        {
            Ok(entry_id) => {
                cleanup.disarm();
                Ok(entry_id)
            }
            Err(error)
                if publication_failure_disposition(&error)
                    == PublicationFailureDisposition::PreserveProviderState =>
            {
                cleanup.disarm();
                Err(error)
            }
            Err(error) => {
                cleanup.cleanup_now().await;
                self.release(
                    context,
                    "restore_version",
                    &command.idempotency_key,
                    &command.request_hash,
                )
                .await;
                Err(error)
            }
        }
    }

    async fn copy_version_for_restore(
        &self,
        preparation: &RestorePreparation,
    ) -> Result<StoredObject, AppError> {
        if let Some(stored) = self
            .reconcile_reserved_restore_destination(preparation)
            .await?
        {
            return Ok(stored);
        }
        if preparation.source_target == preparation.destination_target {
            return self
                .objects
                .copy(CopyObjectRequest {
                    target: &preparation.source_target,
                    source: &preparation.source_key,
                    source_provider_version_id: preparation.source_provider_version_id.as_deref(),
                    destination: &preparation.destination_key,
                    content_type: &preparation.content_type,
                    expected_size: preparation.size,
                    expected_checksum: &preparation.checksum,
                })
                .await
                .map_err(|error| map_object_error(&error));
        }
        self.copy_between_targets(preparation).await
    }

    async fn reconcile_reserved_restore_destination(
        &self,
        preparation: &RestorePreparation,
    ) -> Result<Option<StoredObject>, AppError> {
        let metadata = match self
            .objects
            .head(
                &preparation.destination_target,
                &preparation.destination_key,
                None,
            )
            .await
        {
            Ok(metadata) => metadata,
            Err(ObjectStoreError::NotFound) => return Ok(None),
            Err(error) => return Err(map_object_error(&error)),
        };
        if metadata.size == preparation.size
            && metadata.checksum.as_ref() == Some(&preparation.checksum)
        {
            return Ok(Some(StoredObject {
                key: preparation.destination_key.clone(),
                etag: metadata.etag,
                provider_version_id: metadata.provider_version_id,
                size: metadata.size,
                checksum: metadata.checksum,
            }));
        }

        self.objects
            .delete(
                &preparation.destination_target,
                &preparation.destination_key,
                metadata.provider_version_id.as_deref(),
            )
            .await
            .map_err(|error| map_object_error(&error))?;
        Ok(None)
    }

    /// Validates and conditionally activates organization-owned S3 storage.
    ///
    /// # Errors
    ///
    /// Returns a classified application error when authorization or durable
    /// configuration state cannot be established.
    pub async fn configure_storage(
        &self,
        context: &ExecutionContext,
        command: &ConfigureStorageCommand,
    ) -> Result<StorageConfigurationResult, AppError> {
        let preparation = match self
            .repository
            .prepare_storage_configuration(context, command)
            .await?
        {
            Prepared::Replay(result) => return Ok(result),
            Prepared::Acquired(preparation) => preparation,
        };
        let tested_at = OffsetDateTime::now_utc();
        match self
            .objects
            .validate_configuration(&preparation.target, &preparation.expected_account_id)
            .await
        {
            Ok(_) => {
                self.repository
                    .activate_storage_configuration(context, command, &preparation, tested_at)
                    .await
            }
            Err(error) => {
                let reason = object_error_code(&error);
                self.repository
                    .fail_storage_configuration(context, command, &preparation, tested_at, reason)
                    .await
            }
        }
    }

    async fn copy_between_targets(
        &self,
        preparation: &RestorePreparation,
    ) -> Result<StoredObject, AppError> {
        tokio::fs::create_dir_all(&self.temporary_directory)
            .await
            .map_err(|_| AppError::Internal {
                category: "temporary_storage",
            })?;
        if preparation.size > SINGLE_UPLOAD_MAX_BYTES {
            let plan = MultipartPlan::for_file_size(preparation.size)
                .map_err(|error| map_plan_error(&error))?;
            return copy_large_between_targets(
                Arc::clone(&self.objects),
                &self.temporary_directory,
                preparation,
                plan,
            )
            .await;
        }

        let temporary = tempfile::Builder::new()
            .prefix("briefcase-restore-")
            .tempfile_in(&self.temporary_directory)
            .map_err(|_| AppError::Internal {
                category: "temporary_storage",
            })?;
        let path = temporary.path();
        let metadata = self
            .objects
            .get_to_file(
                &preparation.source_target,
                &preparation.source_key,
                preparation.source_provider_version_id.as_deref(),
                path,
            )
            .await
            .map_err(|error| map_object_error(&error))?;
        if metadata.size != preparation.size {
            return Err(AppError::Conflict {
                code: "version_size_mismatch".into(),
            });
        }
        if metadata.checksum.as_ref() != Some(&preparation.checksum) {
            return Err(AppError::Conflict {
                code: "version_checksum_mismatch".into(),
            });
        }
        let checksum_sha256 = sha256_file(path).await?;
        let expected = ObjectChecksum::new(
            ObjectChecksumAlgorithm::Sha256,
            ObjectChecksumType::FullObject,
            STANDARD.encode(checksum_sha256),
        )
        .map_err(|_| AppError::Internal {
            category: "object_checksum",
        })?;
        if preparation.checksum != expected {
            return Err(AppError::Conflict {
                code: "version_checksum_mismatch".into(),
            });
        }
        let stored = match self
            .objects
            .put_file(
                &preparation.destination_target,
                &preparation.destination_key,
                path,
                &preparation.content_type,
                preparation.size,
                &checksum_sha256,
            )
            .await
        {
            Ok(stored) => stored,
            Err(error) => {
                self.compensating_delete(
                    &preparation.destination_target,
                    &preparation.destination_key,
                    None,
                )
                .await;
                return Err(map_object_error(&error));
            }
        };
        if let Err(error) = validate_restored_object(&stored, preparation, &expected) {
            self.compensating_delete(
                &preparation.destination_target,
                &preparation.destination_key,
                stored.provider_version_id.as_deref(),
            )
            .await;
            return Err(error);
        }
        Ok(stored)
    }

    async fn reconcile_completed_multipart(
        &self,
        preparation: &MultipartCompletionPreparation,
        expected_part_count: usize,
    ) -> Result<StoredObject, AppError> {
        let metadata = self
            .objects
            .head(&preparation.target, &preparation.key, None)
            .await
            .map_err(|error| map_object_error(&error))?;
        if metadata.size != preparation.plan.file_size() {
            return Err(AppError::Conflict {
                code: "multipart_reconciliation_mismatch".into(),
            });
        }
        let checksum = metadata.checksum.ok_or(AppError::Internal {
            category: "object_checksum_unavailable",
        })?;
        let expected_suffix = format!("-{expected_part_count}");
        if checksum.checksum_type() != ObjectChecksumType::Composite
            || !checksum.encoded_value().ends_with(&expected_suffix)
        {
            return Err(AppError::Conflict {
                code: "multipart_reconciliation_mismatch".into(),
            });
        }
        Ok(StoredObject {
            key: preparation.key.clone(),
            etag: metadata.etag,
            provider_version_id: metadata.provider_version_id,
            size: metadata.size,
            checksum: Some(checksum),
        })
    }

    async fn compensating_delete(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_version_id: Option<&str>,
    ) {
        cleanup_unpublished_object(
            self.objects.as_ref(),
            target,
            key,
            None,
            provider_version_id,
        )
        .await;
    }

    async fn release(
        &self,
        context: &ExecutionContext,
        operation: &'static str,
        key: &IdempotencyKey,
        request_hash: &[u8; 32],
    ) {
        if self
            .repository
            .release_operation(context, operation, key, request_hash)
            .await
            .is_err()
        {
            warn!(
                operation,
                "idempotency lease requires expiry-based recovery"
            );
        }
    }
}

async fn copy_large_between_targets<O>(
    objects: Arc<O>,
    temporary_directory: &Path,
    preparation: &RestorePreparation,
    plan: MultipartPlan,
) -> Result<StoredObject, AppError>
where
    O: ObjectStore + ?Sized + 'static,
{
    let source_metadata = objects
        .head(
            &preparation.source_target,
            &preparation.source_key,
            preparation.source_provider_version_id.as_deref(),
        )
        .await
        .map_err(|error| map_object_error(&error))?;
    validate_restore_source(&source_metadata, preparation)?;
    let source_etag = source_metadata.etag.as_deref().ok_or(AppError::Internal {
        category: "object_etag_unavailable",
    })?;
    let upload_id = objects
        .create_multipart(
            &preparation.destination_target,
            &preparation.destination_key,
            &preparation.content_type,
        )
        .await
        .map_err(|error| map_object_error(&error))?;
    let mut cleanup = UnpublishedObjectGuard::multipart(
        Arc::clone(&objects),
        preparation.destination_target.clone(),
        preparation.destination_key.clone(),
        upload_id.clone(),
    );

    let result = transfer_large_restore_parts(
        objects.as_ref(),
        temporary_directory,
        preparation,
        plan,
        source_etag,
        &upload_id,
    )
    .await;
    if result.is_err() {
        cleanup.cleanup_now().await;
    } else {
        cleanup.disarm();
    }
    result
}

async fn transfer_large_restore_parts<O>(
    objects: &O,
    temporary_directory: &Path,
    preparation: &RestorePreparation,
    plan: MultipartPlan,
    source_etag: &str,
    upload_id: &str,
) -> Result<StoredObject, AppError>
where
    O: ObjectStore + ?Sized,
{
    let capacity = usize::try_from(plan.part_count()).map_err(|_| AppError::Internal {
        category: "multipart_plan",
    })?;
    let mut parts = Vec::with_capacity(capacity);
    let mut full_digest = Sha256::new();
    let mut offset = 0_u64;
    let transfer = LargeRestoreTransfer {
        objects,
        temporary_directory,
        preparation,
        source_etag,
        upload_id,
    };

    for part_number in 1..=plan.part_count() {
        let size = plan
            .expected_part_size(part_number)
            .map_err(|_| AppError::Internal {
                category: "multipart_plan",
            })?;
        parts.push(
            transfer
                .transfer_part(part_number, offset, size, &mut full_digest)
                .await?,
        );
        offset = offset.checked_add(size).ok_or(AppError::Internal {
            category: "multipart_plan",
        })?;
    }
    if offset != preparation.size {
        return Err(AppError::Internal {
            category: "multipart_plan",
        });
    }

    let full_checksum = full_object_checksum(full_digest.finalize().into())?;
    let composite_checksum = composite_checksum(&parts)?;
    if !source_checksum_matches(&preparation.checksum, &full_checksum, &composite_checksum) {
        return Err(AppError::Conflict {
            code: "version_checksum_mismatch".into(),
        });
    }

    let stored = transfer
        .complete_or_reconcile(&parts, &composite_checksum)
        .await?;
    validate_restored_object(&stored, preparation, &composite_checksum)?;
    Ok(stored)
}

struct LargeRestoreTransfer<'a, O: ?Sized> {
    objects: &'a O,
    temporary_directory: &'a Path,
    preparation: &'a RestorePreparation,
    source_etag: &'a str,
    upload_id: &'a str,
}

impl<O> LargeRestoreTransfer<'_, O>
where
    O: ObjectStore + ?Sized,
{
    async fn transfer_part(
        &self,
        part_number: u32,
        offset: u64,
        size: u64,
        full_digest: &mut Sha256,
    ) -> Result<StoredPart, AppError> {
        let temporary = tempfile::Builder::new()
            .prefix("briefcase-restore-part-")
            .tempfile_in(self.temporary_directory)
            .map_err(|_| AppError::Internal {
                category: "temporary_storage",
            })?;
        self.objects
            .get_range_to_file(DownloadRangeRequest {
                target: &self.preparation.source_target,
                key: &self.preparation.source_key,
                provider_version_id: self.preparation.source_provider_version_id.as_deref(),
                path: temporary.path(),
                offset,
                size,
                if_match: Some(self.source_etag),
            })
            .await
            .map_err(|error| map_object_error(&error))?;
        let checksum_sha256 = sha256_file_and_update(temporary.path(), full_digest).await?;
        let etag = self
            .objects
            .upload_part(UploadPartRequest {
                target: &self.preparation.destination_target,
                key: &self.preparation.destination_key,
                provider_upload_id: self.upload_id,
                part_number,
                path: temporary.path(),
                // A restore stages each part in its own temporary file.
                offset: 0,
                size,
                checksum_sha256: &checksum_sha256,
            })
            .await
            .map_err(|error| map_object_error(&error))?;
        Ok(StoredPart {
            part_number,
            etag,
            checksum_sha256,
        })
    }

    async fn complete_or_reconcile(
        &self,
        parts: &[StoredPart],
        composite_checksum: &ObjectChecksum,
    ) -> Result<StoredObject, AppError> {
        let completion_error = match self
            .objects
            .complete_multipart(
                &self.preparation.destination_target,
                &self.preparation.destination_key,
                self.upload_id,
                parts,
                self.preparation.size,
            )
            .await
        {
            Ok(stored) => return Ok(stored),
            Err(error) => error,
        };
        let metadata = self
            .objects
            .head(
                &self.preparation.destination_target,
                &self.preparation.destination_key,
                None,
            )
            .await;
        match metadata {
            Ok(metadata)
                if metadata.size == self.preparation.size
                    && metadata.checksum.as_ref() == Some(composite_checksum) =>
            {
                Ok(StoredObject {
                    key: self.preparation.destination_key.clone(),
                    etag: metadata.etag,
                    provider_version_id: metadata.provider_version_id,
                    size: metadata.size,
                    checksum: metadata.checksum,
                })
            }
            Ok(_) | Err(_) => Err(map_object_error(&completion_error)),
        }
    }
}

fn source_checksum_matches(
    expected: &ObjectChecksum,
    full_checksum: &ObjectChecksum,
    composite_checksum: &ObjectChecksum,
) -> bool {
    expected.algorithm() == ObjectChecksumAlgorithm::Sha256
        && match expected.checksum_type() {
            ObjectChecksumType::FullObject => expected == full_checksum,
            ObjectChecksumType::Composite => expected == composite_checksum,
        }
}

async fn cleanup_unpublished_object<O>(
    objects: &O,
    target: &StorageTarget,
    key: &ObjectKey,
    provider_upload_id: Option<&str>,
    provider_version_id: Option<&str>,
) where
    O: ObjectStore + ?Sized,
{
    if let Some(provider_upload_id) = provider_upload_id
        && let Err(error) = objects
            .abort_multipart(target, key, provider_upload_id)
            .await
    {
        warn!(error = %object_error_code(&error), "unpublished multipart session requires cleanup");
    }

    let provider_version_id = match provider_version_id {
        Some(provider_version_id) => Some(provider_version_id.to_owned()),
        None => match objects.head(target, key, None).await {
            Ok(metadata) => metadata.provider_version_id,
            Err(ObjectStoreError::NotFound) => return,
            Err(error) => {
                warn!(
                    error = %object_error_code(&error),
                    "unpublished object version discovery requires reconciliation"
                );
                return;
            }
        },
    };
    if let Err(error) = objects
        .delete(target, key, provider_version_id.as_deref())
        .await
    {
        warn!(error = %object_error_code(&error), "unpublished object requires cleanup");
    }
}

fn validate_restore_source(
    metadata: &ObjectMetadata,
    preparation: &RestorePreparation,
) -> Result<(), AppError> {
    if metadata.size != preparation.size {
        return Err(AppError::Conflict {
            code: "version_size_mismatch".into(),
        });
    }
    if metadata.checksum.as_ref() != Some(&preparation.checksum) {
        return Err(AppError::Conflict {
            code: "version_checksum_mismatch".into(),
        });
    }
    Ok(())
}

fn validate_restored_object(
    stored: &StoredObject,
    preparation: &RestorePreparation,
    expected_checksum: &ObjectChecksum,
) -> Result<(), AppError> {
    if stored.key != preparation.destination_key || stored.size != preparation.size {
        return Err(AppError::Conflict {
            code: "restored_object_mismatch".into(),
        });
    }
    if stored.checksum.as_ref() != Some(expected_checksum) {
        return Err(AppError::Conflict {
            code: "restored_object_checksum_mismatch".into(),
        });
    }
    Ok(())
}

fn full_object_checksum(digest: [u8; 32]) -> Result<ObjectChecksum, AppError> {
    ObjectChecksum::new(
        ObjectChecksumAlgorithm::Sha256,
        ObjectChecksumType::FullObject,
        STANDARD.encode(digest),
    )
    .map_err(|_| AppError::Internal {
        category: "object_checksum",
    })
}

fn composite_checksum(parts: &[StoredPart]) -> Result<ObjectChecksum, AppError> {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.checksum_sha256);
    }
    let value = format!("{}-{}", STANDARD.encode(digest.finalize()), parts.len());
    ObjectChecksum::new(
        ObjectChecksumAlgorithm::Sha256,
        ObjectChecksumType::Composite,
        value,
    )
    .map_err(|_| AppError::Internal {
        category: "object_checksum",
    })
}

async fn sha256_file_and_update(
    path: &Path,
    full_digest: &mut Sha256,
) -> Result<[u8; 32], AppError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| AppError::Internal {
            category: "temporary_storage",
        })?;
    let mut part_digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|_| AppError::Internal {
                category: "temporary_storage",
            })?;
        if read == 0 {
            break;
        }
        part_digest.update(&buffer[..read]);
        full_digest.update(&buffer[..read]);
    }
    Ok(part_digest.finalize().into())
}

/// Digests exactly one byte range of a staged file.
///
/// The provider verifies a per-part checksum, and each part is a range of the
/// single staged upload, so the digest is taken over that range alone.
async fn sha256_file_range(path: &Path, offset: u64, length: u64) -> Result<[u8; 32], AppError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| AppError::Internal {
            category: "temporary_storage",
        })?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|_| AppError::Internal {
            category: "temporary_storage",
        })?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut remaining = length;
    while remaining > 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        let read = file
            .read(&mut buffer[..wanted])
            .await
            .map_err(|_| AppError::Internal {
                category: "temporary_storage",
            })?;
        if read == 0 {
            return Err(AppError::Internal {
                category: "staged_upload_truncated",
            });
        }
        digest.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(digest.finalize().into())
}

async fn sha256_file(path: &Path) -> Result<[u8; 32], AppError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| AppError::Internal {
            category: "temporary_storage",
        })?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|_| AppError::Internal {
                category: "temporary_storage",
            })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().into())
}

/// Resolves a client range request against the bytes the file actually has.
///
/// A range that starts at or past the end is unsatisfiable, which is how a
/// media player learns the real size; an open-ended request is truncated to
/// the final byte, and a suffix request is anchored to it.
fn resolve_range(request: RangeRequest, total_size: u64) -> Result<ByteRange, AppError> {
    if total_size == 0 {
        return Err(AppError::RangeNotSatisfiable { total_size });
    }
    let last_byte = total_size.saturating_sub(1);
    let range = match request {
        RangeRequest::From(start) => ByteRange {
            start,
            end: last_byte,
        },
        RangeRequest::Between(start, end) => ByteRange {
            start,
            end: end.min(last_byte),
        },
        RangeRequest::Last(0) => return Err(AppError::RangeNotSatisfiable { total_size }),
        RangeRequest::Last(length) => ByteRange {
            start: total_size.saturating_sub(length),
            end: last_byte,
        },
    };
    if range.start > range.end || range.start >= total_size {
        return Err(AppError::RangeNotSatisfiable { total_size });
    }
    Ok(range)
}

fn map_plan_error(error: &MultipartPlanError) -> AppError {
    match error {
        MultipartPlanError::FileTooLarge { .. } => AppError::PayloadTooLarge,
        MultipartPlanError::MultipartNotRequired { .. }
        | MultipartPlanError::ArithmeticOverflow
        | MultipartPlanError::TooManyParts { .. } => AppError::validation("invalid_multipart_size"),
    }
}

fn map_object_error(error: &ObjectStoreError) -> AppError {
    match error {
        ObjectStoreError::NotFound => AppError::NotFound,
        ObjectStoreError::Conflict => AppError::conflict("storage_conflict"),
        ObjectStoreError::InvalidConfiguration => {
            AppError::validation("invalid_storage_configuration")
        }
        ObjectStoreError::Unavailable => AppError::DependencyUnavailable {
            dependency: "object_storage",
        },
        ObjectStoreError::Internal(_) => AppError::Internal {
            category: "object_storage",
        },
    }
}

const fn object_error_code(error: &ObjectStoreError) -> &'static str {
    match error {
        ObjectStoreError::NotFound => "storage_not_found",
        ObjectStoreError::Conflict => "storage_conflict",
        ObjectStoreError::InvalidConfiguration => "storage_validation_failed",
        ObjectStoreError::Unavailable => "storage_unavailable",
        ObjectStoreError::Internal(_) => "storage_internal",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        time::Duration,
    };

    use tokio::sync::Notify;

    use super::*;
    use crate::{application::ports::OpenObject, domain::multipart::MULTIPART_MIN_FILE_BYTES};

    struct FailingRangeStore {
        source_size: u64,
        source_checksum: ObjectChecksum,
        range_was_conditional: AtomicBool,
        source_version_was_bound: AtomicBool,
        block_range: bool,
        range_started: Notify,
        aborts: AtomicUsize,
        deletes: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ObjectStore for FailingRangeStore {
        async fn put_file(
            &self,
            _target: &StorageTarget,
            _key: &ObjectKey,
            _path: &Path,
            _content_type: &str,
            _size: u64,
            _checksum_sha256: &[u8; 32],
        ) -> Result<StoredObject, ObjectStoreError> {
            Err(ObjectStoreError::Unavailable)
        }

        async fn get_to_file(
            &self,
            _target: &StorageTarget,
            _key: &ObjectKey,
            _provider_version_id: Option<&str>,
            _path: &Path,
        ) -> Result<ObjectMetadata, ObjectStoreError> {
            Err(ObjectStoreError::Unavailable)
        }

        async fn get_range_to_file(
            &self,
            request: DownloadRangeRequest<'_>,
        ) -> Result<(), ObjectStoreError> {
            self.range_was_conditional
                .store(request.if_match == Some("source-etag"), Ordering::Relaxed);
            self.source_version_was_bound.store(
                request.provider_version_id == Some("source-version"),
                Ordering::Relaxed,
            );
            if self.block_range {
                self.range_started.notify_one();
                return std::future::pending().await;
            }
            Err(ObjectStoreError::Unavailable)
        }

        async fn head(
            &self,
            _target: &StorageTarget,
            _key: &ObjectKey,
            provider_version_id: Option<&str>,
        ) -> Result<ObjectMetadata, ObjectStoreError> {
            if provider_version_id.is_some() {
                self.source_version_was_bound.store(
                    provider_version_id == Some("source-version"),
                    Ordering::Relaxed,
                );
            }
            Ok(ObjectMetadata {
                size: self.source_size,
                etag: Some("source-etag".to_owned()),
                provider_version_id: Some("source-version".to_owned()),
                checksum: Some(self.source_checksum.clone()),
            })
        }

        async fn copy(
            &self,
            _request: CopyObjectRequest<'_>,
        ) -> Result<StoredObject, ObjectStoreError> {
            Err(ObjectStoreError::Unavailable)
        }

        async fn delete(
            &self,
            _target: &StorageTarget,
            _key: &ObjectKey,
            _provider_version_id: Option<&str>,
        ) -> Result<(), ObjectStoreError> {
            self.deletes.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn create_multipart(
            &self,
            _target: &StorageTarget,
            _key: &ObjectKey,
            _content_type: &str,
        ) -> Result<String, ObjectStoreError> {
            Ok("destination-upload".to_owned())
        }

        async fn upload_part(
            &self,
            _request: UploadPartRequest<'_>,
        ) -> Result<String, ObjectStoreError> {
            Err(ObjectStoreError::Unavailable)
        }

        async fn complete_multipart(
            &self,
            _target: &StorageTarget,
            _key: &ObjectKey,
            _provider_upload_id: &str,
            _parts: &[StoredPart],
            _expected_size: u64,
        ) -> Result<StoredObject, ObjectStoreError> {
            Err(ObjectStoreError::Unavailable)
        }

        async fn abort_multipart(
            &self,
            _target: &StorageTarget,
            _key: &ObjectKey,
            _provider_upload_id: &str,
        ) -> Result<(), ObjectStoreError> {
            self.aborts.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn open_object(
            &self,
            _request: OpenObjectRequest<'_>,
        ) -> Result<OpenObject, ObjectStoreError> {
            Err(ObjectStoreError::Unavailable)
        }

        async fn validate_configuration(
            &self,
            _target: &StorageTarget,
            _expected_account_id: &str,
        ) -> Result<crate::application::ports::StorageValidation, ObjectStoreError> {
            Err(ObjectStoreError::Unavailable)
        }
    }

    #[test]
    fn locally_calculated_checksums_verify_both_source_checksum_types() -> anyhow::Result<()> {
        let first: [u8; 32] = Sha256::digest(b"first").into();
        let second: [u8; 32] = Sha256::digest(b"second").into();
        let parts = vec![
            StoredPart {
                part_number: 1,
                etag: "first".to_owned(),
                checksum_sha256: first,
            },
            StoredPart {
                part_number: 2,
                etag: "second".to_owned(),
                checksum_sha256: second,
            },
        ];
        let full = full_object_checksum(Sha256::digest(b"firstsecond").into())?;
        let composite = composite_checksum(&parts)?;

        assert!(source_checksum_matches(&full, &full, &composite));
        assert!(source_checksum_matches(&composite, &full, &composite));
        assert_ne!(full.encoded_value(), composite.encoded_value());

        let reversed = composite_checksum(&[parts[1].clone(), parts[0].clone()])?;
        assert!(!source_checksum_matches(&reversed, &full, &composite));
        Ok(())
    }

    #[test]
    fn restore_lease_tolerates_multiple_missed_heartbeats() {
        assert!(
            RESTORE_LEASE_DURATION.as_secs()
                >= RESTORE_LEASE_RENEWAL_INTERVAL.as_secs().saturating_mul(3)
        );
    }

    #[test]
    fn only_an_unknown_commit_outcome_suppresses_provider_compensation() {
        let unknown = AppError::DatabaseCommitOutcomeUnknown {
            operation: "restore_version",
        };
        let definite = AppError::DependencyUnavailable {
            dependency: "postgresql",
        };

        assert_eq!(
            publication_failure_disposition(&unknown),
            PublicationFailureDisposition::PreserveProviderState
        );
        assert_eq!(
            publication_failure_disposition(&definite),
            PublicationFailureDisposition::CompensateProviderState
        );
    }

    #[tokio::test]
    async fn failed_large_range_download_aborts_and_deletes_destination() -> anyhow::Result<()> {
        let size = MULTIPART_MIN_FILE_BYTES;
        let plan = MultipartPlan::for_file_size(size)?;
        let source_checksum = ObjectChecksum::new(
            ObjectChecksumAlgorithm::Sha256,
            ObjectChecksumType::Composite,
            format!("{}-{}", STANDARD.encode([7_u8; 32]), plan.part_count()),
        )?;
        let store = Arc::new(FailingRangeStore {
            source_size: size,
            source_checksum: source_checksum.clone(),
            range_was_conditional: AtomicBool::new(false),
            source_version_was_bound: AtomicBool::new(false),
            block_range: false,
            range_started: Notify::new(),
            aborts: AtomicUsize::new(0),
            deletes: AtomicUsize::new(0),
        });
        let preparation = RestorePreparation {
            entry_id: EntryId::new(),
            new_version_id: VersionId::new(),
            source_target: test_target("source"),
            source_key: ObjectKey::new("entries/source/versions/one")?,
            source_provider_version_id: Some("source-version".to_owned()),
            destination_target: test_target("destination"),
            destination_key: ObjectKey::new("entries/destination/versions/two")?,
            content_type: "application/octet-stream".to_owned(),
            size,
            checksum: source_checksum,
        };
        let directory = tempfile::tempdir()?;

        let result =
            copy_large_between_targets(Arc::clone(&store), directory.path(), &preparation, plan)
                .await;

        assert!(result.is_err());
        assert!(store.range_was_conditional.load(Ordering::Relaxed));
        assert!(store.source_version_was_bound.load(Ordering::Relaxed));
        assert_eq!(store.aborts.load(Ordering::Relaxed), 1);
        assert_eq!(store.deletes.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_large_restore_aborts_and_deletes_destination() -> anyhow::Result<()> {
        let size = MULTIPART_MIN_FILE_BYTES;
        let plan = MultipartPlan::for_file_size(size)?;
        let source_checksum = ObjectChecksum::new(
            ObjectChecksumAlgorithm::Sha256,
            ObjectChecksumType::Composite,
            format!("{}-{}", STANDARD.encode([7_u8; 32]), plan.part_count()),
        )?;
        let store = Arc::new(FailingRangeStore {
            source_size: size,
            source_checksum: source_checksum.clone(),
            range_was_conditional: AtomicBool::new(false),
            source_version_was_bound: AtomicBool::new(false),
            block_range: true,
            range_started: Notify::new(),
            aborts: AtomicUsize::new(0),
            deletes: AtomicUsize::new(0),
        });
        let preparation = RestorePreparation {
            entry_id: EntryId::new(),
            new_version_id: VersionId::new(),
            source_target: test_target("source"),
            source_key: ObjectKey::new("entries/source/versions/one")?,
            source_provider_version_id: Some("source-version".to_owned()),
            destination_target: test_target("destination"),
            destination_key: ObjectKey::new("entries/destination/versions/two")?,
            content_type: "application/octet-stream".to_owned(),
            size,
            checksum: source_checksum,
        };
        let directory = tempfile::tempdir()?;
        let directory_path = directory.path().to_owned();
        let operation = tokio::spawn({
            let store = Arc::clone(&store);
            async move { copy_large_between_targets(store, &directory_path, &preparation, plan).await }
        });

        store.range_started.notified().await;
        operation.abort();
        let Err(cancellation) = operation.await else {
            anyhow::bail!("operation unexpectedly completed");
        };
        assert!(cancellation.is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), async {
            while store.aborts.load(Ordering::Relaxed) == 0
                || store.deletes.load(Ordering::Relaxed) == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await?;

        assert!(store.range_was_conditional.load(Ordering::Relaxed));
        assert!(store.source_version_was_bound.load(Ordering::Relaxed));
        assert_eq!(store.aborts.load(Ordering::Relaxed), 1);
        assert_eq!(store.deletes.load(Ordering::Relaxed), 1);
        Ok(())
    }

    fn test_target(bucket: &str) -> StorageTarget {
        StorageTarget {
            bucket: bucket.to_owned(),
            region: "us-east-1".to_owned(),
            prefix: "tenant".to_owned(),
            role_arn: None,
            external_id: None,
            encryption: EncryptionMode::SseS3,
            kms_key_arn: None,
        }
    }
}
