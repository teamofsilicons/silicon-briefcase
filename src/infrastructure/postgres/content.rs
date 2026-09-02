//! PostgreSQL side of content and object-storage orchestration.

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    application::{
        content::{
            ClientCompletedPart, CompleteMultipartCommand, ConfigureStorageCommand, ContentIntent,
            ContentRepository, DownloadTarget, InitiateMultipartCommand, MultipartAbortTarget,
            MultipartCompletionPreparation, MultipartPartTarget, MultipartPreparation,
            MultipartReceipt, Prepared, RESTORE_LEASE_DURATION, RestorePreparation,
            RestoreVersionCommand, SmallUploadCommand, SmallUploadPreparation, StagedContent,
            StorageConfigurationPreparation, StorageConfigurationResult,
        },
        context::ExecutionContext,
        idempotency::IdempotencyKey,
        ports::{
            ObjectChecksum, ObjectChecksumAlgorithm, ObjectChecksumType, ObjectKey, StorageTarget,
            StoredObject,
        },
        service::{AuthorizableEntry, MutationMetadata},
    },
    config::S3Settings,
    domain::{
        entry::EntryKind,
        ids::{EntryId, MultipartUploadId, StorageConfigurationId, VersionId},
        multipart::{CompletedPart, MultipartPlan},
        permission::Capability,
        storage::EncryptionMode,
    },
    error::AppError,
};

use super::{
    EntryVersionRow, MultipartPartRow, MultipartUploadRow, OrganizationStorageConfigRow,
    PostgresRepository,
    metadata::common::{
        IdempotencyClaim, actor_kind, begin, boundary_columns, claim_idempotency,
        complete_idempotency, load_entry, map_sql, record_change,
    },
};

const SMALL_UPLOAD_OPERATION: &str = "upload_file";
const MULTIPART_INIT_OPERATION: &str = "initiate_multipart_upload";
const MULTIPART_COMPLETE_OPERATION: &str = "complete_multipart_upload";
const VERSION_RESTORE_OPERATION: &str = "restore_version";

/// Multi-tenant content adapter sharing the metadata pool and platform S3 settings.
#[derive(Clone, Debug)]
pub struct PostgresContentRepository {
    repository: PostgresRepository,
    platform: S3Settings,
}

impl PostgresContentRepository {
    /// Composes PostgreSQL with validated platform-storage settings.
    #[must_use]
    pub const fn new(repository: PostgresRepository, platform: S3Settings) -> Self {
        Self {
            repository,
            platform,
        }
    }

    /// Returns the shared metadata repository.
    #[must_use]
    pub const fn metadata(&self) -> &PostgresRepository {
        &self.repository
    }

    fn platform_target(&self, context: &ExecutionContext) -> StorageTarget {
        crate::infrastructure::s3::platform_storage_target(
            &self.platform,
            context.authorization().organization_id(),
        )
    }
}

#[allow(clippy::too_many_lines)]
#[async_trait]
impl ContentRepository for PostgresContentRepository {
    async fn prepare_small_upload(
        &self,
        context: &ExecutionContext,
        command: &SmallUploadCommand,
        _content: &StagedContent<'_>,
    ) -> std::result::Result<Prepared<SmallUploadPreparation, EntryId>, AppError> {
        let mut request = content_begin(&self.repository, context).await?;
        let parent = require_parent(
            &mut request.transaction,
            context,
            command.parent_id,
            Capability::CreateChild,
        )
        .await?;
        let proposed_entry_id = EntryId::new();
        let metadata = keyed_metadata(&command.idempotency_key, command.request_hash);
        let entry_id = match claim_idempotency(
            &mut request.transaction,
            &request.context,
            SMALL_UPLOAD_OPERATION,
            &metadata,
            Some(proposed_entry_id.as_uuid()),
        )
        .await
        .map_err(map_metadata)?
        {
            IdempotencyClaim::Replay(Some(resource_id)) => {
                let replay = EntryId::from_uuid(resource_id).map_err(internal_error)?;
                request.transaction.commit().await.map_err(database_error)?;
                return Ok(Prepared::Replay(replay));
            }
            IdempotencyClaim::Acquired(Some(resource_id)) => {
                EntryId::from_uuid(resource_id).map_err(internal_error)?
            }
            IdempotencyClaim::Replay(None) | IdempotencyClaim::Acquired(None) => {
                return Err(conflict("idempotency_state"));
            }
        };
        ensure_name_is_versionable(
            &mut request.transaction,
            command.parent_id,
            command.name.as_str(),
        )
        .await?;
        let (target, _, _) = resolve_write_target(
            &mut request.transaction,
            context,
            self.platform_target(context),
        )
        .await?;
        let key = object_key(format!("entries/{entry_id}/versions/initial"))?;
        // Retain the parent lock until the idempotency reservation is committed.
        let _ = parent;
        request.transaction.commit().await.map_err(database_error)?;
        Ok(Prepared::Acquired(SmallUploadPreparation {
            entry_id,
            target,
            key,
        }))
    }

