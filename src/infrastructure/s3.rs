//! AWS S3 object-storage adapter.

use std::{collections::BTreeMap, path::Path, sync::Arc};

use async_trait::async_trait;
use aws_config::{SdkConfig, sts::AssumeRoleProvider};
use aws_credential_types::provider::SharedCredentialsProvider;
use aws_sdk_s3::{
    Client as S3Client,
    config::Region,
    types::{
        ChecksumAlgorithm, ChecksumMode, ChecksumType, CompletedMultipartUpload, CompletedPart,
        ServerSideEncryption,
    },
};
use aws_sdk_sts::Client as StsClient;
use aws_smithy_types::{
    byte_stream::{ByteStream, Length},
    timeout::TimeoutConfig,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::StreamExt as _;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest as _, Sha256};
use tokio::{
    fs::File,
    io::{AsyncReadExt as _, AsyncWriteExt as _, BufWriter},
    sync::RwLock,
};
use tokio_util::io::ReaderStream;
use tracing::warn;
use url::Url;
use uuid::Uuid;

use crate::{
    application::ports::{
        CopyObjectRequest, DownloadRangeRequest, ObjectChecksum, ObjectChecksumAlgorithm,
        ObjectChecksumType, ObjectKey, ObjectMetadata, ObjectStore, ObjectStoreError, OpenObject,
        OpenObjectRequest, StorageTarget, StorageValidation, StoredObject, StoredPart,
        UploadPartRequest,
    },
    config::{S3Encryption, S3Settings},
    domain::actor::OrganizationId,
    domain::multipart::MultipartPlan,
    domain::storage::EncryptionMode,
};

const MAXIMUM_CACHED_CLIENTS: usize = 128;
const COPY_OBJECT_MAX_BYTES: u64 = 5 * 1_073_741_824;
const VALIDATION_CONTENT: &[u8] = b"silicon-briefcase-storage-validation-v1";
const UPDATED_VALIDATION_CONTENT: &[u8] = b"silicon-briefcase-storage-validation-v2";

fn sha256_checksum(
    value: &str,
    checksum_type: ObjectChecksumType,
) -> Result<ObjectChecksum, ObjectStoreError> {
    ObjectChecksum::new(ObjectChecksumAlgorithm::Sha256, checksum_type, value)
        .map_err(|error| ObjectStoreError::Internal(error.into()))
}

fn provider_sha256_checksum(
    value: Option<&str>,
    checksum_type: Option<&ChecksumType>,
) -> Result<Option<ObjectChecksum>, ObjectStoreError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let checksum_type = match checksum_type {
        Some(ChecksumType::FullObject) => ObjectChecksumType::FullObject,
        Some(ChecksumType::Composite) => ObjectChecksumType::Composite,
        Some(_) | None => {
            return Err(ObjectStoreError::Internal(anyhow::anyhow!(
                "S3 omitted or returned an unsupported checksum type"
            )));
        }
    };
    sha256_checksum(value, checksum_type).map(Some)
}

fn decode_sha256(value: &str) -> Result<[u8; 32], ObjectStoreError> {
    STANDARD
        .decode(value)
        .map_err(|error| ObjectStoreError::Internal(error.into()))?
        .try_into()
        .map_err(|_| ObjectStoreError::Internal(anyhow::anyhow!("invalid S3 SHA-256 checksum")))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SameTargetCopyStrategy {
    SingleRequest,
    Multipart,
}

const fn same_target_copy_strategy(size: u64) -> SameTargetCopyStrategy {
    if size <= COPY_OBJECT_MAX_BYTES {
        SameTargetCopyStrategy::SingleRequest
    } else {
        SameTargetCopyStrategy::Multipart
    }
}

/// AWS SDK backed implementation of the Briefcase object-store port.
pub struct S3ObjectStore {
    base_config: SdkConfig,
    endpoint_url: Option<String>,
    force_path_style: bool,
    clients: RwLock<BTreeMap<ClientCacheKey, Arc<ResolvedClients>>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ClientCacheKey {
    region: String,
    role_arn: Option<String>,
    external_id: Option<String>,
}

struct ResolvedClients {
    s3: S3Client,
    sts: StsClient,
}

/// Owns enough client state to compensate a cancelled server-side multipart
/// copy without borrowing the request future that created it.
struct S3MultipartCopyCleanup {
    client: S3Client,
    bucket: String,
    key: String,
    upload_id: String,
    provider_version_id: Option<String>,
    armed: bool,
}

struct MultipartCopyPartsRequest<'a> {
    target: &'a StorageTarget,
    source: &'a ObjectKey,
    source_provider_version_id: Option<&'a str>,
    destination: &'a ObjectKey,
    upload_id: &'a str,
    source_metadata: &'a ObjectMetadata,
    plan: MultipartPlan,
}

impl S3MultipartCopyCleanup {
    fn new(client: S3Client, bucket: String, key: String, upload_id: String) -> Self {
        Self {
            client,
            bucket,
            key,
            upload_id,
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
        cleanup_s3_multipart_copy(
            self.client.clone(),
            self.bucket.clone(),
            self.key.clone(),
            self.upload_id.clone(),
            self.provider_version_id.clone(),
        )
        .await;
        self.armed = false;
    }
}

impl Drop for S3MultipartCopyCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            warn!("cancelled S3 multipart-copy cleanup could not start outside a Tokio runtime");
            return;
        };
        drop(runtime.spawn(cleanup_s3_multipart_copy(
            self.client.clone(),
            self.bucket.clone(),
            self.key.clone(),
            self.upload_id.clone(),
            self.provider_version_id.clone(),
        )));
    }
}

