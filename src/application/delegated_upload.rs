//! Fresh-authorized publication of capability-scoped, durable staged uploads.
//!
//! A staging capability can write only one reserved byte stream. It is not an
//! IAM authorization snapshot and cannot read files or publish an entry.

use std::{future::Future, path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use secrecy::{ExposeSecret as _, SecretString};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    application::{
        content::{map_object_error, sha256_file_range},
        context::{ExecutionContext, TestingEnvironmentContext},
        ports::{
            ObjectChecksum, ObjectChecksumAlgorithm, ObjectChecksumType, ObjectKey, ObjectStore,
            ObjectStoreError, StorageTarget, StoredObject, StoredPart, UploadPartRequest,
        },
    },
    domain::{
        actor::OrganizationId,
        entry::EntryName,
        ids::EntryId,
        multipart::{MultipartPlan, UploadStrategy},
    },
    error::AppError,
};

/// Maximum lifetime of a new logical staging reservation.
pub const DELEGATED_UPLOAD_TTL_SECONDS: i64 = 24 * 60 * 60;

/// Public state of a single logical upload operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegatedUploadState {
    /// No provider write has begun; fresh reserve may rotate the capability.
    Reserved,
    /// One byte-transfer lease owns this reservation.
    Receiving,
    /// The immutable provider object is verified and awaiting fresh commit.
    Staged,
    /// Publication and quota charge completed atomically.
    Committed,
    /// Cancellation completed, with no remaining possible provider bytes.
    Cancelled,
    /// Expiration completed, with no remaining possible provider bytes.
    Expired,
    /// Possible provider bytes remain quarantined until cleanup is proven.
    CleanupPending,
}

/// Secret-free receipt returned by status, transfer, commit and cancellation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DelegatedUploadStatus {
    /// Stable caller-generated UUID for the frozen logical intent.
    pub operation_id: Uuid,
    /// Server-generated identifier for the single provider-write attempt.
    pub upload_id: Uuid,
    /// Current durable state.
    pub state: DelegatedUploadState,
    /// Original expiration, never extended by a retry.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    /// The actual new or versioned entry, only after committed publication.
    pub published_entry_id: Option<Uuid>,
}

/// A reserve receipt whose capability may be returned only to fresh authority.
#[derive(Clone, Debug)]
pub struct DelegatedUploadReservation {
    /// Secret-free durable state.
    pub status: DelegatedUploadStatus,
    /// Newly issued capability, present only for an idle reserved operation.
    pub capability: Option<SecretString>,
}

/// Validated immutable manifest for a reservation.
#[derive(Clone, Debug)]
pub struct ReserveDelegatedUpload {
    /// Stable logical-operation key.
    pub operation_id: Uuid,
    /// User-supplied path bound inside the IAM-authorized manifest.
    pub parent_path: String,
    /// Resolved destination; persistence freezes the first accepted UUID.
    pub parent_id: EntryId,
    /// Validated one-segment file name.
    pub name: EntryName,
    /// Validated media type.
    pub content_type: String,
    /// Exact expected byte count.
    pub size: u64,
    /// SHA-256 of the complete raw body, not a multipart composite checksum.
    pub sha256: [u8; 32],
    /// Canonical hash of the complete caller-visible manifest.
    pub request_hash: [u8; 32],
}

/// A non-IAM selector for the capability-only transfer path.
///
/// The organization is untrusted until the capability is checked against its
/// row. The optional test selector is authenticated separately with its root.
#[derive(Clone, Debug)]
pub struct DelegatedUploadScope {
    /// Public organization selector from x-org-id.
    pub organization_id: OrganizationId,
    /// Root-authenticated test-plane generation, or production.
    pub testing_environment: Option<TestingEnvironmentContext>,
    /// Correlation only; never used as authority.
    pub request_id: String,
}

/// Frozen target and exact lease for one transfer, without cached IAM rights.
#[derive(Clone, Debug)]
pub struct DelegatedUploadTransfer {
    /// Capability-selected tenant and test generation.
    pub scope: DelegatedUploadScope,
    /// Public logical-operation key.
    pub operation_id: Uuid,
    /// Single provider-attempt identifier.
    pub upload_id: Uuid,
    /// Opaque compare-and-set writer lease token.
    pub lease_token: Uuid,
    /// Server-recorded deadline after which external mutations must stop.
    pub lease_expires_at: OffsetDateTime,
    /// Exact frozen destination, including BYO role/encryption snapshot.
    pub target: StorageTarget,
    /// One-write immutable provider key.
    pub key: ObjectKey,
    /// Media type frozen in the authorized manifest.
    pub content_type: String,
    /// Exact expected raw bytes.
    pub size: u64,
    /// Expected full-stream SHA-256.
    pub sha256: [u8; 32],
}