    async fn commit_small_upload(
        &self,
        context: &ExecutionContext,
        command: &SmallUploadCommand,
        staged: &StagedContent<'_>,
        preparation: &SmallUploadPreparation,
        stored: &StoredObject,
    ) -> std::result::Result<EntryId, AppError> {
        validate_stored(stored, &preparation.key, staged.size)?;
        let checksum = stored_checksum(stored, ObjectChecksumType::FullObject)?;
        if checksum.encoded_value() != STANDARD.encode(staged.sha256) {
            return Err(conflict("stored_object_checksum_mismatch"));
        }
        let mut request = content_begin(&self.repository, context).await?;
        let parent = require_parent(
            &mut request.transaction,
            context,
            command.parent_id,
            Capability::CreateChild,
        )
        .await?;
        ensure_idempotency_in_progress(
            &mut request.transaction,
            &request.context,
            SMALL_UPLOAD_OPERATION,
            &command.idempotency_key,
            &command.request_hash,
        )
        .await?;
        let storage = identify_storage_target(
            &mut request.transaction,
            context,
            &preparation.target,
            &self.platform,
        )
        .await?;
        let published = publish_file_content(
            &mut request.transaction,
            context,
            &request.context,
            &parent,
            preparation.entry_id,
            command.name.as_str(),
            &command.content_type,
            staged.size,
            &checksum,
            &preparation.target,
            &preparation.key,
            stored,
            storage,
        )
        .await?;
        let metadata = keyed_metadata(&command.idempotency_key, command.request_hash);
        // An upload over an existing file publishes a new version of that
        // file, so the published identifier — not the reserved one — is what a
        // replay of this key must return.
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            SMALL_UPLOAD_OPERATION,
            &metadata,
            Some(published.as_uuid()),
        )
        .await
        .map_err(map_metadata)?;
        request
            .transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, SMALL_UPLOAD_OPERATION))?;
        Ok(published)
    }

    async fn release_operation(
        &self,
        context: &ExecutionContext,
        operation: &'static str,
        key: &IdempotencyKey,
        request_hash: &[u8; 32],
    ) -> std::result::Result<(), AppError> {
        let mut request = content_begin(&self.repository, context).await?;
        sqlx::query(
            "DELETE FROM briefcase.idempotency_records \
              WHERE org_id = briefcase.current_org_id() \
                AND actor_type = $1 AND actor_id = $2 AND origin_app_id = $3 \
                AND operation = $4 AND idempotency_key = $5 AND request_hash = $6 \
                AND status = 'in_progress'",
        )
        .bind(request.context.actor_type())
        .bind(request.context.actor_id())
        .bind(request.context.origin_app_id().unwrap_or_default())
        .bind(operation)
        .bind(key.as_str())
        .bind(request_hash.as_slice())
        .execute(&mut *request.transaction)
        .await
        .map_err(database_error)?;
        request.transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    async fn prepare_multipart(
        &self,
        context: &ExecutionContext,
        command: &InitiateMultipartCommand,
        plan: MultipartPlan,
        expires_at: OffsetDateTime,
    ) -> std::result::Result<Prepared<MultipartPreparation, MultipartReceipt>, AppError> {
        let mut request = content_begin(&self.repository, context).await?;
        require_parent(
            &mut request.transaction,
            context,
            command.parent_id,
            Capability::CreateChild,
        )
        .await?;
        let proposed_upload_id = MultipartUploadId::new();
        let metadata = keyed_metadata(&command.idempotency_key, command.request_hash);
        let upload_id = match claim_idempotency(
            &mut request.transaction,
            &request.context,
            MULTIPART_INIT_OPERATION,
            &metadata,
            Some(proposed_upload_id.as_uuid()),
        )
        .await
        .map_err(map_metadata)?
        {
            IdempotencyClaim::Replay(Some(resource_id)) => {
                let row = find_multipart(&mut request.transaction, resource_id, false)
                    .await?
                    .ok_or_else(|| conflict("idempotency_state"))?;
                let receipt = multipart_receipt(&row)?;
                request.transaction.commit().await.map_err(database_error)?;
                return Ok(Prepared::Replay(receipt));
            }
            IdempotencyClaim::Acquired(Some(resource_id)) => {
                MultipartUploadId::from_uuid(resource_id).map_err(internal_error)?
            }
            IdempotencyClaim::Replay(None) | IdempotencyClaim::Acquired(None) => {
                return Err(conflict("idempotency_state"));
            }
        };
        ensure_name_is_versionable(
            &mut request.transaction,
            command.parent_id,
            command.name.as_str(),
        )
        .await?;
        let (target, _, _) = resolve_write_target(
            &mut request.transaction,
            context,
            self.platform_target(context),
        )
        .await?;
        let key = object_key(format!("multipart/{upload_id}/object"))?;
        request.transaction.commit().await.map_err(database_error)?;
        Ok(Prepared::Acquired(MultipartPreparation {
            upload_id,
            plan,
            target,
            key,
            expires_at,
        }))
    }

    async fn commit_multipart_initialization(
        &self,
        context: &ExecutionContext,
        command: &InitiateMultipartCommand,
        preparation: &MultipartPreparation,
        provider_upload_id: &str,
    ) -> std::result::Result<MultipartReceipt, AppError> {
        if provider_upload_id.trim().is_empty() {
            return Err(AppError::validation("invalid_provider_upload_id"));
        }
        let mut request = content_begin(&self.repository, context).await?;
        require_parent(
            &mut request.transaction,
            context,
            command.parent_id,
            Capability::CreateChild,
        )
        .await?;
        ensure_idempotency_in_progress(
            &mut request.transaction,
            &request.context,
            MULTIPART_INIT_OPERATION,
            &command.idempotency_key,
            &command.request_hash,
        )
        .await?;
        let storage = identify_storage_target(
            &mut request.transaction,
            context,
            &preparation.target,
            &self.platform,
        )
        .await?;
        let actor = context.authorization().actor();
        sqlx::query(
            "INSERT INTO briefcase.multipart_uploads ( \
                    org_id, upload_id, parent_entry_id, owner_type, owner_id, origin_app_id, \
                    name, content_type, declared_size_bytes, part_size_bytes, expected_part_count, \
                    storage_backend, storage_config_id, bucket_name, storage_region, \
                    storage_prefix, storage_encryption_mode, storage_kms_key_arn, object_key, \
                    provider_upload_id, expires_at \
             ) VALUES (briefcase.current_org_id(), $1, $2, $3, $4, $5, $6, $7, $8, $9, \
                       $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)",
        )
        .bind(preparation.upload_id.as_uuid())
        .bind(command.parent_id.as_uuid())
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .bind(
            context
                .authorization()
                .originating_application()
                .map(crate::domain::actor::ApplicationId::as_str),
        )
        .bind(command.name.as_str())
        .bind(&command.content_type)
        .bind(to_i64(preparation.plan.file_size())?)
        .bind(to_i64(preparation.plan.part_size())?)
        .bind(to_i32(preparation.plan.part_count())?)
        .bind(storage.backend)
        .bind(storage.configuration_id)
        .bind(&preparation.target.bucket)
        .bind(&preparation.target.region)
        .bind(&preparation.target.prefix)
        .bind(encryption_name(preparation.target.encryption))
        .bind(preparation.target.kms_key_arn.as_deref())
        .bind(preparation.key.as_str())
        .bind(provider_upload_id)
        .bind(preparation.expires_at)
        .execute(&mut *request.transaction)
        .await
        .map_err(database_error)?;
        let metadata = keyed_metadata(&command.idempotency_key, command.request_hash);
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            MULTIPART_INIT_OPERATION,
            &metadata,
            Some(preparation.upload_id.as_uuid()),
        )
        .await
        .map_err(map_metadata)?;
        record_change(
            &mut request.transaction,
            &request.context,
            Some(command.parent_id.as_uuid()),
            "multipart.initiated.v1",
            "multipart_upload",
            &preparation.upload_id.to_string(),
            json!({"parent_id": command.parent_id, "expires_at": preparation.expires_at}),
        )
        .await
        .map_err(map_metadata)?;
        let receipt = MultipartReceipt {
            upload_id: preparation.upload_id,
            part_size: preparation.plan.part_size(),
            part_count: preparation.plan.part_count(),
            expires_at: preparation.expires_at,
        };
        request
            .transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, MULTIPART_INIT_OPERATION))?;
        Ok(receipt)
    }

    async fn authorize_multipart_part(
        &self,
        context: &ExecutionContext,
        upload_id: MultipartUploadId,
        part_number: u32,
    ) -> std::result::Result<MultipartPartTarget, AppError> {
        let mut request = content_begin(&self.repository, context).await?;
        let row = find_multipart(&mut request.transaction, upload_id.as_uuid(), false)
            .await?
            .ok_or(AppError::NotFound)?;
        authorize_multipart_owner(context, &row)?;
        if !matches!(row.status.as_str(), "initiated" | "uploading")
            || row.expires_at <= OffsetDateTime::now_utc()
        {
            return Err(conflict("multipart_not_uploadable"));
        }
        let plan = multipart_plan(&row)?;
        plan.expected_part_size(part_number)
            .map_err(|_| AppError::validation("invalid_part_number"))?;
        require_parent(
            &mut request.transaction,
            context,
            EntryId::from_uuid(row.parent_entry_id).map_err(internal_error)?,
            Capability::CreateChild,
        )
        .await?;
        let target =
            target_for_multipart(&mut request.transaction, context, &row, &self.platform).await?;
        let result = MultipartPartTarget {
            plan,
            target,
            key: object_key(row.object_key)?,
            provider_upload_id: row.provider_upload_id,
        };
        request.transaction.commit().await.map_err(database_error)?;
        Ok(result)
    }

    async fn record_multipart_part(
        &self,
        context: &ExecutionContext,
        upload_id: MultipartUploadId,
        part_number: u32,
        etag: &str,
        staged: &StagedContent<'_>,
    ) -> std::result::Result<(), AppError> {
        if etag.trim().is_empty() {
            return Err(AppError::validation("invalid_part_etag"));
        }
        let mut request = content_begin(&self.repository, context).await?;
        let row = find_multipart(&mut request.transaction, upload_id.as_uuid(), true)
            .await?
            .ok_or(AppError::NotFound)?;
        authorize_multipart_owner(context, &row)?;
        if !matches!(row.status.as_str(), "initiated" | "uploading")
            || row.expires_at <= OffsetDateTime::now_utc()
        {
            return Err(conflict("multipart_not_uploadable"));
        }
        let plan = multipart_plan(&row)?;
        if plan
            .expected_part_size(part_number)
            .map_err(|_| AppError::validation("invalid_part_number"))?
            != staged.size
        {
            return Err(AppError::validation("invalid_part_size"));
        }
        require_parent(
            &mut request.transaction,
            context,
            EntryId::from_uuid(row.parent_entry_id).map_err(internal_error)?,
            Capability::CreateChild,
        )
        .await?;
        PostgresRepository::upsert_multipart_part(
            &mut request.transaction,
            upload_id.as_uuid(),
            part_number,
            etag,
            staged.size,
            &staged.sha256,
        )
        .await
        .map_err(database_error)?;
        sqlx::query(
            "UPDATE briefcase.multipart_uploads SET status = 'uploading' \
              WHERE org_id = briefcase.current_org_id() AND upload_id = $1 \
                AND status IN ('initiated', 'uploading')",
        )
        .bind(upload_id.as_uuid())
        .execute(&mut *request.transaction)
        .await
        .map_err(database_error)?;
        request.transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    async fn prepare_multipart_completion(
        &self,
        context: &ExecutionContext,
        command: &CompleteMultipartCommand,
    ) -> std::result::Result<Prepared<MultipartCompletionPreparation, EntryId>, AppError> {
        let mut request = content_begin(&self.repository, context).await?;
        let row = find_multipart(&mut request.transaction, command.upload_id.as_uuid(), true)
            .await?
            .ok_or(AppError::NotFound)?;
        authorize_multipart_owner(context, &row)?;
        if multipart_completion_expired(&row.status, row.expires_at, OffsetDateTime::now_utc()) {
            return Err(conflict("multipart_expired"));
        }
        require_parent(
            &mut request.transaction,
            context,
            EntryId::from_uuid(row.parent_entry_id).map_err(internal_error)?,
            Capability::CreateChild,
        )
        .await?;
        let proposed_entry_id = EntryId::new();
        let metadata = keyed_metadata(&command.idempotency_key, command.request_hash);
        let entry_id = match claim_idempotency(
            &mut request.transaction,
            &request.context,
            MULTIPART_COMPLETE_OPERATION,
            &metadata,
            Some(proposed_entry_id.as_uuid()),
        )
        .await
        .map_err(map_metadata)?
        {
            IdempotencyClaim::Replay(Some(resource_id)) => {
                let replay = EntryId::from_uuid(resource_id).map_err(internal_error)?;
                request.transaction.commit().await.map_err(database_error)?;
                return Ok(Prepared::Replay(replay));
            }
            IdempotencyClaim::Acquired(Some(resource_id)) => {
                EntryId::from_uuid(resource_id).map_err(internal_error)?
            }
            IdempotencyClaim::Replay(None) | IdempotencyClaim::Acquired(None) => {
                return Err(conflict("idempotency_state"));
            }
        };
        if !matches!(
            row.status.as_str(),
            "initiated" | "uploading" | "completing"
        ) {
            return Err(conflict("multipart_not_completable"));
        }
        let plan = multipart_plan(&row)?;
        let stored_parts = PostgresRepository::list_multipart_parts(
            &mut request.transaction,
            command.upload_id.as_uuid(),
        )
        .await
        .map_err(database_error)?;
        let parts = completion_parts(plan, &stored_parts, &command.parts)?;
        if row.status != "completing" {
            let updated = sqlx::query(
                "UPDATE briefcase.multipart_uploads SET status = 'completing' \
                  WHERE org_id = briefcase.current_org_id() AND upload_id = $1 \
                    AND status IN ('initiated', 'uploading')",
            )
            .bind(command.upload_id.as_uuid())
            .execute(&mut *request.transaction)
            .await
            .map_err(database_error)?;
            if updated.rows_affected() != 1 {
                return Err(conflict("multipart_state"));
            }
        }
        let target =
            target_for_multipart(&mut request.transaction, context, &row, &self.platform).await?;
        let result = MultipartCompletionPreparation {
            entry_id,
            plan,
            target,
            key: object_key(row.object_key)?,
            provider_upload_id: row.provider_upload_id,
            parts,
        };
        request.transaction.commit().await.map_err(database_error)?;
        Ok(Prepared::Acquired(result))
    }

    async fn commit_multipart_completion(
        &self,
        context: &ExecutionContext,
        command: &CompleteMultipartCommand,
        preparation: &MultipartCompletionPreparation,
        stored: &StoredObject,
    ) -> std::result::Result<EntryId, AppError> {
        validate_stored(stored, &preparation.key, preparation.plan.file_size())?;
        let checksum = stored_checksum(stored, ObjectChecksumType::Composite)?;
        let mut request = content_begin(&self.repository, context).await?;
        let row = find_multipart(&mut request.transaction, command.upload_id.as_uuid(), true)
            .await?
            .ok_or(AppError::NotFound)?;
        authorize_multipart_owner(context, &row)?;
        if row.status != "completing" {
            return Err(conflict("multipart_state"));
        }
        let parent_id = EntryId::from_uuid(row.parent_entry_id).map_err(internal_error)?;
        let parent = require_parent(
            &mut request.transaction,
            context,
            parent_id,
            Capability::CreateChild,
        )
        .await?;
        ensure_idempotency_in_progress(
            &mut request.transaction,
            &request.context,
            MULTIPART_COMPLETE_OPERATION,
            &command.idempotency_key,
            &command.request_hash,
        )
        .await?;
        let target =
            target_for_multipart(&mut request.transaction, context, &row, &self.platform).await?;
        if target != preparation.target {
            return Err(conflict("storage_target_changed"));
        }
        let storage = StorageReference {
            backend: row.storage_backend.as_str(),
            configuration_id: row.storage_config_id,
        };
        let published = publish_file_content(
            &mut request.transaction,
            context,
            &request.context,
            &parent,
            preparation.entry_id,
            &row.name,
            &row.content_type,
            preparation.plan.file_size(),
            &checksum,
            &preparation.target,
            &preparation.key,
            stored,
            storage,
        )
        .await?;
        sqlx::query(
            "UPDATE briefcase.multipart_uploads \
                SET status = 'completed', completed_entry_id = $2, completed_at = clock_timestamp() \
              WHERE org_id = briefcase.current_org_id() AND upload_id = $1 AND status = 'completing'",
        )
        .bind(command.upload_id.as_uuid())
        .bind(published.as_uuid())
        .execute(&mut *request.transaction)
        .await
        .map_err(database_error)?;
        let metadata = keyed_metadata(&command.idempotency_key, command.request_hash);
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            MULTIPART_COMPLETE_OPERATION,
            &metadata,
            Some(published.as_uuid()),
        )
        .await
        .map_err(map_metadata)?;
        request.transaction.commit().await.map_err(database_error)?;
        Ok(published)
    }

    async fn abort_multipart(
        &self,
        context: &ExecutionContext,
        upload_id: MultipartUploadId,
    ) -> std::result::Result<Option<MultipartAbortTarget>, AppError> {
        let mut request = content_begin(&self.repository, context).await?;
        let Some(row) = find_multipart(&mut request.transaction, upload_id.as_uuid(), true).await?
        else {
            request.transaction.commit().await.map_err(database_error)?;
            return Ok(None);
        };
        authorize_multipart_owner(context, &row)?;
        match multipart_abort_disposition(&row.status)? {
            MultipartAbortDisposition::AlreadyStopped => {
                request.transaction.commit().await.map_err(database_error)?;
                return Ok(None);
            }
            MultipartAbortDisposition::Reject => {
                return Err(conflict("multipart_not_abortable"));
            }
            MultipartAbortDisposition::Abort => {}
        }
        let target =
            target_for_multipart(&mut request.transaction, context, &row, &self.platform).await?;
        sqlx::query(
            "UPDATE briefcase.multipart_uploads \
                SET status = 'aborted', aborted_at = clock_timestamp() \
              WHERE org_id = briefcase.current_org_id() AND upload_id = $1 \
                AND status NOT IN ('completed', 'aborted', 'expired')",
        )
        .bind(upload_id.as_uuid())
        .execute(&mut *request.transaction)
        .await
        .map_err(database_error)?;
        enqueue_multipart_abort_cleanup(&mut request.transaction, &row, &target).await?;
        record_change(
            &mut request.transaction,
            &request.context,
            Some(row.parent_entry_id),
            "multipart.aborted.v1",
            "multipart_upload",
            &upload_id.to_string(),
            json!({}),
        )
        .await
        .map_err(map_metadata)?;
        let result = MultipartAbortTarget {
            target,
            key: object_key(row.object_key)?,
            provider_upload_id: row.provider_upload_id,
        };
        request.transaction.commit().await.map_err(database_error)?;
        Ok(Some(result))
    }

    async fn authorize_download(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
    ) -> std::result::Result<DownloadTarget, AppError> {
        let mut request = content_begin(&self.repository, context).await?;
        let entry = load_entry(&mut request.transaction, context, entry_id, false, false)
            .await
            .map_err(map_metadata)?
            .ok_or(AppError::NotFound)?;
        require_entry_capability(&entry, context, Capability::Read)?;
        if entry.entry.kind != EntryKind::File {
            return Err(AppError::NotFound);
        }
        let version_id = entry
            .entry
            .current_version_id
            .ok_or_else(internal_integrity)?;
        let version = find_version(
            &mut request.transaction,
            entry_id.as_uuid(),
            version_id.as_uuid(),
            false,
        )
        .await?
        .ok_or(AppError::NotFound)?;
        let target =
            target_for_version(&mut request.transaction, context, &version, &self.platform).await?;
        let result = DownloadTarget {
            entry_id,
            filename: entry.entry.name.as_str().to_owned(),
            content_type: version.content_type.clone(),
            size: u64::try_from(version.size_bytes).map_err(|_| internal_integrity())?,
            target,
            key: object_key(version.object_key)?,
            provider_version_id: version.object_version_id,
        };
        request.transaction.commit().await.map_err(database_error)?;
        Ok(result)
    }

    async fn record_content_access(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
        intent: ContentIntent,
    ) -> std::result::Result<(), AppError> {
        let mut request = content_begin(&self.repository, context).await?;
        let entry = load_entry(&mut request.transaction, context, entry_id, false, false)
            .await
            .map_err(map_metadata)?
            .ok_or(AppError::NotFound)?;
        require_entry_capability(&entry, context, Capability::Read)?;
        PostgresRepository::insert_audit_event(
            &mut request.transaction,
            &request.context,
            &super::NewAuditEvent {
                audit_id: Uuid::now_v7(),
                entry_id: Some(entry_id.as_uuid()),
                action: intent.audit_action().to_owned(),
                metadata: json!({}),
            },
        )
        .await
        .map_err(database_error)?;
        request.transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    async fn prepare_version_restore(
        &self,
        context: &ExecutionContext,
        command: &RestoreVersionCommand,
    ) -> std::result::Result<Prepared<RestorePreparation, EntryId>, AppError> {
        let mut request = content_begin(&self.repository, context).await?;
        let entry = load_entry(
            &mut request.transaction,
            context,
            command.entry_id,
            false,
            true,
        )
        .await
        .map_err(map_metadata)?
        .ok_or(AppError::NotFound)?;
        require_entry_capability(&entry, context, Capability::WriteContent)?;
        if entry.entry.kind != EntryKind::File {
            return Err(AppError::NotFound);
        }
        let source = find_version(
            &mut request.transaction,
            command.entry_id.as_uuid(),
            command.version_id.as_uuid(),
            false,
        )
        .await?
        .ok_or(AppError::NotFound)?;
        let proposed_version_id = VersionId::new();
        let metadata = keyed_metadata(&command.idempotency_key, command.request_hash);
        let new_version_id = match claim_idempotency(
            &mut request.transaction,
            &request.context,
            VERSION_RESTORE_OPERATION,
            &metadata,
            Some(proposed_version_id.as_uuid()),
        )
        .await
        .map_err(map_metadata)?
        {
            IdempotencyClaim::Replay(_) => {
                request.transaction.commit().await.map_err(database_error)?;
                return Ok(Prepared::Replay(command.entry_id));
            }
            IdempotencyClaim::Acquired(Some(resource_id)) => {
                extend_version_restore_lease(
                    &mut request.transaction,
                    &request.context,
                    &command.idempotency_key,
                    &command.request_hash,
                )
                .await?;
                VersionId::from_uuid(resource_id).map_err(internal_error)?
            }
            IdempotencyClaim::Acquired(None) => return Err(conflict("idempotency_state")),
        };
        let source_target =
            target_for_version(&mut request.transaction, context, &source, &self.platform).await?;
        let (destination_target, _, _) = resolve_write_target(
            &mut request.transaction,
            context,
            self.platform_target(context),
        )
        .await?;
        let destination_key = object_key(format!(
            "entries/{}/versions/{new_version_id}",
            command.entry_id
        ))?;
        let checksum = object_checksum(
            &source.checksum_algorithm,
            &source.checksum_type,
            source.checksum_value,
        )?;
        let result = RestorePreparation {
            entry_id: command.entry_id,
            new_version_id,
            source_target,
            source_key: object_key(source.object_key)?,
            source_provider_version_id: source.object_version_id,
            destination_target,
            destination_key,
            content_type: source.content_type,
            size: to_u64(source.size_bytes)?,
            checksum,
        };
        request.transaction.commit().await.map_err(database_error)?;
        Ok(Prepared::Acquired(result))
    }

    async fn renew_version_restore_lease(
        &self,
        context: &ExecutionContext,
        command: &RestoreVersionCommand,
    ) -> std::result::Result<(), AppError> {
        let mut request = content_begin(&self.repository, context).await?;
        extend_version_restore_lease(
            &mut request.transaction,
            &request.context,
            &command.idempotency_key,
            &command.request_hash,
        )
        .await?;
        request.transaction.commit().await.map_err(database_error)
    }

    async fn commit_version_restore(
        &self,
        context: &ExecutionContext,
        command: &RestoreVersionCommand,
        preparation: &RestorePreparation,
        stored: &StoredObject,
    ) -> std::result::Result<EntryId, AppError> {
        validate_stored(stored, &preparation.destination_key, preparation.size)?;
        let mut request = content_begin(&self.repository, context).await?;
        let entry = load_entry(
            &mut request.transaction,
            context,
            command.entry_id,
            false,
            true,
        )
        .await
        .map_err(map_metadata)?
        .ok_or(AppError::NotFound)?;
        require_entry_capability(&entry, context, Capability::WriteContent)?;
        find_version(
            &mut request.transaction,
            command.entry_id.as_uuid(),
            command.version_id.as_uuid(),
            false,
        )
        .await?
        .ok_or(AppError::NotFound)?;
        ensure_idempotency_in_progress(
            &mut request.transaction,
            &request.context,
            VERSION_RESTORE_OPERATION,
            &command.idempotency_key,
            &command.request_hash,
        )
        .await?;
        let storage = identify_storage_target(
            &mut request.transaction,
            context,
            &preparation.destination_target,
            &self.platform,
        )
        .await?;
        let next_number = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(version_number), 0) + 1 FROM briefcase.entry_versions \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1",
        )
        .bind(command.entry_id.as_uuid())
        .fetch_one(&mut *request.transaction)
        .await
        .map_err(database_error)?;
        let actor = context.authorization().actor();
        let stored_checksum = verified_stored_checksum(stored)?;
        sqlx::query(
            "INSERT INTO briefcase.entry_versions ( \
                    org_id, entry_id, version_id, version_number, source, \
                    restored_from_version_id, storage_backend, storage_config_id, bucket_name, \
                    storage_region, storage_prefix, storage_encryption_mode, storage_kms_key_arn, \
                    object_key, object_version_id, etag, checksum_algorithm, checksum_type, \
                    checksum_value, size_bytes, \
                    content_type, created_by_type, created_by_id \
             ) VALUES (briefcase.current_org_id(), $1, $2, $3, 'restore', $4, $5, $6, $7, \
                       $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21)",
        )
        .bind(command.entry_id.as_uuid())
        .bind(preparation.new_version_id.as_uuid())
        .bind(next_number)
        .bind(command.version_id.as_uuid())
        .bind(storage.backend)
        .bind(storage.configuration_id)
        .bind(&preparation.destination_target.bucket)
        .bind(&preparation.destination_target.region)
        .bind(&preparation.destination_target.prefix)
        .bind(encryption_name(preparation.destination_target.encryption))
        .bind(preparation.destination_target.kms_key_arn.as_deref())
        .bind(preparation.destination_key.as_str())
        .bind(stored.provider_version_id.as_deref())
        .bind(stored.etag.as_deref())
        .bind(checksum_algorithm(&stored_checksum))
        .bind(checksum_type(&stored_checksum))
        .bind(stored_checksum.encoded_value())
        .bind(to_i64(preparation.size)?)
        .bind(&preparation.content_type)
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .execute(&mut *request.transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "UPDATE briefcase.entries \
                SET current_version_id = $2, size_bytes = $3, content_type = $4, \
                    updated_by_type = $5, updated_by_id = $6 \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1 AND deleted_at IS NULL",
        )
        .bind(command.entry_id.as_uuid())
        .bind(preparation.new_version_id.as_uuid())
        .bind(to_i64(preparation.size)?)
        .bind(&preparation.content_type)
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .execute(&mut *request.transaction)
        .await
        .map_err(database_error)?;
        record_change(
            &mut request.transaction,
            &request.context,
            Some(command.entry_id.as_uuid()),
            "entry.version_restored.v1",
            "entry",
            &command.entry_id.to_string(),
            json!({
                "version_id": preparation.new_version_id,
                "source_version_id": command.version_id,
                "version_number": next_number,
            }),
        )
        .await
        .map_err(map_metadata)?;
        let metadata = keyed_metadata(&command.idempotency_key, command.request_hash);
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            VERSION_RESTORE_OPERATION,
            &metadata,
            Some(preparation.new_version_id.as_uuid()),
        )
        .await
        .map_err(map_metadata)?;
        request
            .transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, VERSION_RESTORE_OPERATION))?;
        Ok(command.entry_id)
    }

    async fn prepare_storage_configuration(
        &self,
        context: &ExecutionContext,
        command: &ConfigureStorageCommand,
    ) -> std::result::Result<
        Prepared<StorageConfigurationPreparation, StorageConfigurationResult>,
        AppError,
    > {
        require_administrator(context)?;
        let mut request = content_begin(&self.repository, context).await?;
        let proposed_configuration_id = StorageConfigurationId::new();
        let configuration_id = if let Some((key, hash)) = &command.idempotency {
            let metadata = keyed_metadata(key, *hash);
            match claim_idempotency(
                &mut request.transaction,
                &request.context,
                "configure_storage",
                &metadata,
                Some(proposed_configuration_id.as_uuid()),
            )
            .await
            .map_err(map_metadata)?
            {
                IdempotencyClaim::Replay(Some(resource_id)) => {
                    let configuration =
                        find_storage_configuration(&mut request.transaction, resource_id)
                            .await?
                            .ok_or_else(|| conflict("idempotency_state"))?;
                    let replay = storage_configuration_result(configuration)?;
                    request.transaction.commit().await.map_err(database_error)?;
                    return Ok(Prepared::Replay(replay));
                }
                IdempotencyClaim::Acquired(Some(resource_id)) => {
                    StorageConfigurationId::from_uuid(resource_id).map_err(internal_error)?
                }
                IdempotencyClaim::Replay(None) | IdempotencyClaim::Acquired(None) => {
                    return Err(conflict("idempotency_state"));
                }
            }
        } else {
            proposed_configuration_id
        };
        let actor = context.authorization().actor();
        let encryption = encryption_name(command.encryption);
        let inserted = sqlx::query(
            "INSERT INTO briefcase.organization_storage_configs ( \
                    org_id, storage_config_id, status, bucket_name, region, role_arn, \
                    bucket_prefix, aws_account_id, encryption_mode, kms_key_arn, \
                    created_by_type, created_by_id \
             ) VALUES (briefcase.current_org_id(), $1, 'validating', $2, $3, $4, $5, $6, \
                       $7, $8, $9, $10) \
             ON CONFLICT (org_id, storage_config_id) DO NOTHING",
        )
        .bind(configuration_id.as_uuid())
        .bind(&command.bucket_name)
        .bind(&command.region)
        .bind(&command.role_arn)
        .bind(&command.prefix)
        .bind(&command.aws_account_id)
        .bind(encryption)
        .bind(command.kms_key_arn.as_deref())
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .execute(&mut *request.transaction)
        .await
        .map_err(database_error)?;
        let configuration =
            find_storage_configuration(&mut request.transaction, configuration_id.as_uuid())
                .await?
                .ok_or_else(|| conflict("idempotency_state"))?;
        if configuration.status != "validating"
            || configuration.bucket_name != command.bucket_name
            || configuration.region != command.region
            || configuration.role_arn != command.role_arn
            || configuration.bucket_prefix != command.prefix
            || configuration.aws_account_id != command.aws_account_id
            || configuration.encryption_mode != encryption
            || configuration.kms_key_arn != command.kms_key_arn
            || configuration.created_by_type != actor_kind(actor.kind())
            || configuration.created_by_id != actor.id().as_str()
        {
            return Err(conflict("idempotency_state"));
        }
        let expected_account_id = configuration.aws_account_id.clone();
        let target = target_from_configuration(context, configuration)?;
        if inserted.rows_affected() == 1 {
            record_change(
                &mut request.transaction,
                &request.context,
                None,
                "storage.configuration_validation_started.v1",
                "storage_configuration",
                &configuration_id.to_string(),
                json!({"configuration_id": configuration_id}),
            )
            .await
            .map_err(map_metadata)?;
        }
        request.transaction.commit().await.map_err(database_error)?;
        Ok(Prepared::Acquired(StorageConfigurationPreparation {
            configuration_id,
            target,
            expected_account_id,
        }))
    }

    async fn activate_storage_configuration(
        &self,
        context: &ExecutionContext,
        command: &ConfigureStorageCommand,
        preparation: &StorageConfigurationPreparation,
        tested_at: OffsetDateTime,
    ) -> std::result::Result<StorageConfigurationResult, AppError> {
        require_administrator(context)?;
        let mut request = content_begin(&self.repository, context).await?;
        if let Some((key, hash)) = &command.idempotency {
            ensure_idempotency_in_progress(
                &mut request.transaction,
                &request.context,
                "configure_storage",
                key,
                hash,
            )
            .await?;
        }
        lock_storage_configuration(
            &mut request.transaction,
            preparation.configuration_id.as_uuid(),
        )
        .await?;
        sqlx::query(
            "UPDATE briefcase.organization_storage_configs \
                SET status = 'superseded' \
              WHERE org_id = briefcase.current_org_id() AND status = 'active' \
                AND storage_config_id <> $1",
        )
        .bind(preparation.configuration_id.as_uuid())
        .execute(&mut *request.transaction)
        .await
        .map_err(database_error)?;
        let updated = sqlx::query(
            "UPDATE briefcase.organization_storage_configs \
                SET status = 'active', validated_at = $2 \
              WHERE org_id = briefcase.current_org_id() AND storage_config_id = $1 \
                AND status = 'validating'",
        )
        .bind(preparation.configuration_id.as_uuid())
        .bind(tested_at)
        .execute(&mut *request.transaction)
        .await
        .map_err(database_error)?;
        if updated.rows_affected() != 1 {
            return Err(conflict("storage_configuration_state"));
        }
        record_change(
            &mut request.transaction,
            &request.context,
            None,
            "storage.configuration_activated.v1",
            "storage_configuration",
            &preparation.configuration_id.to_string(),
            json!({"tested_at": tested_at}),
        )
        .await
        .map_err(map_metadata)?;
        if let Some((key, hash)) = &command.idempotency {
            let metadata = keyed_metadata(key, *hash);
            complete_idempotency(
                &mut request.transaction,
                &request.context,
                "configure_storage",
                &metadata,
                Some(preparation.configuration_id.as_uuid()),
            )
            .await
            .map_err(map_metadata)?;
        }
        request.transaction.commit().await.map_err(database_error)?;
        Ok(StorageConfigurationResult {
            configuration_id: preparation.configuration_id,
            configured: true,
            tested_at,
            failure_reason: None,
        })
    }

    async fn fail_storage_configuration(
        &self,
        context: &ExecutionContext,
        command: &ConfigureStorageCommand,
        preparation: &StorageConfigurationPreparation,
        tested_at: OffsetDateTime,
        reason: &'static str,
    ) -> std::result::Result<StorageConfigurationResult, AppError> {
        require_administrator(context)?;
        let mut request = content_begin(&self.repository, context).await?;
        if let Some((key, hash)) = &command.idempotency {
            ensure_idempotency_in_progress(
                &mut request.transaction,
                &request.context,
                "configure_storage",
                key,
                hash,
            )
            .await?;
        }
        lock_storage_configuration(
            &mut request.transaction,
            preparation.configuration_id.as_uuid(),
        )
        .await?;
        let updated = sqlx::query(
            "UPDATE briefcase.organization_storage_configs \
                SET status = 'failed', validated_at = $2, validation_failure_code = $3, \
                    validation_failure_reason = $3 \
              WHERE org_id = briefcase.current_org_id() AND storage_config_id = $1 \
                AND status = 'validating'",
        )
        .bind(preparation.configuration_id.as_uuid())
        .bind(tested_at)
        .bind(reason)
        .execute(&mut *request.transaction)
        .await
        .map_err(database_error)?;
        if updated.rows_affected() != 1 {
            return Err(conflict("storage_configuration_state"));
        }
        record_change(
            &mut request.transaction,
            &request.context,
            None,
            "storage.configuration_failed.v1",
            "storage_configuration",
            &preparation.configuration_id.to_string(),
            json!({"tested_at": tested_at, "reason": reason}),
        )
        .await
        .map_err(map_metadata)?;
        if let Some((key, hash)) = &command.idempotency {
            let metadata = keyed_metadata(key, *hash);
            complete_idempotency(
                &mut request.transaction,
                &request.context,
                "configure_storage",
                &metadata,
                Some(preparation.configuration_id.as_uuid()),
            )
            .await
            .map_err(map_metadata)?;
        }
        request.transaction.commit().await.map_err(database_error)?;
        Ok(StorageConfigurationResult {
            configuration_id: preparation.configuration_id,
            configured: false,
            tested_at,
            failure_reason: Some(reason.to_owned()),
        })
    }
}