async fn cleanup_s3_multipart_copy(
    client: S3Client,
    bucket: String,
    key: String,
    upload_id: String,
    known_version_id: Option<String>,
) {
    if client
        .abort_multipart_upload()
        .bucket(&bucket)
        .key(&key)
        .upload_id(upload_id)
        .send()
        .await
        .is_err()
    {
        warn!("cancelled S3 multipart-copy session requires lifecycle reconciliation");
    }
    let provider_version_id = if let Some(version_id) = known_version_id {
        Some(version_id)
    } else {
        let Ok(metadata) = client.head_object().bucket(&bucket).key(&key).send().await else {
            warn!("cancelled S3 multipart-copy object requires reconciliation");
            return;
        };
        metadata.version_id().map(str::to_owned)
    };
    if client
        .delete_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(provider_version_id)
        .send()
        .await
        .is_err()
    {
        warn!("cancelled S3 multipart-copy object requires reconciliation");
    }
}

impl S3ObjectStore {
    /// Builds the adapter from validated service settings and the process AWS
    /// credential chain.
    pub async fn from_settings(settings: &S3Settings) -> Self {
        let timeout = TimeoutConfig::builder()
            .operation_timeout(settings.operation_timeout)
            .build();
        let base_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .timeout_config(timeout)
            .load()
            .await;
        Self::new(
            base_config,
            settings.endpoint_url.clone(),
            settings.force_path_style,
        )
    }

    /// Builds an adapter from the process AWS credential chain.
    ///
    /// `endpoint_url` and path-style access exist for local S3-compatible test
    /// services. Production validation should leave both at their AWS defaults.
    pub async fn from_environment(endpoint_url: Option<Url>, force_path_style: bool) -> Self {
        let base_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self::new(base_config, endpoint_url, force_path_style)
    }

    /// Builds an adapter from an already loaded AWS shared configuration.
    #[must_use]
    pub fn new(base_config: SdkConfig, endpoint_url: Option<Url>, force_path_style: bool) -> Self {
        Self {
            base_config,
            endpoint_url: endpoint_url.map(Into::into),
            force_path_style,
            clients: RwLock::new(BTreeMap::new()),
        }
    }

    async fn clients(
        &self,
        target: &StorageTarget,
    ) -> Result<Arc<ResolvedClients>, ObjectStoreError> {
        let cache_key = ClientCacheKey {
            region: target.region.clone(),
            role_arn: target.role_arn.clone(),
            external_id: target.external_id.clone(),
        };
        if let Some(clients) = self.clients.read().await.get(&cache_key).cloned() {
            return Ok(clients);
        }

        let clients = Arc::new(self.build_clients(target).await?);
        let mut cache = self.clients.write().await;
        if cache.len() >= MAXIMUM_CACHED_CLIENTS
            && let Some(oldest_key) = cache.keys().next().cloned()
        {
            cache.remove(&oldest_key);
        }
        Ok(cache.entry(cache_key).or_insert(clients).clone())
    }

    async fn build_clients(
        &self,
        target: &StorageTarget,
    ) -> Result<ResolvedClients, ObjectStoreError> {
        let mut config_builder = self
            .base_config
            .to_builder()
            .region(Region::new(target.region.clone()));

        if let Some(role_arn) = target.role_arn.as_deref() {
            let mut provider_builder = AssumeRoleProvider::builder(role_arn)
                .configure(&self.base_config)
                .region(Region::new(target.region.clone()))
                .session_name("silicon-briefcase");
            if let Some(external_id) = target.external_id.as_deref() {
                provider_builder = provider_builder.external_id(external_id);
            }
            let provider = provider_builder.build().await;
            config_builder =
                config_builder.credentials_provider(SharedCredentialsProvider::new(provider));
        }

        let shared_config = config_builder.build();
        let mut s3_builder = aws_sdk_s3::config::Builder::from(&shared_config)
            .force_path_style(self.force_path_style);
        if let Some(endpoint_url) = self.endpoint_url.as_deref() {
            s3_builder = s3_builder.endpoint_url(endpoint_url);
        }
        Ok(ResolvedClients {
            s3: S3Client::from_conf(s3_builder.build()),
            sts: StsClient::new(&shared_config),
        })
    }

    fn full_key(target: &StorageTarget, key: &ObjectKey) -> String {
        let prefix = target.prefix.trim_matches('/');
        if prefix.is_empty() {
            key.as_str().to_owned()
        } else {
            format!("{prefix}/{}", key.as_str())
        }
    }

    fn copy_source(
        target: &StorageTarget,
        key: &ObjectKey,
        provider_version_id: Option<&str>,
    ) -> String {
        let source_key = Self::full_key(target, key);
        let source = format!("{}/{source_key}", target.bucket);
        let encoded_source = utf8_percent_encode(&source, NON_ALPHANUMERIC);
        match provider_version_id {
            Some(version_id) => format!(
                "{encoded_source}?versionId={}",
                utf8_percent_encode(version_id, NON_ALPHANUMERIC)
            ),
            None => encoded_source.to_string(),
        }
    }

