//! Durable, identity-bound delegated staging without cached authorization.

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sqlx::{Postgres, Transaction};
use subtle::ConstantTimeEq as _;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    application::{
        context::{ExecutionContext, TestingEnvironmentContext},
        delegated_upload::{
            DelegatedUploadCommit, DelegatedUploadRepository, DelegatedUploadScope,
            DelegatedUploadState, DelegatedUploadStatus, DelegatedUploadTransfer,
            PrepareDelegatedUploadCommit, ReserveDelegatedUpload,
        },
        ports::{
            ObjectChecksum, ObjectChecksumAlgorithm, ObjectChecksumType, ObjectKey, StorageTarget,
            StoredObject,
        },
    },
    domain::{ids::EntryId, multipart::MAX_UPLOAD_BYTES, permission::Capability},
    error::AppError,
    infrastructure::postgres::{TenantContext, quota},
};

use super::{
    PostgresContentRepository, StorageReference, checksum_type, conflict, content_begin,
    database_error, encryption_name, find_named_child, identify_storage_target, internal_integrity,
    load_entry, map_metadata, object_key, parse_encryption, publish_file_content,
    require_entry_capability, require_upload_parent, resolve_write_target, to_i64, to_u64,
    unknown_commit_error, validate_stored,
};

/// Bound metadata and unaccounted bytes even for zero-byte malicious reserves.
const MAX_PENDING_RESERVATIONS: i64 = 32;
const STAGING_LOCK_NAMESPACE: i32 = 1_618_031;

#[derive(Debug, sqlx::FromRow)]
struct UploadRow {
    upload_id: Uuid,
    operation_id: Uuid,
    testing_environment_id: Option<Uuid>,
    control_version: i64,
    request_hash: Vec<u8>,
    capability_hash: Option<Vec<u8>>,
    parent_id: Uuid,
    parent_path: String,
    name: String,
    content_type: String,
    size_bytes: i64,
    expected_sha256: Vec<u8>,
    destination_entry_id: Option<Uuid>,
    proposed_entry_id: Uuid,
    status: String,
    expires_at: OffsetDateTime,
    lease_token: Option<Uuid>,
    lease_expires_at: Option<OffsetDateTime>,
    provider_write_started: bool,
    provider_completion_started: bool,
    provider_deadline_at: Option<OffsetDateTime>,
    provider_upload_id: Option<String>,
    storage_backend: String,
    storage_config_id: Option<Uuid>,
    bucket_name: String,
    storage_region: String,
    storage_prefix: String,
    storage_role_arn: Option<String>,
    storage_external_id: Option<String>,
    storage_encryption_mode: String,
    storage_kms_key_arn: Option<String>,
    object_key: String,
    object_version_id: Option<String>,
    object_etag: Option<String>,
    object_checksum_type: Option<String>,
    object_checksum_value: Option<String>,
    published_entry_id: Option<Uuid>,
}

impl UploadRow {
    fn status(&self) -> Result<DelegatedUploadStatus, AppError> {
        let state = match self.status.as_str() {
            "reserved" => DelegatedUploadState::Reserved,
            "receiving" => DelegatedUploadState::Receiving,
            "staged" => DelegatedUploadState::Staged,
            "committed" => DelegatedUploadState::Committed,
            "cancelled" => DelegatedUploadState::Cancelled,
            "expired" => DelegatedUploadState::Expired,
            "cleanup_pending" => DelegatedUploadState::CleanupPending,
            _ => return Err(internal_integrity()),
        };
        Ok(DelegatedUploadStatus {
            operation_id: self.operation_id,
            upload_id: self.upload_id,
            state,
            expires_at: self.expires_at,
            published_entry_id: self.published_entry_id,
        })
    }

    fn target(&self) -> Result<StorageTarget, AppError> {
        Ok(StorageTarget {
            bucket: self.bucket_name.clone(),
            region: self.storage_region.clone(),
            prefix: self.storage_prefix.clone(),
            role_arn: self.storage_role_arn.clone(),
            external_id: self.storage_external_id.clone(),
            encryption: parse_encryption(&self.storage_encryption_mode)?,
            kms_key_arn: self.storage_kms_key_arn.clone(),
        })
    }