#[derive(Clone, Copy)]
struct StorageReference<'a> {
    backend: &'a str,
    configuration_id: Option<Uuid>,
}

async fn content_begin<'pool>(
    repository: &'pool PostgresRepository,
    context: &ExecutionContext,
) -> std::result::Result<super::metadata::common::RequestTransaction<'pool>, AppError> {
    begin(repository, context).await.map_err(map_metadata)
}

async fn require_parent(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ExecutionContext,
    parent_id: EntryId,
    capability: Capability,
) -> std::result::Result<AuthorizableEntry, AppError> {
    let parent = load_entry(transaction, context, parent_id, false, true)
        .await
        .map_err(map_metadata)?
        .ok_or(AppError::NotFound)?;
    if parent.entry.kind != EntryKind::Folder {
        return Err(AppError::NotFound);
    }
    require_entry_capability(&parent, context, capability)?;
    Ok(parent)
}

fn require_entry_capability(
    entry: &AuthorizableEntry,
    context: &ExecutionContext,
    capability: Capability,
) -> std::result::Result<(), AppError> {
    if entry
        .authorization(context.authorization())
        .allows(capability)
    {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn require_administrator(context: &ExecutionContext) -> std::result::Result<(), AppError> {
    if context.authorization().role().has_administrative_access() {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Finds the active entry that already carries a name inside a folder.
///
/// Uploading over an existing file is an update, not a collision: it publishes
/// the next version of that file. A folder with the same name is still a
/// collision, because a folder has no content to version.
async fn find_named_child(
    transaction: &mut Transaction<'_, Postgres>,
    parent_id: EntryId,
    name: &str,
    lock: bool,
) -> std::result::Result<Option<(EntryId, String)>, AppError> {
    #[derive(sqlx::FromRow)]
    struct NamedChild {
        entry_id: Uuid,
        entry_type: String,
    }

    let statement = if lock {
        "SELECT entry_id, entry_type FROM briefcase.entries \
          WHERE org_id = briefcase.current_org_id() AND parent_id = $1 \
            AND name = $2 AND deleted_at IS NULL \
          FOR UPDATE"
    } else {
        "SELECT entry_id, entry_type FROM briefcase.entries \
          WHERE org_id = briefcase.current_org_id() AND parent_id = $1 \
            AND name = $2 AND deleted_at IS NULL"
    };
    let row = sqlx::query_as::<_, NamedChild>(statement)
        .bind(parent_id.as_uuid())
        .bind(name)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?;
    row.map(|row| {
        EntryId::from_uuid(row.entry_id)
            .map_err(internal_error)
            .map(|entry_id| (entry_id, row.entry_type))
    })
    .transpose()
}

/// Rejects an upload whose name is taken by something that cannot be versioned.
async fn ensure_name_is_versionable(
    transaction: &mut Transaction<'_, Postgres>,
    parent_id: EntryId,
    name: &str,
) -> std::result::Result<(), AppError> {
    match find_named_child(transaction, parent_id, name, false).await? {
        Some((_, kind)) if kind != "file" => Err(conflict("entry_name_exists")),
        Some(_) | None => Ok(()),
    }
}

async fn resolve_write_target(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ExecutionContext,
    platform: StorageTarget,
) -> std::result::Result<(StorageTarget, &'static str, Option<Uuid>), AppError> {
    let configuration = sqlx::query_as::<_, OrganizationStorageConfigRow>(
        "SELECT org_id, storage_config_id, status, bucket_name, region, role_arn, \
                bucket_prefix, aws_account_id, encryption_mode, kms_key_arn, validated_at, \
                validation_failure_code, validation_failure_reason, created_by_type, \
                created_by_id, created_at, updated_at \
           FROM briefcase.organization_storage_configs \
          WHERE org_id = briefcase.current_org_id() AND status = 'active' \
          ORDER BY validated_at DESC, storage_config_id DESC LIMIT 1",
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    match configuration {
        Some(configuration) => {
            let id = configuration.storage_config_id;
            Ok((
                target_from_configuration(context, configuration)?,
                "organization",
                Some(id),
            ))
        }
        None => Ok((platform, "platform", None)),
    }
}

async fn identify_storage_target<'a>(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ExecutionContext,
    target: &StorageTarget,
    platform: &S3Settings,
) -> std::result::Result<StorageReference<'a>, AppError> {
    let platform_target = crate::infrastructure::s3::platform_storage_target(
        platform,
        context.authorization().organization_id(),
    );
    if target == &platform_target {
        return Ok(StorageReference {
            backend: "platform",
            configuration_id: None,
        });
    }
    let configuration_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT storage_config_id FROM briefcase.organization_storage_configs \
          WHERE org_id = briefcase.current_org_id() \
            AND bucket_name = $1 AND region = $2 AND role_arn = $3 AND bucket_prefix = $4 \
            AND encryption_mode = $5 AND kms_key_arn IS NOT DISTINCT FROM $6 \
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&target.bucket)
    .bind(&target.region)
    .bind(target.role_arn.as_deref())
    .bind(&target.prefix)
    .bind(encryption_name(target.encryption))
    .bind(target.kms_key_arn.as_deref())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| conflict("storage_target_changed"))?;
    Ok(StorageReference {
        backend: "organization",
        configuration_id: Some(configuration_id),
    })
}

fn target_from_configuration(
    context: &ExecutionContext,
    configuration: OrganizationStorageConfigRow,
) -> std::result::Result<StorageTarget, AppError> {
    let encryption = parse_encryption(&configuration.encryption_mode)?;
    Ok(StorageTarget {
        bucket: configuration.bucket_name,
        region: configuration.region,
        prefix: configuration.bucket_prefix,
        role_arn: Some(configuration.role_arn),
        external_id: Some(external_id(context)),
        encryption,
        kms_key_arn: configuration.kms_key_arn,
    })
}

async fn target_for_multipart(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ExecutionContext,
    row: &MultipartUploadRow,
    _platform: &S3Settings,
) -> std::result::Result<StorageTarget, AppError> {
    match row.storage_backend.as_str() {
        "platform" if row.storage_config_id.is_none() => Ok(StorageTarget {
            bucket: row.bucket_name.clone(),
            region: row.storage_region.clone(),
            prefix: row.storage_prefix.clone(),
            role_arn: None,
            external_id: None,
            encryption: parse_encryption(&row.storage_encryption_mode)?,
            kms_key_arn: row.storage_kms_key_arn.clone(),
        }),
        "organization" => {
            let id = row.storage_config_id.ok_or_else(internal_integrity)?;
            let configuration = find_storage_configuration(transaction, id)
                .await?
                .ok_or_else(internal_integrity)?;
            Ok(StorageTarget {
                bucket: row.bucket_name.clone(),
                region: row.storage_region.clone(),
                prefix: row.storage_prefix.clone(),
                role_arn: Some(configuration.role_arn),
                external_id: Some(external_id(context)),
                encryption: parse_encryption(&row.storage_encryption_mode)?,
                kms_key_arn: row.storage_kms_key_arn.clone(),
            })
        }
        _ => Err(internal_integrity()),
    }
}

async fn target_for_version(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ExecutionContext,
    row: &EntryVersionRow,
    _platform: &S3Settings,
) -> std::result::Result<StorageTarget, AppError> {
    match row.storage_backend.as_str() {
        "platform" if row.storage_config_id.is_none() => Ok(StorageTarget {
            bucket: row.bucket_name.clone(),
            region: row.storage_region.clone(),
            prefix: row.storage_prefix.clone(),
            role_arn: None,
            external_id: None,
            encryption: parse_encryption(&row.storage_encryption_mode)?,
            kms_key_arn: row.storage_kms_key_arn.clone(),
        }),
        "organization" => {
            let id = row.storage_config_id.ok_or_else(internal_integrity)?;
            let configuration = find_storage_configuration(transaction, id)
                .await?
                .ok_or_else(internal_integrity)?;
            Ok(StorageTarget {
                bucket: row.bucket_name.clone(),
                region: row.storage_region.clone(),
                prefix: row.storage_prefix.clone(),
                role_arn: Some(configuration.role_arn),
                external_id: Some(external_id(context)),
                encryption: parse_encryption(&row.storage_encryption_mode)?,
                kms_key_arn: row.storage_kms_key_arn.clone(),
            })
        }
        _ => Err(internal_integrity()),
    }
}

async fn find_storage_configuration(
    transaction: &mut Transaction<'_, Postgres>,
    configuration_id: Uuid,
) -> std::result::Result<Option<OrganizationStorageConfigRow>, AppError> {
    sqlx::query_as::<_, OrganizationStorageConfigRow>(
        "SELECT org_id, storage_config_id, status, bucket_name, region, role_arn, \
                bucket_prefix, aws_account_id, encryption_mode, kms_key_arn, validated_at, \
                validation_failure_code, validation_failure_reason, created_by_type, \
                created_by_id, created_at, updated_at \
           FROM briefcase.organization_storage_configs \
          WHERE org_id = briefcase.current_org_id() AND storage_config_id = $1",
    )
    .bind(configuration_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)
}

fn storage_configuration_result(
    configuration: OrganizationStorageConfigRow,
) -> std::result::Result<StorageConfigurationResult, AppError> {
    let configuration_id = StorageConfigurationId::from_uuid(configuration.storage_config_id)
        .map_err(internal_error)?;
    let tested_at = configuration
        .validated_at
        .ok_or_else(|| conflict("idempotency_state"))?;
    match configuration.status.as_str() {
        "active" | "superseded" | "disabled" => Ok(StorageConfigurationResult {
            configuration_id,
            configured: true,
            tested_at,
            failure_reason: None,
        }),
        "failed" => Ok(StorageConfigurationResult {
            configuration_id,
            configured: false,
            tested_at,
            failure_reason: configuration.validation_failure_code,
        }),
        _ => Err(conflict("idempotency_state")),
    }
}

async fn find_multipart(
    transaction: &mut Transaction<'_, Postgres>,
    upload_id: Uuid,
    lock: bool,
) -> std::result::Result<Option<MultipartUploadRow>, AppError> {
    let query = if lock {
        sqlx::query_as::<_, MultipartUploadRow>(
            "SELECT org_id, upload_id, parent_entry_id, owner_type, owner_id, origin_app_id, \
                    name, content_type, declared_size_bytes, part_size_bytes, \
                    expected_part_count, storage_backend, storage_config_id, bucket_name, \
                    storage_region, storage_prefix, storage_encryption_mode, storage_kms_key_arn, \
                    object_key, provider_upload_id, status, completed_entry_id, expires_at, \
                    completed_at, aborted_at, created_at, updated_at \
               FROM briefcase.multipart_uploads \
              WHERE org_id = briefcase.current_org_id() AND upload_id = $1 FOR UPDATE",
        )
    } else {
        sqlx::query_as::<_, MultipartUploadRow>(
            "SELECT org_id, upload_id, parent_entry_id, owner_type, owner_id, origin_app_id, \
                    name, content_type, declared_size_bytes, part_size_bytes, \
                    expected_part_count, storage_backend, storage_config_id, bucket_name, \
                    storage_region, storage_prefix, storage_encryption_mode, storage_kms_key_arn, \
                    object_key, provider_upload_id, status, completed_entry_id, expires_at, \
                    completed_at, aborted_at, created_at, updated_at \
               FROM briefcase.multipart_uploads \
              WHERE org_id = briefcase.current_org_id() AND upload_id = $1",
        )
    };
    query
        .bind(upload_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)
}

async fn find_version(
    transaction: &mut Transaction<'_, Postgres>,
    entry_id: Uuid,
    version_id: Uuid,
    lock: bool,
) -> std::result::Result<Option<EntryVersionRow>, AppError> {
    let query = if lock {
        sqlx::query_as::<_, EntryVersionRow>(
            "SELECT org_id, entry_id, version_id, version_number, source, \
                    restored_from_version_id, storage_backend, storage_config_id, bucket_name, \
                    storage_region, storage_prefix, storage_encryption_mode, storage_kms_key_arn, \
                    object_key, object_version_id, etag, checksum_algorithm, checksum_type, \
                    checksum_value, size_bytes, content_type, created_by_type, created_by_id, created_at \
               FROM briefcase.entry_versions \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1 AND version_id = $2 \
              FOR UPDATE",
        )
    } else {
        sqlx::query_as::<_, EntryVersionRow>(
            "SELECT org_id, entry_id, version_id, version_number, source, \
                    restored_from_version_id, storage_backend, storage_config_id, bucket_name, \
                    storage_region, storage_prefix, storage_encryption_mode, storage_kms_key_arn, \
                    object_key, object_version_id, etag, checksum_algorithm, checksum_type, \
                    checksum_value, size_bytes, content_type, created_by_type, created_by_id, created_at \
               FROM briefcase.entry_versions \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1 AND version_id = $2",
        )
    };
    query
        .bind(entry_id)
        .bind(version_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)
}

/// Publishes uploaded bytes as a file, creating it or versioning it.
///
/// Uploading over an existing file is how a file is updated, so the bytes
/// become that file's next version and its history keeps the previous ones.
/// Replacing content requires update authority on the file itself; creating a
/// new one required create authority on the folder.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn publish_file_content(
    transaction: &mut Transaction<'_, Postgres>,
    execution: &ExecutionContext,
    tenant: &super::TenantContext,
    parent: &AuthorizableEntry,
    entry_id: EntryId,
    name: &str,
    content_type: &str,
    size: u64,
    checksum: &ObjectChecksum,
    target: &StorageTarget,
    key: &ObjectKey,
    stored: &StoredObject,
    storage: StorageReference<'_>,
) -> std::result::Result<EntryId, AppError> {
    if let Some((existing_id, kind)) =
        find_named_child(transaction, parent.entry.id, name, true).await?
    {
        if kind != "file" {
            return Err(conflict("entry_name_exists"));
        }
        return publish_next_version(
            transaction,
            execution,
            tenant,
            existing_id,
            content_type,
            size,
            checksum,
            target,
            key,
            stored,
            storage,
        )
        .await;
    }
    let actor = execution.authorization().actor();
    let (root_type, _) = boundary_columns(&parent.entry.boundary);
    let version_id = VersionId::new();
    let inserted = sqlx::query(
        "INSERT INTO briefcase.entries ( \
                org_id, entry_id, parent_id, entry_type, name, root_type, tag_id, owner_type, \
                owner_id, origin_app_id, content_type, size_bytes, current_version_id, \
                created_by_type, created_by_id, updated_by_type, updated_by_id \
         ) SELECT briefcase.current_org_id(), $1, parent.entry_id, 'file', $2, $3, parent.tag_id, \
                  $4, $5, $6, $7, $8, $9, $4, $5, $4, $5 \
             FROM briefcase.entries AS parent \
            WHERE parent.org_id = briefcase.current_org_id() AND parent.entry_id = $10 \
              AND parent.deleted_at IS NULL AND parent.entry_type = 'folder'",
    )
    .bind(entry_id.as_uuid())
    .bind(name)
    .bind(root_type)
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .bind(
        execution
            .authorization()
            .originating_application()
            .map(crate::domain::actor::ApplicationId::as_str),
    )
    .bind(content_type)
    .bind(to_i64(size)?)
    .bind(version_id.as_uuid())
    .bind(parent.entry.id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if inserted.rows_affected() != 1 {
        return Err(AppError::NotFound);
    }
    sqlx::query(
        "INSERT INTO briefcase.entry_versions ( \
                org_id, entry_id, version_id, version_number, source, storage_backend, \
                storage_config_id, bucket_name, storage_region, storage_prefix, \
                storage_encryption_mode, storage_kms_key_arn, object_key, object_version_id, etag, \
                checksum_algorithm, checksum_type, checksum_value, size_bytes, content_type, \
                created_by_type, created_by_id \
         ) VALUES (briefcase.current_org_id(), $1, $2, 1, 'upload', $3, $4, $5, $6, $7, \
                   $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)",
    )
    .bind(entry_id.as_uuid())
    .bind(version_id.as_uuid())
    .bind(storage.backend)
    .bind(storage.configuration_id)
    .bind(&target.bucket)
    .bind(&target.region)
    .bind(&target.prefix)
    .bind(encryption_name(target.encryption))
    .bind(target.kms_key_arn.as_deref())
    .bind(key.as_str())
    .bind(stored.provider_version_id.as_deref())
    .bind(stored.etag.as_deref())
    .bind(checksum_algorithm(checksum))
    .bind(checksum_type(checksum))
    .bind(checksum.encoded_value())
    .bind(to_i64(size)?)
    .bind(content_type)
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO briefcase.search_documents (org_id, entry_id, filename) \
         VALUES (briefcase.current_org_id(), $1, $2)",
    )
    .bind(entry_id.as_uuid())
    .bind(name)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    record_change(
        transaction,
        tenant,
        Some(entry_id.as_uuid()),
        "entry.file_created.v1",
        "entry",
        &entry_id.to_string(),
        json!({"parent_id": parent.entry.id, "version_id": version_id, "size": size}),
    )
    .await
    .map_err(map_metadata)?;
    Ok(entry_id)
}

