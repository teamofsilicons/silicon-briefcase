//! An object store in memory, for router tests that read file bytes.
//!
//! It serves `open_object` (whole objects and ranges) from bytes a test put
//! in; everything else reports the store unavailable.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream;

use crate::application::ports::{
    ByteRange, CopyObjectRequest, DownloadRangeRequest, ObjectKey, ObjectMetadata, ObjectStore,
    ObjectStoreError, OpenObject, OpenObjectRequest, StorageTarget, StorageValidation,
    StoredObject, StoredPart, UploadPartRequest,
};

/// Objects by key.
#[derive(Clone, Default)]
pub(crate) struct MemoryStore {
    objects: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl MemoryStore {
    /// Stores `bytes` under `key`.
    pub(crate) fn insert(&self, key: &ObjectKey, bytes: Vec<u8>) {
        if let Ok(mut objects) = self.objects.lock() {
            objects.insert(key.as_str().to_owned(), bytes);
        }
    }
}

#[async_trait]
impl ObjectStore for MemoryStore {
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

    async fn open_object(
        &self,
        request: OpenObjectRequest<'_>,
    ) -> Result<OpenObject, ObjectStoreError> {
        let bytes = self
            .objects
            .lock()
            .map_err(|_| ObjectStoreError::Unavailable)?
            .get(request.key.as_str())
            .cloned()
            .ok_or(ObjectStoreError::NotFound)?;
        let total_size = bytes.len() as u64;
        let (start, end) = request
            .range
            .map_or((0, bytes.len()), |ByteRange { start, end }| {
                (
                    usize::try_from(start).unwrap_or(usize::MAX),
                    usize::try_from(end + 1).unwrap_or(usize::MAX),
                )
            });
        let slice = bytes
            .get(start..end.min(bytes.len()))
            .ok_or(ObjectStoreError::NotFound)?
            .to_vec();
        Ok(OpenObject {
            total_size,
            range: request.range,
            etag: None,
            body: Box::pin(stream::iter([Ok(Bytes::from(slice))])),
        })
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
        _request: DownloadRangeRequest<'_>,
    ) -> Result<(), ObjectStoreError> {
        Err(ObjectStoreError::Unavailable)
    }

    async fn head(
        &self,
        _target: &StorageTarget,
        _key: &ObjectKey,
        _provider_version_id: Option<&str>,
    ) -> Result<ObjectMetadata, ObjectStoreError> {
        Err(ObjectStoreError::Unavailable)
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
        Err(ObjectStoreError::Unavailable)
    }

    async fn create_multipart(
        &self,
        _target: &StorageTarget,
        _key: &ObjectKey,
        _content_type: &str,
    ) -> Result<String, ObjectStoreError> {
        Err(ObjectStoreError::Unavailable)
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
        Err(ObjectStoreError::Unavailable)
    }

    async fn validate_configuration(
        &self,
        _target: &StorageTarget,
        _expected_account_id: &str,
    ) -> Result<StorageValidation, ObjectStoreError> {
        Err(ObjectStoreError::Unavailable)
    }
}