    fn key(&self) -> Result<ObjectKey, AppError> {
        object_key(self.object_key.clone())
    }

    fn sha256(&self) -> Result<[u8; 32], AppError> {
        self.expected_sha256
            .as_slice()
            .try_into()
            .map_err(|_| internal_integrity())
    }

    fn stored(&self) -> Result<StoredObject, AppError> {
        let kind = match self.object_checksum_type.as_deref() {
            Some("full_object") => ObjectChecksumType::FullObject,
            Some("composite") => ObjectChecksumType::Composite,
            _ => return Err(internal_integrity()),
        };
        let checksum = ObjectChecksum::new(
            ObjectChecksumAlgorithm::Sha256,
            kind,
            self.object_checksum_value
                .clone()
                .ok_or_else(internal_integrity)?,
        )
        .map_err(|_| internal_integrity())?;
        Ok(StoredObject {
            key: self.key()?,
            etag: self.object_etag.clone(),
            provider_version_id: self.object_version_id.clone(),
            size: to_u64(self.size_bytes)?,
            checksum: Some(checksum),
        })
    }
}

#[async_trait]
impl DelegatedUploadRepository for PostgresContentRepository {
    async fn reserve(
        &self,
        context: &ExecutionContext,
        command: &ReserveDelegatedUpload,
        capability_hash: &[u8; 32],
        expires_at: OffsetDateTime,
    ) -> Result<DelegatedUploadStatus, AppError> {
        let identity = Identity::new(context)?;
        if command.operation_id.is_nil() || command.size > MAX_UPLOAD_BYTES {
            return Err(AppError::validation("invalid_delegated_upload"));
        }
        let mut request = content_begin(&self.repository, context).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext(briefcase.current_org_id()), $1)")
            .bind(STAGING_LOCK_NAMESPACE)
            .execute(&mut *request.transaction)
            .await
            .map_err(database_error)?;
        if let Some(mut row) =
            find_operation(&mut request.transaction, &identity, command.operation_id).await?
        {
            if !bool::from(
                row.request_hash
                    .as_slice()
                    .ct_eq(command.request_hash.as_slice()),
            ) || row.parent_path != command.parent_path
                || row.parent_id != command.parent_id.as_uuid()
            {
                return Err(conflict("idempotency_key_reused"));
            }
            self.expire(&mut request.transaction, &mut row).await?;
            if row.status == "reserved" {
                authorize_destination(&mut request.transaction, context, &row).await?;
                sqlx::query("UPDATE briefcase.delegated_uploads SET capability_hash = $2 WHERE org_id = briefcase.current_org_id() AND upload_id = $1")
                    .bind(row.upload_id).bind(capability_hash.as_slice()).execute(&mut *request.transaction).await.map_err(database_error)?;
            } else if row.status == "committed" {
                authorize_committed(&mut request.transaction, context, &row).await?;
            }
            let status = row.status()?;
            request
                .transaction
                .commit()
                .await
                .map_err(|error| unknown_commit_error(error, "reserve_delegated_upload"))?;
            return Ok(status);
        }
        require_upload_parent(
            &mut request.transaction,
            context,
            command.parent_id,
            command.name.as_str(),
        )
        .await?;
        let destination = find_named_child(
            &mut request.transaction,
            command.parent_id,
            command.name.as_str(),
            true,
        )
        .await?
        .map(|(id, _)| id.as_uuid());
        let (count, bytes) = sqlx::query_as::<_, (i64, i64)>(
            "SELECT count(*), COALESCE(sum(size_bytes), 0)::bigint FROM briefcase.delegated_uploads WHERE org_id = briefcase.current_org_id() AND status IN ('reserved','receiving','staged','cleanup_pending')",
        ).fetch_one(&mut *request.transaction).await.map_err(database_error)?;
        if count >= MAX_PENDING_RESERVATIONS
            || to_u64(bytes)?.saturating_add(command.size) > MAX_UPLOAD_BYTES
        {
            return Err(conflict("delegated_upload_staging_limit_exhausted"));
        }
        quota::reserve_delegated_upload(&mut request.transaction, command.size).await?;
        let now = database_now(&mut request.transaction).await?;
        let expires_at = expires_at.min(now + time::Duration::hours(24));
        if expires_at <= now {
            return Err(AppError::validation("invalid_delegated_upload_expiration"));
        }
        let (target, backend, configuration_id) = resolve_write_target(
            &mut request.transaction,
            context,
            self.platform_target(context),
        )
        .await?;
        let upload_id = Uuid::now_v7();
        let proposed = EntryId::new();
        let key = object_key(format!("delegated/{upload_id}/object"))?;
        sqlx::query(
            "INSERT INTO briefcase.delegated_uploads (org_id,upload_id,operation_id,testing_environment_id,control_version,iam_organization_id,iam_principal_id,iam_membership_id,actor_type,actor_id,origin_app_id,request_hash,capability_hash,parent_id,parent_path,name,content_type,size_bytes,expected_sha256,destination_entry_id,proposed_entry_id,expires_at,storage_backend,storage_config_id,bucket_name,storage_region,storage_prefix,storage_role_arn,storage_external_id,storage_encryption_mode,storage_kms_key_arn,object_key) VALUES (briefcase.current_org_id(),$1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31)",
        ).bind(upload_id).bind(command.operation_id).bind(context.testing_environment().map(TestingEnvironmentContext::id))
            .bind(identity.control_version).bind(identity.organization).bind(identity.principal).bind(identity.membership)
            .bind(identity.actor_type).bind(identity.actor_id).bind(identity.application)
            .bind(command.request_hash.as_slice()).bind(capability_hash.as_slice()).bind(command.parent_id.as_uuid())
            .bind(&command.parent_path).bind(command.name.as_str()).bind(&command.content_type).bind(to_i64(command.size)?)
            .bind(command.sha256.as_slice()).bind(destination).bind(proposed.as_uuid()).bind(expires_at)
            .bind(backend).bind(configuration_id).bind(&target.bucket).bind(&target.region).bind(&target.prefix)
            .bind(target.role_arn.as_deref()).bind(target.external_id.as_deref()).bind(encryption_name(target.encryption))
            .bind(target.kms_key_arn.as_deref()).bind(key.as_str()).execute(&mut *request.transaction).await.map_err(database_error)?;
        request
            .transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, "reserve_delegated_upload"))?;
        Ok(DelegatedUploadStatus {
            operation_id: command.operation_id,
            upload_id,
            state: DelegatedUploadState::Reserved,
            expires_at,
            published_entry_id: None,
        })
    }

    async fn status(
        &self,
        context: &ExecutionContext,
        operation_id: Uuid,
    ) -> Result<DelegatedUploadStatus, AppError> {
        let identity = Identity::new(context)?;
        let mut request = content_begin(&self.repository, context).await?;
        let mut row = find_operation(&mut request.transaction, &identity, operation_id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.expire(&mut request.transaction, &mut row).await?;
        let status = row.status()?;
        request
            .transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, "describe_delegated_upload"))?;
        Ok(status)
    }

    async fn claim_transfer(
        &self,
        scope: &DelegatedUploadScope,
        upload_id: Uuid,
        capability_hash: &[u8; 32],
        lease_seconds: i64,
    ) -> Result<DelegatedUploadTransfer, AppError> {
        if !(1..=86_400).contains(&lease_seconds) {
            return Err(AppError::validation("invalid_delegated_upload_lease"));
        }
        let mut transaction = self.scope_begin(scope).await?;
        let mut row = find_upload(&mut transaction, upload_id)
            .await?
            .ok_or(AppError::NotFound)?;
        validate_scope(&row, scope)?;
        let expected = row.capability_hash.as_deref().unwrap_or(&[0; 32]);
        if !bool::from(expected.ct_eq(capability_hash.as_slice())) || row.capability_hash.is_none()
        {
            return Err(AppError::NotFound);
        }
        self.expire(&mut transaction, &mut row).await?;
        if row.status != "reserved" {
            return Err(conflict("delegated_upload_not_reserved"));
        }
        let now = database_now(&mut transaction).await?;
        let deadline = now + time::Duration::seconds(lease_seconds);
        // Do not shorten the provider quiescence budget near reservation TTL.
        // The caller can cancel this intent and reserve a fresh operation.
        if deadline > row.expires_at {
            return Err(conflict("delegated_upload_insufficient_lifetime"));
        }
        let lease_token = Uuid::now_v7();
        sqlx::query("UPDATE briefcase.delegated_uploads SET status='receiving',lease_token=$2,lease_expires_at=$3 WHERE org_id=briefcase.current_org_id() AND upload_id=$1")
            .bind(upload_id).bind(lease_token).bind(deadline).execute(&mut *transaction).await.map_err(database_error)?;
        let transfer = DelegatedUploadTransfer {
            scope: scope.clone(),
            operation_id: row.operation_id,
            upload_id,
            lease_token,
            lease_expires_at: deadline,
            target: row.target()?,
            key: row.key()?,
            content_type: row.content_type.clone(),
            size: to_u64(row.size_bytes)?,
            sha256: row.sha256()?,
        };
        transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, "claim_delegated_upload_transfer"))?;
        Ok(transfer)
    }

    async fn before_provider_write(
        &self,
        transfer: &DelegatedUploadTransfer,
    ) -> Result<(), AppError> {
        let mut transaction = self.scope_begin(&transfer.scope).await?;
        let row = self.require_writer(&mut transaction, transfer).await?;
        validate_transfer(&row, transfer)?;
        sqlx::query("UPDATE briefcase.delegated_uploads SET provider_write_started=true,capability_hash=NULL,provider_deadline_at=clock_timestamp()+($2::bigint * interval '1 millisecond') WHERE org_id=briefcase.current_org_id() AND upload_id=$1")
            .bind(transfer.upload_id).bind(self.provider_timeout_millis()?).execute(&mut *transaction).await.map_err(database_error)?;
        transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, "begin_delegated_provider_write"))
    }

    async fn before_provider_completion(
        &self,
        transfer: &DelegatedUploadTransfer,
    ) -> Result<(), AppError> {
        let mut transaction = self.scope_begin(&transfer.scope).await?;
        let row = self.require_writer(&mut transaction, transfer).await?;
        validate_transfer(&row, transfer)?;
        sqlx::query("UPDATE briefcase.delegated_uploads SET provider_write_started=true,provider_completion_started=true,capability_hash=NULL,provider_deadline_at=clock_timestamp()+($2::bigint * interval '1 millisecond') WHERE org_id=briefcase.current_org_id() AND upload_id=$1")
            .bind(transfer.upload_id).bind(self.provider_timeout_millis()?).execute(&mut *transaction).await.map_err(database_error)?;
        transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, "begin_delegated_provider_completion"))
    }

    async fn record_multipart(
        &self,
        transfer: &DelegatedUploadTransfer,
        provider_upload_id: &str,
    ) -> Result<(), AppError> {
        if provider_upload_id.is_empty() || provider_upload_id.len() > 2048 {
            return Err(internal_integrity());
        }
        let mut transaction = self.scope_begin(&transfer.scope).await?;
        let row = self.require_writer(&mut transaction, transfer).await?;
        validate_transfer(&row, transfer)?;
        if !row.provider_write_started
            || row
                .provider_upload_id
                .as_ref()
                .is_some_and(|id| id != provider_upload_id)
        {
            return Err(conflict("delegated_upload_provider_changed"));
        }
        sqlx::query("UPDATE briefcase.delegated_uploads SET provider_upload_id=$2 WHERE org_id=briefcase.current_org_id() AND upload_id=$1")
            .bind(transfer.upload_id).bind(provider_upload_id).execute(&mut *transaction).await.map_err(database_error)?;
        transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, "record_delegated_multipart"))
    }

    async fn complete_transfer(
        &self,
        transfer: &DelegatedUploadTransfer,
        stored: &StoredObject,
    ) -> Result<DelegatedUploadStatus, AppError> {
        let mut transaction = self.scope_begin(&transfer.scope).await?;
        let mut row = self.require_writer(&mut transaction, transfer).await?;
        validate_transfer(&row, transfer)?;
        validate_object(&row, stored)?;
        if !row.provider_write_started || !row.provider_completion_started {
            return Err(conflict("delegated_upload_provider_not_started"));
        }
        let checksum = stored.checksum.as_ref().ok_or_else(internal_integrity)?;
        sqlx::query("UPDATE briefcase.delegated_uploads SET status='staged',capability_hash=NULL,lease_token=NULL,lease_expires_at=NULL,object_version_id=$2,object_etag=$3,object_checksum_type=$4,object_checksum_value=$5 WHERE org_id=briefcase.current_org_id() AND upload_id=$1")
            .bind(row.upload_id).bind(stored.provider_version_id.as_deref()).bind(stored.etag.as_deref())
            .bind(checksum_type(checksum)).bind(checksum.encoded_value()).execute(&mut *transaction).await.map_err(database_error)?;
        "staged".clone_into(&mut row.status);
        let status = row.status()?;
        transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, "complete_delegated_transfer"))?;
        Ok(status)
    }

    async fn fail_transfer(
        &self,
        transfer: &DelegatedUploadTransfer,
        provider_upload_id: Option<&str>,
        stored: Option<&StoredObject>,
    ) -> Result<(), AppError> {
        let mut transaction = self.scope_begin(&transfer.scope).await?;
        let Some(row) = find_upload(&mut transaction, transfer.upload_id).await? else {
            return Ok(());
        };
        validate_scope(&row, &transfer.scope)?;
        // An unknown completed transaction may already have staged/published.
        // Never turn its successful result into a destructive cleanup request.
        if row.status != "receiving" || row.lease_token != Some(transfer.lease_token) {
            return Ok(());
        }
        validate_transfer(&row, transfer)?;
        if let Some(stored) = stored {
            validate_stored(stored, &row.key()?, to_u64(row.size_bytes)?)?;
        }
        if let Some(id) = provider_upload_id
            && (id.is_empty()
                || id.len() > 2048
                || row.provider_upload_id.as_ref().is_some_and(|old| old != id))
        {
            return Err(internal_integrity());
        }
        let possible_bytes =
            row.provider_write_started || provider_upload_id.is_some() || stored.is_some();
        let now = database_now(&mut transaction).await?;
        if possible_bytes {
            let provider_deadline = row
                .provider_deadline_at
                .unwrap_or(now + time::Duration::milliseconds(self.provider_timeout_millis()?));
            let cleanup_after = Self::cleanup_deadline(&row, now.max(provider_deadline))?;
            sqlx::query("UPDATE briefcase.delegated_uploads SET status='cleanup_pending',capability_hash=NULL,lease_token=NULL,lease_expires_at=NULL,provider_write_started=true,cleanup_after=$2,provider_upload_id=COALESCE($3,provider_upload_id),object_version_id=COALESCE($4,object_version_id),object_etag=COALESCE($5,object_etag),provider_completion_started=provider_completion_started OR $6,provider_deadline_at=$7 WHERE org_id=briefcase.current_org_id() AND upload_id=$1")
                .bind(row.upload_id).bind(cleanup_after).bind(provider_upload_id)
                .bind(stored.and_then(|object| object.provider_version_id.as_deref())).bind(stored.and_then(|object| object.etag.as_deref()))
                .bind(stored.is_some())
                .bind(provider_deadline)
                .execute(&mut *transaction).await.map_err(database_error)?;
        } else {
            let next = if row.expires_at > now {
                "reserved"
            } else {
                "expired"
            };
            sqlx::query("UPDATE briefcase.delegated_uploads SET status=$2,capability_hash=CASE WHEN $2='reserved' THEN capability_hash ELSE NULL END,lease_token=NULL,lease_expires_at=NULL WHERE org_id=briefcase.current_org_id() AND upload_id=$1")
                .bind(row.upload_id).bind(next).execute(&mut *transaction).await.map_err(database_error)?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, "fail_delegated_upload_transfer"))
    }

    async fn prepare_commit(
        &self,
        context: &ExecutionContext,
        operation_id: Uuid,
        upload_id: Uuid,
    ) -> Result<PrepareDelegatedUploadCommit, AppError> {
        let identity = Identity::new(context)?;
        let mut request = content_begin(&self.repository, context).await?;
        let mut row = find_operation(&mut request.transaction, &identity, operation_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if row.upload_id != upload_id {
            return Err(AppError::NotFound);
        }
        self.expire(&mut request.transaction, &mut row).await?;
        let prepared = if row.status == "committed" {
            authorize_committed(&mut request.transaction, context, &row).await?;
            PrepareDelegatedUploadCommit::Committed(row.status()?)
        } else if row.status == "staged" {
            authorize_destination(&mut request.transaction, context, &row).await?;
            PrepareDelegatedUploadCommit::Staged(Box::new(DelegatedUploadCommit {
                status: row.status()?,
                target: row.target()?,
                stored: row.stored()?,
            }))
        } else {
            return Err(conflict("delegated_upload_not_staged"));
        };
        request
            .transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, "prepare_delegated_upload_commit"))?;
        Ok(prepared)
    }

    async fn commit(
        &self,
        context: &ExecutionContext,
        operation_id: Uuid,
        upload_id: Uuid,
        stored: &StoredObject,
    ) -> Result<DelegatedUploadStatus, AppError> {
        let identity = Identity::new(context)?;
        let mut request = content_begin(&self.repository, context).await?;
        let mut row = find_operation(&mut request.transaction, &identity, operation_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if row.upload_id != upload_id {
            return Err(AppError::NotFound);
        }
        self.expire(&mut request.transaction, &mut row).await?;
        if row.status == "committed" {
            authorize_committed(&mut request.transaction, context, &row).await?;
            let status = row.status()?;
            request.transaction.commit().await.map_err(database_error)?;
            return Ok(status);
        }
        if row.status != "staged" {
            return Err(conflict("delegated_upload_not_staged"));
        }
        if row.stored()? != *stored {
            return Err(conflict("delegated_upload_object_changed"));
        }
        let parent = authorize_destination(&mut request.transaction, context, &row).await?;
        let target = row.target()?;
        let storage =
            identify_storage_target(&mut request.transaction, context, &target, &self.platform)
                .await?;
        if storage.backend != row.storage_backend
            || storage.configuration_id != row.storage_config_id
        {
            return Err(conflict("storage_target_changed"));
        }
        let expected_entry = row.destination_entry_id.unwrap_or(row.proposed_entry_id);
        // Remove only this reservation from pending admission inside the same
        // transaction that publishes it. Rollback restores both stage and quota.
        sqlx::query("UPDATE briefcase.delegated_uploads SET status='committed',published_entry_id=$2 WHERE org_id=briefcase.current_org_id() AND upload_id=$1")
            .bind(upload_id).bind(expected_entry).execute(&mut *request.transaction).await.map_err(database_error)?;
        let published = publish_file_content(
            &mut request.transaction,
            context,
            &request.context,
            &parent,
            EntryId::from_uuid(row.proposed_entry_id).map_err(|_| internal_integrity())?,
            &row.name,
            &row.content_type,
            to_u64(row.size_bytes)?,
            &hex::encode(row.sha256()?),
            stored.checksum.as_ref().ok_or_else(internal_integrity)?,
            &target,
            &row.key()?,
            stored,
            StorageReference {
                backend: &row.storage_backend,
                configuration_id: row.storage_config_id,
            },
        )
        .await?;
        if published.as_uuid() != expected_entry {
            return Err(internal_integrity());
        }
        "committed".clone_into(&mut row.status);
        row.published_entry_id = Some(expected_entry);
        let status = row.status()?;
        request
            .transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, "commit_delegated_upload"))?;
        Ok(status)
    }

    async fn cancel(
        &self,
        context: &ExecutionContext,
        operation_id: Uuid,
    ) -> Result<DelegatedUploadStatus, AppError> {
        let identity = Identity::new(context)?;
        let mut request = content_begin(&self.repository, context).await?;
        let mut row = find_operation(&mut request.transaction, &identity, operation_id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.expire(&mut request.transaction, &mut row).await?;
        if matches!(row.status.as_str(), "reserved" | "receiving" | "staged") {
            let now = database_now(&mut request.transaction).await?;
            let next = if row.provider_write_started {
                "cleanup_pending"
            } else {
                "cancelled"
            };
            let cleanup_after = if row.provider_write_started {
                Some(Self::cleanup_deadline(&row, now)?)
            } else {
                None
            };
            sqlx::query("UPDATE briefcase.delegated_uploads SET status=$2,capability_hash=NULL,lease_token=NULL,lease_expires_at=NULL,cleanup_after=$3 WHERE org_id=briefcase.current_org_id() AND upload_id=$1")
                .bind(row.upload_id).bind(next).bind(cleanup_after).execute(&mut *request.transaction).await.map_err(database_error)?;
            next.clone_into(&mut row.status);
        }
        let status = row.status()?;
        request
            .transaction
            .commit()
            .await
            .map_err(|error| unknown_commit_error(error, "cancel_delegated_upload"))?;
        Ok(status)
    }
}