/// Adds the next immutable version to an existing file and makes it current.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn publish_next_version(
    transaction: &mut Transaction<'_, Postgres>,
    execution: &ExecutionContext,
    tenant: &super::TenantContext,
    entry_id: EntryId,
    content_type: &str,
    size: u64,
    checksum: &ObjectChecksum,
    target: &StorageTarget,
    key: &ObjectKey,
    stored: &StoredObject,
    storage: StorageReference<'_>,
) -> std::result::Result<EntryId, AppError> {
    let entry = load_entry(transaction, execution, entry_id, false, false)
        .await
        .map_err(map_metadata)?
        .ok_or(AppError::NotFound)?;
    require_entry_capability(&entry, execution, Capability::WriteContent)?;

    let actor = execution.authorization().actor();
    let version_id = VersionId::new();
    let version_number = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(version_number), 0) + 1 FROM briefcase.entry_versions \
          WHERE org_id = briefcase.current_org_id() AND entry_id = $1",
    )
    .bind(entry_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO briefcase.entry_versions ( \
                org_id, entry_id, version_id, version_number, source, storage_backend, \
                storage_config_id, bucket_name, storage_region, storage_prefix, \
                storage_encryption_mode, storage_kms_key_arn, object_key, object_version_id, etag, \
                checksum_algorithm, checksum_type, checksum_value, size_bytes, content_type, \
                created_by_type, created_by_id \
         ) VALUES (briefcase.current_org_id(), $1, $2, $3, 'upload', $4, $5, $6, $7, $8, \
                   $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)",
    )
    .bind(entry_id.as_uuid())
    .bind(version_id.as_uuid())
    .bind(version_number)
    .bind(storage.backend)
    .bind(storage.configuration_id)
    .bind(&target.bucket)
    .bind(&target.region)
    .bind(&target.prefix)
    .bind(encryption_name(target.encryption))
    .bind(target.kms_key_arn.as_deref())
    .bind(key.as_str())
    .bind(stored.provider_version_id.as_deref())
    .bind(stored.etag.as_deref())
    .bind(checksum_algorithm(checksum))
    .bind(checksum_type(checksum))
    .bind(checksum.encoded_value())
    .bind(to_i64(size)?)
    .bind(content_type)
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;

    let updated = sqlx::query(
        "UPDATE briefcase.entries \
            SET current_version_id = $2, content_type = $3, size_bytes = $4, \
                updated_by_type = $5, updated_by_id = $6 \
          WHERE org_id = briefcase.current_org_id() AND entry_id = $1 \
            AND entry_type = 'file' AND deleted_at IS NULL",
    )
    .bind(entry_id.as_uuid())
    .bind(version_id.as_uuid())
    .bind(content_type)
    .bind(to_i64(size)?)
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if updated.rows_affected() != 1 {
        return Err(AppError::NotFound);
    }

    // The indexed content belongs to the previous version, so it is retired
    // and the worker re-extracts from the new one.
    sqlx::query(
        "UPDATE briefcase.search_documents \
            SET extracted_content = NULL, extraction_status = 'pending', \
                extraction_error_code = NULL, indexed_at = NULL \
          WHERE org_id = briefcase.current_org_id() AND entry_id = $1",
    )
    .bind(entry_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;

    record_change(
        transaction,
        tenant,
        Some(entry_id.as_uuid()),
        "entry.version_created.v1",
        "entry",
        &entry_id.to_string(),
        json!({"version_id": version_id, "version_number": version_number, "size": size}),
    )
    .await
    .map_err(map_metadata)?;
    Ok(entry_id)
}