/// Immutable stored-object evidence required before fresh publication.
#[derive(Clone, Debug)]
pub struct DelegatedUploadCommit {
    /// Status observed under current IAM authority.
    pub status: DelegatedUploadStatus,
    /// Frozen storage destination.
    pub target: StorageTarget,
    /// Provider-confirmed exact key, version, size and checksum.
    pub stored: StoredObject,
}

/// A committed replay never needs another provider operation.
#[derive(Clone, Debug)]
pub enum PrepareDelegatedUploadCommit {
    /// Verify the already-staged provider object before final publication.
    Staged(Box<DelegatedUploadCommit>),
    /// The original publication already completed; current authority passed.
    Committed(DelegatedUploadStatus),
}

/// Persistence contract for one-write staging and fresh-authorized commit.
#[async_trait]
pub trait DelegatedUploadRepository: Send + Sync {
    /// Freezes intent/identity/target and atomically reserves staging budget.
    /// A retry may replace the capability hash only in idle reserved state.
    async fn reserve(
        &self,
        context: &ExecutionContext,
        command: &ReserveDelegatedUpload,
        capability_hash: &[u8; 32],
        expires_at: OffsetDateTime,
    ) -> Result<DelegatedUploadStatus, AppError>;

    /// Reads one operation only after fresh current authority is established.
    async fn status(
        &self,
        context: &ExecutionContext,
        operation_id: Uuid,
    ) -> Result<DelegatedUploadStatus, AppError>;

    /// Atomically consumes a capability into one bounded receiving lease.
    async fn claim_transfer(
        &self,
        scope: &DelegatedUploadScope,
        upload_id: Uuid,
        capability_hash: &[u8; 32],
        lease_seconds: i64,
    ) -> Result<DelegatedUploadTransfer, AppError>;

    /// CAS-validates state, lease and test generation before EACH mutation.
    /// The first call durably marks that provider bytes may now exist.
    async fn before_provider_write(
        &self,
        transfer: &DelegatedUploadTransfer,
    ) -> Result<(), AppError>;

    /// The same writer CAS, additionally marking possible object publication.
    /// Used before single PUT and multipart completion, not before part writes.
    async fn before_provider_completion(
        &self,
        transfer: &DelegatedUploadTransfer,
    ) -> Result<(), AppError>;

    /// Records the exact created multipart ID before any parts are uploaded.
    async fn record_multipart(
        &self,
        transfer: &DelegatedUploadTransfer,
        provider_upload_id: &str,
    ) -> Result<(), AppError>;

    /// Freezes verified object evidence without publishing any entry.
    /// An uncertain transaction commit must return `DatabaseCommitOutcomeUnknown`.
    async fn complete_transfer(
        &self,
        transfer: &DelegatedUploadTransfer,
        stored: &StoredObject,
    ) -> Result<DelegatedUploadStatus, AppError>;

    /// Fences a failed/cancelled writer. Pre-provider failures alone may reset;
    /// after any attempted write the row/budget survives for durable cleanup.
    /// Supplied provider IDs improve cleanup after a lost DB acknowledgement.
    async fn fail_transfer(
        &self,
        transfer: &DelegatedUploadTransfer,
        provider_upload_id: Option<&str>,
        stored: Option<&StoredObject>,
    ) -> Result<(), AppError>;

    /// Requires fresh matching immutable identity and current destination rights.
    async fn prepare_commit(
        &self,
        context: &ExecutionContext,
        operation_id: Uuid,
        upload_id: Uuid,
    ) -> Result<PrepareDelegatedUploadCommit, AppError>;

    /// Rechecks identity/rights/generation under lock, publishes the existing
    /// object and charges quota exactly once in the same transaction.
    async fn commit(
        &self,
        context: &ExecutionContext,
        operation_id: Uuid,
        upload_id: Uuid,
        stored: &StoredObject,
    ) -> Result<DelegatedUploadStatus, AppError>;

    /// Cancels under fresh authority; no possible bytes are discarded as state.
    async fn cancel(
        &self,
        context: &ExecutionContext,
        operation_id: Uuid,
    ) -> Result<DelegatedUploadStatus, AppError>;
}

/// Coordinates capability-only staging and separately authorized publication.
pub struct DelegatedUploadService<R: ?Sized, O: ?Sized> {
    repository: Arc<R>,
    objects: Arc<O>,
    lease_seconds: i64,
    provider_timeout: Duration,
}