    async fn cleanup_object(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        known_version_id: Option<&str>,
    ) {
        let provider_version_id = match known_version_id {
            Some(version_id) => Some(version_id.to_owned()),
            None => match self.head(target, key, None).await {
                Ok(metadata) => metadata.provider_version_id,
                Err(_) => return,
            },
        };
        let _ = self
            .delete(target, key, provider_version_id.as_deref())
            .await;
    }

    fn apply_encryption_to_put(
        request: aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder,
        target: &StorageTarget,
    ) -> aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder {
        match target.encryption {
            EncryptionMode::SseS3 => request.server_side_encryption(ServerSideEncryption::Aes256),
            EncryptionMode::SseKms => request
                .server_side_encryption(ServerSideEncryption::AwsKms)
                .set_ssekms_key_id(target.kms_key_arn.clone()),
        }
    }

    fn apply_encryption_to_create_multipart(
        request: aws_sdk_s3::operation::create_multipart_upload::builders::CreateMultipartUploadFluentBuilder,
        target: &StorageTarget,
    ) -> aws_sdk_s3::operation::create_multipart_upload::builders::CreateMultipartUploadFluentBuilder
    {
        match target.encryption {
            EncryptionMode::SseS3 => request.server_side_encryption(ServerSideEncryption::Aes256),
            EncryptionMode::SseKms => request
                .server_side_encryption(ServerSideEncryption::AwsKms)
                .set_ssekms_key_id(target.kms_key_arn.clone()),
        }
    }

    fn apply_encryption_to_copy(
        request: aws_sdk_s3::operation::copy_object::builders::CopyObjectFluentBuilder,
        target: &StorageTarget,
    ) -> aws_sdk_s3::operation::copy_object::builders::CopyObjectFluentBuilder {
        match target.encryption {
            EncryptionMode::SseS3 => request.server_side_encryption(ServerSideEncryption::Aes256),
            EncryptionMode::SseKms => request
                .server_side_encryption(ServerSideEncryption::AwsKms)
                .set_ssekms_key_id(target.kms_key_arn.clone()),
        }
    }

    async fn put_probe(
        clients: &ResolvedClients,
        target: &StorageTarget,
        key: &str,
        body: &'static [u8],
    ) -> Result<(), ObjectStoreError> {
        let request = clients
            .s3
            .put_object()
            .bucket(&target.bucket)
            .key(key)
            .content_type("application/octet-stream")
            .body(ByteStream::from_static(body));
        Self::apply_encryption_to_put(request, target)
            .send()
            .await
            .map_err(|_| ObjectStoreError::InvalidConfiguration)?;
        Ok(())
    }

    async fn copy_single_object(
        &self,
        target: &StorageTarget,
        source: &ObjectKey,
        source_provider_version_id: Option<&str>,
        destination: &ObjectKey,
        content_type: &str,
        source_metadata: ObjectMetadata,
    ) -> Result<StoredObject, ObjectStoreError> {
        let clients = self.clients(target).await?;
        let request = clients
            .s3
            .copy_object()
            .bucket(&target.bucket)
            .key(Self::full_key(target, destination))
            .copy_source(Self::copy_source(
                target,
                source,
                source_provider_version_id,
            ))
            .set_copy_source_if_match(source_metadata.etag.clone())
            .content_type(content_type)
            .checksum_algorithm(ChecksumAlgorithm::Sha256)
            .metadata_directive(aws_sdk_s3::types::MetadataDirective::Replace);
        let Ok(output) = Self::apply_encryption_to_copy(request, target).send().await else {
            self.cleanup_object(target, destination, None).await;
            return Err(ObjectStoreError::Unavailable);
        };
        let output_version_id = output.version_id().map(str::to_owned);
        let metadata = self.head(target, destination, None).await;
        if metadata.is_err() {
            self.cleanup_object(target, destination, output_version_id.as_deref())
                .await;
        }
        let metadata = metadata?;
        let destination_version_id = output_version_id
            .clone()
            .or_else(|| metadata.provider_version_id.clone());
        if metadata.size != source_metadata.size {
            self.cleanup_object(target, destination, destination_version_id.as_deref())
                .await;
            return Err(ObjectStoreError::Conflict);
        }
        let response_checksum = provider_sha256_checksum(
            output
                .copy_object_result()
                .and_then(|result| result.checksum_sha256()),
            output
                .copy_object_result()
                .and_then(|result| result.checksum_type()),
        );
        if response_checksum.is_err() {
            self.cleanup_object(target, destination, destination_version_id.as_deref())
                .await;
        }
        let response_checksum = response_checksum?;
        let Some(destination_checksum) = response_checksum.or_else(|| metadata.checksum.clone())
        else {
            self.cleanup_object(target, destination, destination_version_id.as_deref())
                .await;
            return Err(ObjectStoreError::Internal(anyhow::anyhow!(
                "S3 omitted the copied object's SHA-256 checksum"
            )));
        };
        if source_metadata
            .checksum
            .as_ref()
            .is_some_and(|source_checksum| {
                source_checksum.checksum_type() == destination_checksum.checksum_type()
                    && source_checksum != &destination_checksum
            })
        {
            self.cleanup_object(target, destination, destination_version_id.as_deref())
                .await;
            return Err(ObjectStoreError::Conflict);
        }
        Ok(StoredObject {
            key: destination.clone(),
            etag: output
                .copy_object_result()
                .and_then(|result| result.e_tag())
                .map(str::to_owned)
                .or(metadata.etag),
            provider_version_id: destination_version_id,
            size: metadata.size,
            checksum: Some(destination_checksum),
        })
    }