fn keyed_metadata(key: &IdempotencyKey, hash: [u8; 32]) -> MutationMetadata {
    MutationMetadata::new(Some(key.clone()), hash)
}

async fn ensure_idempotency_in_progress(
    transaction: &mut Transaction<'_, Postgres>,
    context: &super::TenantContext,
    operation: &'static str,
    key: &IdempotencyKey,
    request_hash: &[u8; 32],
) -> std::result::Result<(), AppError> {
    #[derive(sqlx::FromRow)]
    struct State {
        request_hash: Vec<u8>,
        status: String,
    }
    let state = sqlx::query_as::<_, State>(
        "SELECT request_hash, status FROM briefcase.idempotency_records \
          WHERE org_id = briefcase.current_org_id() \
            AND actor_type = $1 AND actor_id = $2 AND origin_app_id = $3 \
            AND operation = $4 AND idempotency_key = $5 FOR UPDATE",
    )
    .bind(context.actor_type())
    .bind(context.actor_id())
    .bind(context.origin_app_id().unwrap_or_default())
    .bind(operation)
    .bind(key.as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| conflict("idempotency_state"))?;
    if state.status != "in_progress" || state.request_hash.as_slice() != request_hash {
        return Err(conflict("idempotency_state"));
    }
    Ok(())
}