/// Cancellation-safe ownership of one receiving lease.
///
/// Dropping this value records failure, never deletes provider bytes directly.
/// Durable state is the arbiter if a database commit acknowledgement is lost.
pub struct DelegatedUploadLease<R: DelegatedUploadRepository + ?Sized + 'static> {
    repository: Arc<R>,
    transfer: DelegatedUploadTransfer,
    provider_upload_id: Option<String>,
    stored: Option<StoredObject>,
    armed: bool,
}

impl<R: DelegatedUploadRepository + ?Sized + 'static> DelegatedUploadLease<R> {
    /// Exact declared size, checked before accepting any provider bytes.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.transfer.size
    }

    /// Full-object manifest digest, independent of provider checksum format.
    #[must_use]
    pub const fn sha256(&self) -> [u8; 32] {
        self.transfer.sha256
    }
}

impl<R: DelegatedUploadRepository + ?Sized + 'static> Drop for DelegatedUploadLease<R> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let repository = Arc::clone(&self.repository);
        let transfer = self.transfer.clone();
        let provider_upload_id = self.provider_upload_id.take();
        let stored = self.stored.take();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = repository.fail_transfer(&transfer, provider_upload_id.as_deref(), stored.as_ref()).await {
                    tracing::warn!(upload_id = %transfer.upload_id, error = %error, "delegated upload failure remains for lease recovery");
                }
            });
        }
    }
}