    async fn copy_multipart_object(
        &self,
        target: &StorageTarget,
        source: &ObjectKey,
        source_provider_version_id: Option<&str>,
        destination: &ObjectKey,
        content_type: &str,
        source_metadata: &ObjectMetadata,
    ) -> Result<StoredObject, ObjectStoreError> {
        let plan = MultipartPlan::for_file_size(source_metadata.size)
            .map_err(|error| ObjectStoreError::Internal(error.into()))?;
        let clients = self.clients(target).await?;
        let upload_id = self
            .create_multipart(target, destination, content_type)
            .await?;
        let mut cleanup = S3MultipartCopyCleanup::new(
            clients.s3.clone(),
            target.bucket.clone(),
            Self::full_key(target, destination),
            upload_id.clone(),
        );
        let outcome = self
            .copy_multipart_parts(MultipartCopyPartsRequest {
                target,
                source,
                source_provider_version_id,
                destination,
                upload_id: &upload_id,
                source_metadata,
                plan,
            })
            .await;
        if outcome.is_err() {
            cleanup.cleanup_now().await;
        }
        let stored = outcome?;
        cleanup.record_stored_object(&stored);
        if source_metadata
            .checksum
            .as_ref()
            .is_some_and(|checksum| checksum.checksum_type() == ObjectChecksumType::Composite)
            && stored.checksum.as_ref() != source_metadata.checksum.as_ref()
        {
            cleanup.cleanup_now().await;
            return Err(ObjectStoreError::Conflict);
        }
        cleanup.disarm();
        Ok(stored)
    }

    async fn copy_multipart_parts(
        &self,
        request: MultipartCopyPartsRequest<'_>,
    ) -> Result<StoredObject, ObjectStoreError> {
        let MultipartCopyPartsRequest {
            target,
            source,
            source_provider_version_id,
            destination,
            upload_id,
            source_metadata,
            plan,
        } = request;
        let clients = self.clients(target).await?;
        let copy_source = Self::copy_source(target, source, source_provider_version_id);
        let mut parts = Vec::with_capacity(
            usize::try_from(plan.part_count())
                .map_err(|error| ObjectStoreError::Internal(error.into()))?,
        );
        let mut offset = 0_u64;
        for part_number in 1..=plan.part_count() {
            let size = plan
                .expected_part_size(part_number)
                .map_err(|error| ObjectStoreError::Internal(error.into()))?;
            let end = offset
                .checked_add(size)
                .and_then(|exclusive| exclusive.checked_sub(1))
                .ok_or_else(|| {
                    ObjectStoreError::Internal(anyhow::anyhow!("copy range overflow"))
                })?;
            let part_number_i32 = i32::try_from(part_number)
                .map_err(|error| ObjectStoreError::Internal(error.into()))?;
            let output = clients
                .s3
                .upload_part_copy()
                .bucket(&target.bucket)
                .key(Self::full_key(target, destination))
                .upload_id(upload_id)
                .part_number(part_number_i32)
                .copy_source(&copy_source)
                .copy_source_range(format!("bytes={offset}-{end}"))
                .set_copy_source_if_match(source_metadata.etag.clone())
                .send()
                .await
                .map_err(|_| ObjectStoreError::Unavailable)?;
            let result = output.copy_part_result().ok_or_else(|| {
                ObjectStoreError::Internal(anyhow::anyhow!("S3 omitted copy part result"))
            })?;
            let etag = result.e_tag().map(str::to_owned).ok_or_else(|| {
                ObjectStoreError::Internal(anyhow::anyhow!("S3 omitted part ETag"))
            })?;
            let checksum = result.checksum_sha256().ok_or_else(|| {
                ObjectStoreError::Internal(anyhow::anyhow!("S3 omitted copied part checksum"))
            })?;
            parts.push(StoredPart {
                part_number,
                etag,
                checksum_sha256: decode_sha256(checksum)?,
            });
            offset = end.checked_add(1).ok_or_else(|| {
                ObjectStoreError::Internal(anyhow::anyhow!("copy range overflow"))
            })?;
        }
        self.complete_multipart(target, destination, upload_id, &parts, source_metadata.size)
            .await
    }
}

/// Resolves the immutable platform-owned storage target for an organization.
///
/// The tenant path segment is a hash because IAM organization identifiers are
/// opaque and must never be interpreted as S3 path syntax.
#[must_use]
pub fn platform_storage_target(
    settings: &S3Settings,
    organization_id: &OrganizationId,
) -> StorageTarget {
    platform_storage_target_for_scope(settings, organization_id.as_str())
}

/// Resolves platform storage for an internal, already authenticated scope.
///
/// Test scopes include the environment UUID, preventing a sandbox from ever
/// addressing production objects even when the public IAM organization is the
/// same.
#[must_use]
pub fn platform_storage_target_for_scope(settings: &S3Settings, scope: &str) -> StorageTarget {
    let tenant_segment = hex::encode(Sha256::digest(scope.as_bytes()));
    let encryption = match &settings.encryption {
        S3Encryption::SseS3 => EncryptionMode::SseS3,
        S3Encryption::SseKms { .. } => EncryptionMode::SseKms,
    };
    let kms_key_arn = match &settings.encryption {
        S3Encryption::SseS3 => None,
        S3Encryption::SseKms { key_arn } => Some(key_arn.clone()),
    };
    StorageTarget {
        bucket: settings.bucket.clone(),
        region: settings.region.clone(),
        prefix: format!("{}/{tenant_segment}", settings.key_prefix),
        role_arn: None,
        external_id: None,
        encryption,
        kms_key_arn,
    }
}