async fn extend_version_restore_lease(
    transaction: &mut Transaction<'_, Postgres>,
    context: &super::TenantContext,
    key: &IdempotencyKey,
    request_hash: &[u8; 32],
) -> std::result::Result<(), AppError> {
    let updated = sqlx::query(
        "UPDATE briefcase.idempotency_records \
            SET locked_until = clock_timestamp() + make_interval(secs => $6::double precision), \
                expires_at = GREATEST(expires_at, clock_timestamp() + interval '24 hours') \
          WHERE org_id = briefcase.current_org_id() \
            AND actor_type = $1 AND actor_id = $2 AND origin_app_id = $3 \
            AND operation = $4 AND idempotency_key = $5 \
            AND request_hash = $7 AND status = 'in_progress'",
    )
    .bind(context.actor_type())
    .bind(context.actor_id())
    .bind(context.origin_app_id().unwrap_or_default())
    .bind(VERSION_RESTORE_OPERATION)
    .bind(key.as_str())
    .bind(RESTORE_LEASE_DURATION.as_secs_f64())
    .bind(request_hash.as_slice())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if updated.rows_affected() != 1 {
        return Err(conflict("idempotency_state"));
    }
    Ok(())
}

fn multipart_plan(row: &MultipartUploadRow) -> std::result::Result<MultipartPlan, AppError> {
    let plan = MultipartPlan::for_file_size(to_u64(row.declared_size_bytes)?)
        .map_err(|_| internal_integrity())?;
    if plan.part_size() != to_u64(row.part_size_bytes)?
        || plan.part_count() != u32::try_from(row.expected_part_count).map_err(internal_error)?
    {
        return Err(internal_integrity());
    }
    Ok(plan)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MultipartAbortDisposition {
    Abort,
    AlreadyStopped,
    Reject,
}

fn multipart_abort_disposition(
    status: &str,
) -> std::result::Result<MultipartAbortDisposition, AppError> {
    match status {
        "initiated" | "uploading" => Ok(MultipartAbortDisposition::Abort),
        "aborted" | "expired" => Ok(MultipartAbortDisposition::AlreadyStopped),
        "completing" | "completed" => Ok(MultipartAbortDisposition::Reject),
        _ => Err(internal_integrity()),
    }
}

fn multipart_completion_expired(
    status: &str,
    expires_at: OffsetDateTime,
    now: OffsetDateTime,
) -> bool {
    matches!(status, "initiated" | "uploading") && expires_at <= now
}

fn multipart_receipt(row: &MultipartUploadRow) -> std::result::Result<MultipartReceipt, AppError> {
    let plan = multipart_plan(row)?;
    Ok(MultipartReceipt {
        upload_id: MultipartUploadId::from_uuid(row.upload_id).map_err(internal_error)?,
        part_size: plan.part_size(),
        part_count: plan.part_count(),
        expires_at: row.expires_at,
    })
}

fn authorize_multipart_owner(
    context: &ExecutionContext,
    row: &MultipartUploadRow,
) -> std::result::Result<(), AppError> {
    let actor = context.authorization().actor();
    let current_origin = context
        .authorization()
        .originating_application()
        .map(crate::domain::actor::ApplicationId::as_str);
    if row.owner_type == actor_kind(actor.kind())
        && row.owner_id == actor.id().as_str()
        && row.origin_app_id.as_deref() == current_origin
    {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}

fn completion_parts(
    plan: MultipartPlan,
    stored: &[MultipartPartRow],
    client: &[ClientCompletedPart],
) -> std::result::Result<Vec<CompletedPart>, AppError> {
    if stored.len() != usize::try_from(plan.part_count()).map_err(internal_error)?
        || client.len() != stored.len()
    {
        return Err(AppError::validation("invalid_multipart_completion"));
    }
    let mut completed = Vec::with_capacity(stored.len());
    for (stored, supplied) in stored.iter().zip(client) {
        let part_number = u32::try_from(stored.part_number).map_err(internal_error)?;
        if supplied.part_number != part_number || supplied.etag != stored.etag {
            return Err(AppError::validation("invalid_multipart_completion"));
        }
        let expected = plan
            .expected_part_size(part_number)
            .map_err(|_| AppError::validation("invalid_multipart_completion"))?;
        if expected != to_u64(stored.size_bytes)? {
            return Err(AppError::validation("invalid_multipart_completion"));
        }
        let checksum = checksum_array(&stored.checksum_sha256)?;
        completed.push(
            CompletedPart::new(part_number, stored.etag.clone(), expected, checksum)
                .map_err(|_| AppError::validation("invalid_multipart_completion"))?,
        );
    }
    Ok(completed)
}

fn validate_stored(
    stored: &StoredObject,
    key: &ObjectKey,
    expected_size: u64,
) -> std::result::Result<(), AppError> {
    if &stored.key != key || stored.size != expected_size {
        Err(conflict("stored_object_mismatch"))
    } else {
        Ok(())
    }
}

fn stored_checksum(
    stored: &StoredObject,
    expected_type: ObjectChecksumType,
) -> std::result::Result<ObjectChecksum, AppError> {
    let checksum = verified_stored_checksum(stored)?;
    if checksum.checksum_type() != expected_type {
        return Err(conflict("stored_object_checksum_mismatch"));
    }
    Ok(checksum)
}

fn verified_stored_checksum(
    stored: &StoredObject,
) -> std::result::Result<ObjectChecksum, AppError> {
    let checksum = stored.checksum.clone().ok_or(AppError::Internal {
        category: "object_checksum_unavailable",
    })?;
    if checksum.algorithm() != ObjectChecksumAlgorithm::Sha256 {
        return Err(conflict("stored_object_checksum_mismatch"));
    }
    Ok(checksum)
}

fn checksum_array(value: &[u8]) -> std::result::Result<[u8; 32], AppError> {
    value.try_into().map_err(|_| internal_integrity())
}

fn object_checksum(
    algorithm: &str,
    checksum_kind: &str,
    value: String,
) -> std::result::Result<ObjectChecksum, AppError> {
    let algorithm = match algorithm {
        "sha256" => ObjectChecksumAlgorithm::Sha256,
        _ => return Err(internal_integrity()),
    };
    let checksum_kind = match checksum_kind {
        "full_object" => ObjectChecksumType::FullObject,
        "composite" => ObjectChecksumType::Composite,
        _ => return Err(internal_integrity()),
    };
    ObjectChecksum::new(algorithm, checksum_kind, value).map_err(internal_error)
}

fn checksum_algorithm(checksum: &ObjectChecksum) -> &'static str {
    match checksum.algorithm() {
        ObjectChecksumAlgorithm::Sha256 => "sha256",
    }
}

fn checksum_type(checksum: &ObjectChecksum) -> &'static str {
    match checksum.checksum_type() {
        ObjectChecksumType::FullObject => "full_object",
        ObjectChecksumType::Composite => "composite",
    }
}