impl<R, O> DelegatedUploadService<R, O>
where
    R: DelegatedUploadRepository + ?Sized + 'static,
    O: ObjectStore + ?Sized + 'static,
{
    /// Creates a service with a lease covering HTTP and provider deadlines.
    #[must_use]
    pub fn new(
        repository: Arc<R>,
        objects: Arc<O>,
        lease_seconds: i64,
        provider_timeout: Duration,
    ) -> Self {
        Self {
            repository,
            objects,
            lease_seconds,
            provider_timeout,
        }
    }

    /// Reserves an immutable manifest with fresh current IAM authority.
    ///
    /// # Errors
    /// Returns an authorization, quota, intent-conflict, or persistence error.
    pub async fn reserve(
        &self,
        context: &ExecutionContext,
        command: &ReserveDelegatedUpload,
    ) -> Result<DelegatedUploadReservation, AppError> {
        UploadStrategy::for_file_size(command.size).map_err(|_| AppError::PayloadTooLarge)?;
        // Two independent OS-random UUIDs retain 244 bits of capability entropy.
        let capability = SecretString::from(format!(
            "{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        ));
        let hash = capability_hash(capability.expose_secret())?;
        let expires_at =
            OffsetDateTime::now_utc() + time::Duration::seconds(DELEGATED_UPLOAD_TTL_SECONDS);
        let status = self
            .repository
            .reserve(context, command, &hash, expires_at)
            .await?;
        let capability = (status.state == DelegatedUploadState::Reserved).then_some(capability);
        Ok(DelegatedUploadReservation { status, capability })
    }

    /// Returns current status after fresh IAM authority and immutable binding.
    ///
    /// # Errors
    /// Returns an error if current authority, identity, or persistence fails.
    pub async fn status(
        &self,
        context: &ExecutionContext,
        operation_id: Uuid,
    ) -> Result<DelegatedUploadStatus, AppError> {
        self.repository.status(context, operation_id).await
    }

    /// Cancels without claiming unknown provider writes have been removed.
    ///
    /// # Errors
    /// Returns an error if current authority, identity, or persistence fails.
    pub async fn cancel(
        &self,
        context: &ExecutionContext,
        operation_id: Uuid,
    ) -> Result<DelegatedUploadStatus, AppError> {
        self.repository.cancel(context, operation_id).await
    }

    /// Consumes a narrow capability before streaming an untrusted request body.
    ///
    /// # Errors
    /// Returns an error for an invalid grant, stale scope, or unavailable lease.
    pub async fn claim_transfer(
        &self,
        scope: &DelegatedUploadScope,
        upload_id: Uuid,
        capability: &SecretString,
    ) -> Result<DelegatedUploadLease<R>, AppError> {
        let hash = capability_hash(capability.expose_secret())?;
        let transfer = self
            .repository
            .claim_transfer(scope, upload_id, &hash, self.lease_seconds)
            .await?;
        Ok(DelegatedUploadLease {
            repository: Arc::clone(&self.repository),
            transfer,
            provider_upload_id: None,
            stored: None,
            armed: true,
        })
    }

    /// Stores one verified private tempfile under its one-write provider key.
    ///
    /// No cached IAM proof or authorization snapshot participates in this path.
    ///
    /// # Errors
    /// Returns an error on integrity, lease, provider, or persistence failure.
    /// A caller must query fresh status before deciding whether to retry.
    pub async fn store(
        &self,
        mut lease: DelegatedUploadLease<R>,
        path: &Path,
        size: u64,
        sha256: [u8; 32],
    ) -> Result<DelegatedUploadStatus, AppError> {
        if size != lease.transfer.size || sha256 != lease.transfer.sha256 {
            return Err(AppError::validation("delegated_upload_manifest_mismatch"));
        }
        let stored = match UploadStrategy::for_file_size(size)
            .map_err(|_| AppError::PayloadTooLarge)?
        {
            UploadStrategy::SingleRequest => self.store_single(&lease.transfer, path).await?,
            UploadStrategy::Multipart(plan) => self.store_multipart(&mut lease, path, plan).await?,
        };
        lease.stored = Some(stored.clone());
        let result = self
            .repository
            .complete_transfer(&lease.transfer, &stored)
            .await;
        if result.is_ok() || matches!(&result, Err(AppError::DatabaseCommitOutcomeUnknown { .. })) {
            // A lost commit acknowledgement must be resolved through fresh
            // status; compensation could otherwise delete a staged object.
            lease.armed = false;
        }
        result
    }

    async fn store_multipart(
        &self,
        lease: &mut DelegatedUploadLease<R>,
        path: &Path,
        plan: MultipartPlan,
    ) -> Result<StoredObject, AppError> {
        // Compute all part checksums before the first provider mutation.
        let (mut parts, expected) = prepare_parts(path, plan).await?;
        let provider_id = self
            .provider_mutation(
                self.repository.before_provider_write(&lease.transfer),
                || {
                    self.objects.create_multipart(
                        &lease.transfer.target,
                        &lease.transfer.key,
                        &lease.transfer.content_type,
                    )
                },
            )
            .await?
            .map_err(|error| map_object_error(&error))?;
        lease.provider_upload_id = Some(provider_id.clone());
        self.repository
            .record_multipart(&lease.transfer, &provider_id)
            .await?;
        for part in &mut parts {
            let part_size =
                plan.expected_part_size(part.part_number)
                    .map_err(|_| AppError::Internal {
                        category: "delegated_upload_plan",
                    })?;
            part.etag = self
                .provider_mutation(
                    self.repository.before_provider_write(&lease.transfer),
                    || {
                        self.objects.upload_part(UploadPartRequest {
                            target: &lease.transfer.target,
                            key: &lease.transfer.key,
                            provider_upload_id: &provider_id,
                            part_number: part.part_number,
                            path,
                            offset: u64::from(part.part_number - 1) * plan.part_size(),
                            size: part_size,
                            checksum_sha256: &part.checksum_sha256,
                        })
                    },
                )
                .await?
                .map_err(|error| map_object_error(&error))?;
        }
        let result = self
            .provider_mutation(
                self.repository.before_provider_completion(&lease.transfer),
                || {
                    self.objects.complete_multipart(
                        &lease.transfer.target,
                        &lease.transfer.key,
                        &provider_id,
                        &parts,
                        plan.file_size(),
                    )
                },
            )
            .await?;
        self.reconcile_write(&lease.transfer, result, &expected)
            .await
    }

    async fn store_single(
        &self,
        transfer: &DelegatedUploadTransfer,
        path: &Path,
    ) -> Result<StoredObject, AppError> {
        let expected = checksum(
            ObjectChecksumType::FullObject,
            STANDARD.encode(transfer.sha256),
        )?;
        let result = self
            .provider_mutation(self.repository.before_provider_completion(transfer), || {
                self.objects.put_file(
                    &transfer.target,
                    &transfer.key,
                    path,
                    &transfer.content_type,
                    transfer.size,
                    &transfer.sha256,
                )
            })
            .await?;
        self.reconcile_write(transfer, result, &expected).await
    }

    async fn reconcile_write(
        &self,
        transfer: &DelegatedUploadTransfer,
        result: Result<StoredObject, ObjectStoreError>,
        expected: &ObjectChecksum,
    ) -> Result<StoredObject, AppError> {
        let stored = match result {
            Ok(stored) => stored,
            Err(original) => {
                // Positive exact evidence can recover a lost provider reply.
                // Absence is deliberately not evidence of successful cleanup.
                let metadata = self
                    .objects
                    .head(&transfer.target, &transfer.key, None)
                    .await
                    .map_err(|_| map_object_error(&original))?;
                StoredObject {
                    key: transfer.key.clone(),
                    etag: metadata.etag,
                    provider_version_id: metadata.provider_version_id,
                    size: metadata.size,
                    checksum: metadata.checksum,
                }
            }
        };
        if stored.key != transfer.key
            || stored.size != transfer.size
            || stored.checksum.as_ref() != Some(expected)
        {
            return Err(AppError::DependencyUnavailable {
                dependency: "object_storage_integrity",
            });
        }
        Ok(stored)
    }

    async fn provider_mutation<T, F>(
        &self,
        validation: impl Future<Output = Result<(), AppError>>,
        mutation: impl FnOnce() -> F,
    ) -> Result<Result<T, ObjectStoreError>, AppError>
    where
        F: Future<Output = Result<T, ObjectStoreError>>,
    {
        // Start the common budget BEFORE the writer CAS: slow acknowledgement
        // cannot move the port call's deadline beyond provider_deadline_at.
        // The factory is invoked only after CAS succeeds, so even construction
        // of the external mutation is deferred until current ownership passes.
        let mut provider_started = false;
        let result = tokio::time::timeout(self.provider_timeout, async {
            validation.await?;
            provider_started = true;
            Ok(mutation().await)
        })
        .await;
        match result {
            Ok(result) => result,
            // The armed guard retains possible provider bytes on timeout;
            // positive HEAD evidence may still reconcile a lost write reply.
            Err(_) if provider_started => Ok(Err(ObjectStoreError::Unavailable)),
            // Preserve CAS/application errors instead of calling them storage
            // failures. A timeout during CAS cannot invoke the provider.
            Err(_) => Err(AppError::Timeout),
        }
    }

    /// Publishes the exact staged object only after a new IAM verification.
    ///
    /// # Errors
    /// Returns an error if current authority, immutable object evidence, quota,
    /// or the final transaction fails. Unknown commits require a status lookup.
    pub async fn commit(
        &self,
        context: &ExecutionContext,
        operation_id: Uuid,
        upload_id: Uuid,
    ) -> Result<DelegatedUploadStatus, AppError> {
        let preparation = match self
            .repository
            .prepare_commit(context, operation_id, upload_id)
            .await?
        {
            PrepareDelegatedUploadCommit::Committed(status) => return Ok(status),
            PrepareDelegatedUploadCommit::Staged(preparation) => preparation,
        };
        let metadata = self
            .objects
            .head(
                &preparation.target,
                &preparation.stored.key,
                preparation.stored.provider_version_id.as_deref(),
            )
            .await
            .map_err(|error| map_object_error(&error))?;
        if metadata.size != preparation.stored.size
            || metadata.checksum != preparation.stored.checksum
            || metadata.provider_version_id != preparation.stored.provider_version_id
            || metadata.checksum.is_none()
        {
            return Err(AppError::DependencyUnavailable {
                dependency: "object_storage_integrity",
            });
        }
        self.repository
            .commit(context, operation_id, upload_id, &preparation.stored)
            .await
    }
}

fn capability_hash(value: &str) -> Result<[u8; 32], AppError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppError::Unauthenticated);
    }
    let mut hash = Sha256::new();
    hash.update(b"briefcase-upload-capability-v1:");
    hash.update(value.as_bytes());
    Ok(hash.finalize().into())
}