/// Derives the stable STS External ID for an organization-owned storage role.
///
/// The identifier is derived only from server-trusted organization context. It
/// is confused-deputy protection, not a credential, and is never accepted from
/// an API payload or persisted cleanup job.
#[must_use]
pub fn organization_storage_external_id(organization_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"silicon-briefcase:byo-storage:v1\0");
    digest.update(organization_id.as_bytes());
    hex::encode(digest.finalize())
}

#[cfg(test)]
fn platform_tenant_segment(organization_id: &OrganizationId) -> String {
    hex::encode(Sha256::digest(organization_id.as_str().as_bytes()))
}

#[async_trait]
impl ObjectStore for S3ObjectStore {
    async fn put_file(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        path: &Path,
        content_type: &str,
        size: u64,
        checksum_sha256: &[u8; 32],
    ) -> Result<StoredObject, ObjectStoreError> {
        let clients = self.clients(target).await?;
        let full_key = Self::full_key(target, key);
        let body = ByteStream::read_from()
            .path(path)
            .length(Length::Exact(size))
            .build()
            .await
            .map_err(|error| ObjectStoreError::Internal(error.into()))?;
        let content_length =
            i64::try_from(size).map_err(|error| ObjectStoreError::Internal(error.into()))?;
        let encoded_checksum = STANDARD.encode(checksum_sha256);
        let request = clients
            .s3
            .put_object()
            .bucket(&target.bucket)
            .key(full_key)
            .content_type(content_type)
            .content_length(content_length)
            .checksum_sha256(&encoded_checksum)
            .body(body);
        let output = Self::apply_encryption_to_put(request, target)
            .send()
            .await
            .map_err(|_| ObjectStoreError::Unavailable)?;
        if output
            .checksum_sha256()
            .is_some_and(|value| value != encoded_checksum)
            || output
                .checksum_type()
                .is_some_and(|value| value != &ChecksumType::FullObject)
        {
            return Err(ObjectStoreError::Conflict);
        }
        Ok(StoredObject {
            key: key.clone(),
            etag: output.e_tag().map(str::to_owned),
            provider_version_id: output.version_id().map(str::to_owned),
            size,
            checksum: Some(sha256_checksum(
                &encoded_checksum,
                ObjectChecksumType::FullObject,
            )?),
        })
    }

    async fn open_object(
        &self,
        request: OpenObjectRequest<'_>,
    ) -> Result<OpenObject, ObjectStoreError> {
        let target = request.target;
        let clients = self.clients(target).await?;
        let output = clients
            .s3
            .get_object()
            .bucket(&target.bucket)
            .key(Self::full_key(target, request.key))
            .set_version_id(request.provider_version_id.map(str::to_owned))
            .set_range(
                request
                    .range
                    .map(|range| format!("bytes={}-{}", range.start, range.end)),
            )
            .send()
            .await
            .map_err(|error| {
                if error
                    .as_service_error()
                    .is_some_and(aws_sdk_s3::operation::get_object::GetObjectError::is_no_such_key)
                {
                    ObjectStoreError::NotFound
                } else {
                    ObjectStoreError::Unavailable
                }
            })?;
        let served_bytes = u64::try_from(output.content_length().unwrap_or_default())
            .map_err(|error| ObjectStoreError::Internal(error.into()))?;
        // A ranged read reports the complete size only in Content-Range; the
        // provider must agree with the range Briefcase asked for.
        let (total_size, range) = match request.range {
            None => (served_bytes, None),
            Some(range) => {
                let total_size = output
                    .content_range()
                    .and_then(|value| value.rsplit_once('/'))
                    .and_then(|(_, total)| total.trim().parse::<u64>().ok())
                    .ok_or(ObjectStoreError::Conflict)?;
                if served_bytes != range.length() {
                    return Err(ObjectStoreError::Conflict);
                }
                (total_size, Some(range))
            }
        };
        let etag = output.e_tag().map(str::to_owned);
        let body = ReaderStream::new(output.body.into_async_read()).boxed();
        Ok(OpenObject {
            total_size,
            range,
            etag,
            body,
        })
    }

    async fn get_to_file(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_version_id: Option<&str>,
        path: &Path,
    ) -> Result<ObjectMetadata, ObjectStoreError> {
        let clients = self.clients(target).await?;
        let output = clients
            .s3
            .get_object()
            .bucket(&target.bucket)
            .key(Self::full_key(target, key))
            .set_version_id(provider_version_id.map(str::to_owned))
            .checksum_mode(ChecksumMode::Enabled)
            .send()
            .await
            .map_err(|error| {
                if error
                    .as_service_error()
                    .is_some_and(aws_sdk_s3::operation::get_object::GetObjectError::is_no_such_key)
                {
                    ObjectStoreError::NotFound
                } else {
                    ObjectStoreError::Unavailable
                }
            })?;
        let content_length = output.content_length().unwrap_or_default();
        let size = u64::try_from(content_length)
            .map_err(|error| ObjectStoreError::Internal(error.into()))?;
        let etag = output.e_tag().map(str::to_owned);
        let provider_version_id = output.version_id().map(str::to_owned);
        let checksum = provider_sha256_checksum(output.checksum_sha256(), output.checksum_type())?;
        let mut reader = output.body.into_async_read();
        let file = File::create(path)
            .await
            .map_err(|error| ObjectStoreError::Internal(error.into()))?;
        let mut writer = BufWriter::new(file);
        tokio::io::copy(&mut reader, &mut writer)
            .await
            .map_err(|error| ObjectStoreError::Internal(error.into()))?;
        writer
            .flush()
            .await
            .map_err(|error| ObjectStoreError::Internal(error.into()))?;
        Ok(ObjectMetadata {
            size,
            etag,
            provider_version_id,
            checksum,
        })
    }