impl PostgresContentRepository {
    async fn scope_begin<'a>(
        &'a self,
        scope: &DelegatedUploadScope,
    ) -> Result<Transaction<'a, Postgres>, AppError> {
        let tenant = scope.testing_environment.map_or_else(
            || {
                TenantContext::for_control_service(
                    scope.organization_id.as_str(),
                    &scope.request_id,
                )
            },
            |plane| {
                TenantContext::for_testing_environment_service(
                    scope.organization_id.as_str(),
                    plane,
                    &scope.request_id,
                )
            },
        );
        self.repository.begin(&tenant).await.map_err(database_error)
    }

    async fn require_writer(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        transfer: &DelegatedUploadTransfer,
    ) -> Result<UploadRow, AppError> {
        let row = find_upload(transaction, transfer.upload_id)
            .await?
            .ok_or(AppError::NotFound)?;
        validate_scope(&row, &transfer.scope)?;
        let now = database_now(transaction).await?;
        if row.status != "receiving"
            || row.lease_token != Some(transfer.lease_token)
            || row.lease_expires_at != Some(transfer.lease_expires_at)
            || row.lease_expires_at.is_none_or(|deadline| deadline <= now)
            || row.expires_at <= now
        {
            return Err(conflict("delegated_upload_lease_lost"));
        }
        Ok(row)
    }

    fn cleanup_deadline(row: &UploadRow, now: OffsetDateTime) -> Result<OffsetDateTime, AppError> {
        row.lease_expires_at
            .unwrap_or(now)
            .max(now)
            .max(row.provider_deadline_at.unwrap_or(now))
            .checked_add(time::Duration::minutes(2))
            .ok_or_else(internal_integrity)
    }

    fn provider_timeout_millis(&self) -> Result<i64, AppError> {
        i64::try_from(self.platform.operation_timeout.as_millis()).map_err(|_| internal_integrity())
    }

    async fn expire(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        row: &mut UploadRow,
    ) -> Result<(), AppError> {
        let now = database_now(transaction).await?;
        if row.expires_at > now
            || !matches!(row.status.as_str(), "reserved" | "receiving" | "staged")
        {
            return Ok(());
        }
        let next = if row.provider_write_started {
            "cleanup_pending"
        } else {
            "expired"
        };
        let after = if row.provider_write_started {
            Some(Self::cleanup_deadline(row, now)?)
        } else {
            None
        };
        sqlx::query("UPDATE briefcase.delegated_uploads SET status=$2,capability_hash=NULL,lease_token=NULL,lease_expires_at=NULL,cleanup_after=$3 WHERE org_id=briefcase.current_org_id() AND upload_id=$1")
            .bind(row.upload_id).bind(next).bind(after).execute(&mut **transaction).await.map_err(database_error)?;
        next.clone_into(&mut row.status);
        row.capability_hash = None;
        row.lease_token = None;
        row.lease_expires_at = None;
        Ok(())
    }
}