async fn lock_storage_configuration(
    transaction: &mut Transaction<'_, Postgres>,
    configuration_id: Uuid,
) -> std::result::Result<(), AppError> {
    let exists = sqlx::query_scalar::<_, Uuid>(
        "SELECT storage_config_id FROM briefcase.organization_storage_configs \
          WHERE org_id = briefcase.current_org_id() AND storage_config_id = $1 FOR UPDATE",
    )
    .bind(configuration_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    if exists.is_some() {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}

fn encryption_name(encryption: EncryptionMode) -> &'static str {
    match encryption {
        EncryptionMode::SseS3 => "sse_s3",
        EncryptionMode::SseKms => "sse_kms",
    }
}

fn parse_encryption(value: &str) -> std::result::Result<EncryptionMode, AppError> {
    match value {
        "sse_s3" => Ok(EncryptionMode::SseS3),
        "sse_kms" => Ok(EncryptionMode::SseKms),
        _ => Err(internal_integrity()),
    }
}

fn external_id(context: &ExecutionContext) -> String {
    crate::infrastructure::s3::organization_storage_external_id(
        context.authorization().organization_id().as_str(),
    )
}

async fn enqueue_multipart_abort_cleanup(
    transaction: &mut Transaction<'_, Postgres>,
    upload: &MultipartUploadRow,
    target: &StorageTarget,
) -> std::result::Result<(), AppError> {
    sqlx::query(
        "INSERT INTO briefcase.object_cleanup_jobs ( \
                org_id, cleanup_id, cleanup_kind, source_upload_id, storage_backend, \
                storage_config_id, bucket_name, storage_region, storage_prefix, \
                storage_role_arn, storage_encryption_mode, storage_kms_key_arn, object_key, \
                provider_upload_id \
         ) VALUES (briefcase.current_org_id(), $1, 'multipart_abort', $2, $3, $4, $5, \
                   $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(Uuid::now_v7())
    .bind(upload.upload_id)
    .bind(&upload.storage_backend)
    .bind(upload.storage_config_id)
    .bind(&target.bucket)
    .bind(&target.region)
    .bind(&target.prefix)
    .bind(target.role_arn.as_deref())
    .bind(encryption_name(target.encryption))
    .bind(target.kms_key_arn.as_deref())
    .bind(&upload.object_key)
    .bind(&upload.provider_upload_id)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

fn object_key(value: String) -> std::result::Result<ObjectKey, AppError> {
    ObjectKey::new(value).map_err(internal_error)
}

fn to_i64(value: u64) -> std::result::Result<i64, AppError> {
    i64::try_from(value).map_err(internal_error)
}

fn to_i32(value: u32) -> std::result::Result<i32, AppError> {
    i32::try_from(value).map_err(internal_error)
}

fn to_u64(value: i64) -> std::result::Result<u64, AppError> {
    u64::try_from(value).map_err(internal_error)
}

fn conflict(code: &'static str) -> AppError {
    AppError::conflict(code)
}

fn database_error(error: sqlx::Error) -> AppError {
    map_metadata(map_sql(error))
}

fn unknown_commit_error(_error: sqlx::Error, operation: &'static str) -> AppError {
    AppError::DatabaseCommitOutcomeUnknown { operation }
}

fn map_metadata(error: crate::application::service::MetadataRepositoryError) -> AppError {
    match error {
        crate::application::service::MetadataRepositoryError::NotFound => AppError::NotFound,
        crate::application::service::MetadataRepositoryError::Conflict => {
            conflict("metadata_conflict")
        }
        crate::application::service::MetadataRepositoryError::Unavailable => {
            AppError::DependencyUnavailable {
                dependency: "postgresql",
            }
        }
        crate::application::service::MetadataRepositoryError::Internal(source) => {
            drop(source);
            AppError::Internal {
                category: "database_operation",
            }
        }
    }
}

fn internal_error(_error: impl std::error::Error) -> AppError {
    AppError::Internal {
        category: "persisted_data",
    }
}

fn internal_integrity() -> AppError {
    AppError::Internal {
        category: "persisted_data",
    }
}

#[cfg(test)]
mod tests {
    use time::{Duration, OffsetDateTime};

    use super::{
        MULTIPART_INIT_OPERATION, MultipartAbortDisposition, SMALL_UPLOAD_OPERATION,
        VERSION_RESTORE_OPERATION, multipart_abort_disposition, multipart_completion_expired,
        unknown_commit_error,
    };
    use crate::error::AppError;

    #[test]
    fn publication_commit_errors_keep_a_static_operation_identity() {
        for operation in [
            SMALL_UPLOAD_OPERATION,
            MULTIPART_INIT_OPERATION,
            VERSION_RESTORE_OPERATION,
        ] {
            assert!(matches!(
                unknown_commit_error(sqlx::Error::PoolClosed, operation),
                AppError::DatabaseCommitOutcomeUnknown { operation: actual }
                    if actual == operation
            ));
        }
    }

    #[test]
    fn durable_multipart_completion_cannot_be_aborted() {
        assert!(matches!(
            multipart_abort_disposition("completing"),
            Ok(MultipartAbortDisposition::Reject)
        ));
        assert!(matches!(
            multipart_abort_disposition("completed"),
            Ok(MultipartAbortDisposition::Reject)
        ));
    }

    #[test]
    fn durable_multipart_completion_survives_session_expiry() {
        let now = OffsetDateTime::now_utc();
        let expired_at = now - Duration::minutes(1);

        assert!(multipart_completion_expired("uploading", expired_at, now));
        assert!(!multipart_completion_expired("completing", expired_at, now));
        assert!(!multipart_completion_expired("completed", expired_at, now));
    }
}