    async fn get_range_to_file(
        &self,
        request: DownloadRangeRequest<'_>,
    ) -> Result<(), ObjectStoreError> {
        if request.size == 0 {
            return Err(ObjectStoreError::Internal(anyhow::anyhow!(
                "zero-length S3 range requested"
            )));
        }
        let end = request
            .offset
            .checked_add(request.size)
            .and_then(|exclusive| exclusive.checked_sub(1))
            .ok_or_else(|| ObjectStoreError::Internal(anyhow::anyhow!("S3 range overflow")))?;
        let clients = self.clients(request.target).await?;
        let output = clients
            .s3
            .get_object()
            .bucket(&request.target.bucket)
            .key(Self::full_key(request.target, request.key))
            .set_version_id(request.provider_version_id.map(str::to_owned))
            .range(format!("bytes={}-{end}", request.offset))
            .set_if_match(request.if_match.map(str::to_owned))
            .send()
            .await
            .map_err(|error| {
                let service_error = error.as_service_error();
                if service_error
                    .is_some_and(aws_sdk_s3::operation::get_object::GetObjectError::is_no_such_key)
                {
                    ObjectStoreError::NotFound
                } else if service_error
                    .and_then(|inner| inner.meta().code())
                    .is_some_and(|code| matches!(code, "PreconditionFailed" | "InvalidRange"))
                {
                    ObjectStoreError::Conflict
                } else {
                    ObjectStoreError::Unavailable
                }
            })?;
        let reported_size = u64::try_from(output.content_length().unwrap_or_default())
            .map_err(|error| ObjectStoreError::Internal(error.into()))?;
        let expected_content_range = format!("bytes {}-{end}/", request.offset);
        if reported_size != request.size
            || !output
                .content_range()
                .is_some_and(|value| value.starts_with(&expected_content_range))
        {
            return Err(ObjectStoreError::Conflict);
        }

        let mut reader = output.body.into_async_read();
        let file = File::create(request.path)
            .await
            .map_err(|error| ObjectStoreError::Internal(error.into()))?;
        let mut writer = BufWriter::new(file);
        let mut buffer = vec![0_u8; 64 * 1024];
        let mut written = 0_u64;
        loop {
            let read = reader
                .read(&mut buffer)
                .await
                .map_err(|error| ObjectStoreError::Internal(error.into()))?;
            if read == 0 {
                break;
            }
            writer
                .write_all(&buffer[..read])
                .await
                .map_err(|error| ObjectStoreError::Internal(error.into()))?;
            written = written
                .checked_add(
                    u64::try_from(read)
                        .map_err(|error| ObjectStoreError::Internal(error.into()))?,
                )
                .ok_or_else(|| {
                    ObjectStoreError::Internal(anyhow::anyhow!("range byte count overflow"))
                })?;
            if written > request.size {
                return Err(ObjectStoreError::Conflict);
            }
        }
        writer
            .flush()
            .await
            .map_err(|error| ObjectStoreError::Internal(error.into()))?;
        if written != request.size {
            return Err(ObjectStoreError::Conflict);
        }
        Ok(())
    }

    async fn head(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_version_id: Option<&str>,
    ) -> Result<ObjectMetadata, ObjectStoreError> {
        let clients = self.clients(target).await?;
        let output = clients
            .s3
            .head_object()
            .bucket(&target.bucket)
            .key(Self::full_key(target, key))
            .set_version_id(provider_version_id.map(str::to_owned))
            .checksum_mode(ChecksumMode::Enabled)
            .send()
            .await
            .map_err(|error| {
                if error
                    .as_service_error()
                    .is_some_and(aws_sdk_s3::operation::head_object::HeadObjectError::is_not_found)
                {
                    ObjectStoreError::NotFound
                } else {
                    ObjectStoreError::Unavailable
                }
            })?;
        let size = u64::try_from(output.content_length().unwrap_or_default())
            .map_err(|error| ObjectStoreError::Internal(error.into()))?;
        Ok(ObjectMetadata {
            size,
            etag: output.e_tag().map(str::to_owned),
            provider_version_id: output.version_id().map(str::to_owned),
            checksum: provider_sha256_checksum(output.checksum_sha256(), output.checksum_type())?,
        })
    }

    async fn copy(&self, request: CopyObjectRequest<'_>) -> Result<StoredObject, ObjectStoreError> {
        let source_metadata = self
            .head(
                request.target,
                request.source,
                request.source_provider_version_id,
            )
            .await?;
        if source_metadata.size != request.expected_size
            || source_metadata.checksum.as_ref() != Some(request.expected_checksum)
        {
            return Err(ObjectStoreError::Conflict);
        }
        if source_metadata.etag.is_none() {
            return Err(ObjectStoreError::Internal(anyhow::anyhow!(
                "S3 omitted the source object's ETag"
            )));
        }
        match same_target_copy_strategy(source_metadata.size) {
            SameTargetCopyStrategy::SingleRequest => {
                self.copy_single_object(
                    request.target,
                    request.source,
                    request.source_provider_version_id,
                    request.destination,
                    request.content_type,
                    source_metadata,
                )
                .await
            }
            SameTargetCopyStrategy::Multipart => {
                self.copy_multipart_object(
                    request.target,
                    request.source,
                    request.source_provider_version_id,
                    request.destination,
                    request.content_type,
                    &source_metadata,
                )
                .await
            }
        }
    }