struct Identity<'a> {
    control_version: i64,
    organization: Uuid,
    principal: Uuid,
    membership: Uuid,
    actor_type: &'static str,
    actor_id: &'a str,
    application: &'a str,
}

impl<'a> Identity<'a> {
    fn new(context: &'a ExecutionContext) -> Result<Self, AppError> {
        let auth = context.authorization();
        let binding = auth.iam_binding().ok_or(AppError::Forbidden)?;
        let application = auth.originating_application().ok_or(AppError::Forbidden)?;
        Ok(Self {
            control_version: context
                .testing_environment()
                .map_or(0, TestingEnvironmentContext::control_version),
            organization: binding.organization_id,
            principal: binding.principal_id,
            membership: binding.membership_id,
            actor_type: super::actor_kind(auth.actor().kind()),
            actor_id: auth.actor().id().as_str(),
            application: application.as_str(),
        })
    }
}

async fn find_operation(
    transaction: &mut Transaction<'_, Postgres>,
    identity: &Identity<'_>,
    operation_id: Uuid,
) -> Result<Option<UploadRow>, AppError> {
    sqlx::query_as::<_, UploadRow>("SELECT * FROM briefcase.delegated_uploads WHERE org_id=briefcase.current_org_id() AND control_version=$1 AND iam_organization_id=$2 AND iam_principal_id=$3 AND iam_membership_id=$4 AND actor_type=$5 AND actor_id=$6 AND origin_app_id=$7 AND operation_id=$8 FOR UPDATE")
        .bind(identity.control_version).bind(identity.organization).bind(identity.principal).bind(identity.membership)
        .bind(identity.actor_type).bind(identity.actor_id).bind(identity.application).bind(operation_id)
        .fetch_optional(&mut **transaction).await.map_err(database_error)
}