async fn prepare_parts(
    path: &Path,
    plan: MultipartPlan,
) -> Result<(Vec<StoredPart>, ObjectChecksum), AppError> {
    let mut parts = Vec::with_capacity(plan.part_count() as usize);
    let mut composite = Sha256::new();
    for part_number in 1..=plan.part_count() {
        let offset = u64::from(part_number - 1) * plan.part_size();
        let size = plan
            .expected_part_size(part_number)
            .map_err(|_| AppError::Internal {
                category: "delegated_upload_plan",
            })?;
        let checksum = sha256_file_range(path, offset, size).await?;
        composite.update(checksum);
        parts.push(StoredPart {
            part_number,
            etag: String::new(),
            checksum_sha256: checksum,
        });
    }
    let expected = checksum(
        ObjectChecksumType::Composite,
        format!(
            "{}-{}",
            STANDARD.encode(composite.finalize()),
            plan.part_count()
        ),
    )?;
    Ok((parts, expected))
}

fn checksum(kind: ObjectChecksumType, encoded: String) -> Result<ObjectChecksum, AppError> {
    ObjectChecksum::new(ObjectChecksumAlgorithm::Sha256, kind, encoded).map_err(|_| {
        AppError::Internal {
            category: "delegated_upload_checksum",
        }
    })
}