    async fn delete(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_version_id: Option<&str>,
    ) -> Result<(), ObjectStoreError> {
        let clients = self.clients(target).await?;
        clients
            .s3
            .delete_object()
            .bucket(&target.bucket)
            .key(Self::full_key(target, key))
            .set_version_id(provider_version_id.map(str::to_owned))
            .send()
            .await
            .map_err(|_| ObjectStoreError::Unavailable)?;
        Ok(())
    }

    async fn create_multipart(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        content_type: &str,
    ) -> Result<String, ObjectStoreError> {
        let clients = self.clients(target).await?;
        let request = clients
            .s3
            .create_multipart_upload()
            .bucket(&target.bucket)
            .key(Self::full_key(target, key))
            .content_type(content_type)
            .checksum_algorithm(ChecksumAlgorithm::Sha256)
            .checksum_type(ChecksumType::Composite);
        let output = Self::apply_encryption_to_create_multipart(request, target)
            .send()
            .await
            .map_err(|_| ObjectStoreError::Unavailable)?;
        output
            .upload_id()
            .map(str::to_owned)
            .ok_or_else(|| ObjectStoreError::Internal(anyhow::anyhow!("S3 omitted upload_id")))
    }

    async fn upload_part(
        &self,
        request: UploadPartRequest<'_>,
    ) -> Result<String, ObjectStoreError> {
        let UploadPartRequest {
            target,
            key,
            provider_upload_id,
            part_number,
            path,
            offset,
            size,
            checksum_sha256,
        } = request;
        let clients = self.clients(target).await?;
        // A part is a range of one staged upload, so the body reads exactly
        // that range instead of copying it to its own temporary file.
        let body = ByteStream::read_from()
            .path(path)
            .offset(offset)
            .length(Length::Exact(size))
            .build()
            .await
            .map_err(|error| ObjectStoreError::Internal(error.into()))?;
        let content_length =
            i64::try_from(size).map_err(|error| ObjectStoreError::Internal(error.into()))?;
        let part_number =
            i32::try_from(part_number).map_err(|error| ObjectStoreError::Internal(error.into()))?;
        let encoded_checksum = STANDARD.encode(checksum_sha256);
        let output = clients
            .s3
            .upload_part()
            .bucket(&target.bucket)
            .key(Self::full_key(target, key))
            .upload_id(provider_upload_id)
            .part_number(part_number)
            .content_length(content_length)
            .checksum_sha256(&encoded_checksum)
            .body(body)
            .send()
            .await
            .map_err(|_| ObjectStoreError::Unavailable)?;
        if output
            .checksum_sha256()
            .is_some_and(|value| value != encoded_checksum)
        {
            return Err(ObjectStoreError::Conflict);
        }
        output
            .e_tag()
            .map(str::to_owned)
            .ok_or_else(|| ObjectStoreError::Internal(anyhow::anyhow!("S3 omitted part ETag")))
    }