async fn find_upload(
    transaction: &mut Transaction<'_, Postgres>,
    upload_id: Uuid,
) -> Result<Option<UploadRow>, AppError> {
    sqlx::query_as::<_, UploadRow>("SELECT * FROM briefcase.delegated_uploads WHERE org_id=briefcase.current_org_id() AND upload_id=$1 FOR UPDATE")
        .bind(upload_id).fetch_optional(&mut **transaction).await.map_err(database_error)
}

async fn database_now(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<OffsetDateTime, AppError> {
    sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)
}

fn validate_scope(row: &UploadRow, scope: &DelegatedUploadScope) -> Result<(), AppError> {
    if row.testing_environment_id != scope.testing_environment.map(TestingEnvironmentContext::id)
        || row.control_version
            != scope
                .testing_environment
                .map_or(0, TestingEnvironmentContext::control_version)
    {
        return Err(AppError::NotFound);
    }
    Ok(())
}

fn validate_transfer(row: &UploadRow, transfer: &DelegatedUploadTransfer) -> Result<(), AppError> {
    if row.operation_id != transfer.operation_id
        || row.target()? != transfer.target
        || row.key()? != transfer.key
        || row.content_type != transfer.content_type
        || to_u64(row.size_bytes)? != transfer.size
        || row.sha256()? != transfer.sha256
    {
        return Err(conflict("delegated_upload_transfer_changed"));
    }
    Ok(())
}

fn validate_object(row: &UploadRow, stored: &StoredObject) -> Result<(), AppError> {
    validate_stored(stored, &row.key()?, to_u64(row.size_bytes)?)?;
    let checksum = stored.checksum.as_ref().ok_or_else(internal_integrity)?;
    if checksum.algorithm() != ObjectChecksumAlgorithm::Sha256
        || (checksum.checksum_type() == ObjectChecksumType::FullObject
            && checksum.encoded_value() != STANDARD.encode(row.sha256()?))
        || (checksum.checksum_type() == ObjectChecksumType::Composite
            && row.provider_upload_id.is_none())
    {
        return Err(conflict("stored_object_checksum_mismatch"));
    }
    Ok(())
}

async fn authorize_destination(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ExecutionContext,
    row: &UploadRow,
) -> Result<crate::application::service::AuthorizableEntry, AppError> {
    let parent_id = EntryId::from_uuid(row.parent_id).map_err(|_| internal_integrity())?;
    let parent = require_upload_parent(transaction, context, parent_id, &row.name).await?;
    let current = find_named_child(transaction, parent_id, &row.name, true)
        .await?
        .map(|(id, _)| id.as_uuid());
    if current != row.destination_entry_id {
        return Err(conflict("delegated_upload_destination_changed"));
    }
    Ok(parent)
}

async fn authorize_committed(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ExecutionContext,
    row: &UploadRow,
) -> Result<(), AppError> {
    let id = EntryId::from_uuid(row.published_entry_id.ok_or_else(internal_integrity)?)
        .map_err(|_| internal_integrity())?;
    let entry = load_entry(transaction, context, id, false, true)
        .await
        .map_err(map_metadata)?
        .ok_or(AppError::NotFound)?;
    require_entry_capability(&entry, context, Capability::WriteContent)
}