    async fn complete_multipart(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_upload_id: &str,
        parts: &[StoredPart],
        expected_size: u64,
    ) -> Result<StoredObject, ObjectStoreError> {
        let clients = self.clients(target).await?;
        let completed_parts = parts
            .iter()
            .map(|part| {
                i32::try_from(part.part_number)
                    .map(|part_number| {
                        CompletedPart::builder()
                            .part_number(part_number)
                            .e_tag(&part.etag)
                            .checksum_sha256(STANDARD.encode(part.checksum_sha256))
                            .build()
                    })
                    .map_err(|error| ObjectStoreError::Internal(error.into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let completed_upload = CompletedMultipartUpload::builder()
            .set_parts(Some(completed_parts))
            .build();
        let output = clients
            .s3
            .complete_multipart_upload()
            .bucket(&target.bucket)
            .key(Self::full_key(target, key))
            .upload_id(provider_upload_id)
            .multipart_upload(completed_upload)
            .send()
            .await
            .map_err(|_| ObjectStoreError::Unavailable)?;
        let metadata = self.head(target, key, None).await?;
        if metadata.size != expected_size {
            return Err(ObjectStoreError::Conflict);
        }
        let checksum = provider_sha256_checksum(output.checksum_sha256(), output.checksum_type())?
            .or(metadata.checksum)
            .filter(|checksum| checksum.checksum_type() == ObjectChecksumType::Composite)
            .ok_or_else(|| {
                ObjectStoreError::Internal(anyhow::anyhow!(
                    "S3 omitted the completed multipart SHA-256 checksum"
                ))
            })?;
        let expected_suffix = format!("-{}", parts.len());
        if !checksum.encoded_value().ends_with(&expected_suffix) {
            return Err(ObjectStoreError::Conflict);
        }
        Ok(StoredObject {
            key: key.clone(),
            etag: output.e_tag().map(str::to_owned).or(metadata.etag),
            provider_version_id: output
                .version_id()
                .map(str::to_owned)
                .or(metadata.provider_version_id),
            size: metadata.size,
            checksum: Some(checksum),
        })
    }

    async fn abort_multipart(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_upload_id: &str,
    ) -> Result<(), ObjectStoreError> {
        let clients = self.clients(target).await?;
        let result = clients
            .s3
            .abort_multipart_upload()
            .bucket(&target.bucket)
            .key(Self::full_key(target, key))
            .upload_id(provider_upload_id)
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(error)
                if error
                    .as_service_error()
                    .and_then(|inner| inner.meta().code())
                    .is_some_and(|code| matches!(code, "NoSuchUpload" | "NoSuchKey")) =>
            {
                Ok(())
            }
            Err(_) => Err(ObjectStoreError::Unavailable),
        }
    }

    async fn validate_configuration(
        &self,
        target: &StorageTarget,
        expected_account_id: &str,
    ) -> Result<StorageValidation, ObjectStoreError> {
        if target.role_arn.is_none() || target.external_id.is_none() {
            return Err(ObjectStoreError::InvalidConfiguration);
        }
        let clients = self.clients(target).await?;
        let identity = clients
            .sts
            .get_caller_identity()
            .send()
            .await
            .map_err(|_| ObjectStoreError::InvalidConfiguration)?;
        let account_id = identity
            .account()
            .filter(|account| *account == expected_account_id)
            .map(str::to_owned)
            .ok_or(ObjectStoreError::InvalidConfiguration)?;

        let relative_key = format!("validation/{}", Uuid::now_v7());
        let key = if target.prefix.trim_matches('/').is_empty() {
            relative_key
        } else {
            format!("{}/{relative_key}", target.prefix.trim_matches('/'))
        };

        let probe = async {
            Self::put_probe(&clients, target, &key, VALIDATION_CONTENT).await?;
            let first = clients
                .s3
                .get_object()
                .bucket(&target.bucket)
                .key(&key)
                .send()
                .await
                .map_err(|_| ObjectStoreError::InvalidConfiguration)?
                .body
                .collect()
                .await
                .map_err(|_| ObjectStoreError::InvalidConfiguration)?;
            if first.into_bytes().as_ref() != VALIDATION_CONTENT {
                return Err(ObjectStoreError::InvalidConfiguration);
            }

            Self::put_probe(&clients, target, &key, UPDATED_VALIDATION_CONTENT).await?;
            let second = clients
                .s3
                .get_object()
                .bucket(&target.bucket)
                .key(&key)
                .send()
                .await
                .map_err(|_| ObjectStoreError::InvalidConfiguration)?
                .body
                .collect()
                .await
                .map_err(|_| ObjectStoreError::InvalidConfiguration)?;
            if second.into_bytes().as_ref() != UPDATED_VALIDATION_CONTENT {
                return Err(ObjectStoreError::InvalidConfiguration);
            }
            Ok(())
        }
        .await;

        let cleanup = clients
            .s3
            .delete_object()
            .bucket(&target.bucket)
            .key(&key)
            .send()
            .await;
        if cleanup.is_err() {
            return Err(ObjectStoreError::InvalidConfiguration);
        }
        probe?;
        Ok(StorageValidation { account_id })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        application::ports::{ObjectKey, StorageTarget},
        domain::{actor::OrganizationId, storage::EncryptionMode},
    };

    use super::{
        COPY_OBJECT_MAX_BYTES, S3ObjectStore, SameTargetCopyStrategy, platform_tenant_segment,
        same_target_copy_strategy,
    };

    #[test]
    fn platform_tenant_segment_never_interprets_opaque_ids_as_paths() -> anyhow::Result<()> {
        let first = OrganizationId::new("acme/../other")?;
        let second = OrganizationId::new("acme")?;

        let first_segment = platform_tenant_segment(&first);
        let second_segment = platform_tenant_segment(&second);

        assert_eq!(first_segment.len(), 64);
        assert!(first_segment.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first_segment, second_segment);
        assert!(!first_segment.contains("acme"));
        Ok(())
    }

    #[test]
    fn same_target_copy_uses_multipart_above_provider_limit() {
        assert_eq!(
            same_target_copy_strategy(COPY_OBJECT_MAX_BYTES),
            SameTargetCopyStrategy::SingleRequest
        );
        assert_eq!(
            same_target_copy_strategy(COPY_OBJECT_MAX_BYTES + 1),
            SameTargetCopyStrategy::Multipart
        );
    }

    #[test]
    fn copy_source_is_url_encoded_without_an_extra_root_segment() -> anyhow::Result<()> {
        let target = StorageTarget {
            bucket: "briefcase-data".to_owned(),
            region: "us-east-1".to_owned(),
            prefix: "tenant one".to_owned(),
            role_arn: None,
            external_id: None,
            encryption: EncryptionMode::SseS3,
            kms_key_arn: None,
        };
        let key = ObjectKey::new("entries/file/versions/one")?;

        assert_eq!(
            S3ObjectStore::copy_source(&target, &key, None),
            "briefcase%2Ddata%2Ftenant%20one%2Fentries%2Ffile%2Fversions%2Fone"
        );
        assert_eq!(
            S3ObjectStore::copy_source(&target, &key, Some("version/one+two=")),
            concat!(
                "briefcase%2Ddata%2Ftenant%20one%2Fentries%2Ffile%2Fversions%2Fone",
                "?versionId=version%2Fone%2Btwo%3D"
            )
        );
        Ok(())
    }
}
