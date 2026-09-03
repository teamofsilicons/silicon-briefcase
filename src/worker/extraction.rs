//! Search-index text extraction.
//!
//! Publishing a file records its name in the search index and leaves the
//! extraction pending. This pass turns that pending row into searchable text
//! by reading the bytes the version actually points at, so a search for a word
//! inside a document finds the document rather than only its filename.
//!
//! Only media types that already are text are read. Everything else is
//! recorded as `unsupported`, which is a settled answer rather than a queue
//! that never drains: a format-specific extractor can revisit those rows when
//! one exists. Extraction never blocks an upload and never fails one.

use futures::StreamExt as _;
use sqlx::PgPool;
use tracing::warn;

use crate::{
    application::ports::{
        ObjectKey, ObjectStore, ObjectStoreError, OpenObjectRequest, StorageTarget,
    },
    domain::{media::is_extractable_text, storage::EncryptionMode},
    infrastructure::s3::organization_storage_external_id,
};

/// Most text kept for one document.
///
/// A search index is not a copy of the file: the opening megabyte is what
/// makes a document findable, and the bytes themselves remain the source of
/// truth for anyone who opens it.
const MAX_EXTRACTED_BYTES: usize = 1 << 20;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ExtractionStats {
    pub(super) indexed: u64,
    pub(super) unsupported: u64,
    pub(super) deferred: u64,
}

#[derive(Debug, sqlx::FromRow)]
struct PendingDocument {
    org_id: String,
    entry_id: uuid::Uuid,
    content_type: Option<String>,
    storage_backend: String,
    storage_config_id: Option<uuid::Uuid>,
    bucket_name: String,
    storage_region: String,
    storage_prefix: String,
    storage_encryption_mode: String,
    storage_kms_key_arn: Option<String>,
    storage_role_arn: Option<String>,
    object_key: String,
    object_version_id: Option<String>,
}

/// Extracts text for one bounded batch of pending documents.
///
/// # Errors
///
/// Returns the database error when the batch cannot be read or recorded. A
/// single document that cannot be read from storage is left pending for the
/// next pass instead of failing the batch.
pub(super) async fn run<O>(
    pool: &PgPool,
    objects: &O,
    batch_size: i64,
) -> Result<ExtractionStats, sqlx::Error>
where
    O: ObjectStore + ?Sized,
{
    let pending = sqlx::query_as::<_, PendingDocument>(
        "SELECT document.org_id, document.entry_id, entry.content_type, \
                version.storage_backend, version.storage_config_id, version.bucket_name, \
                version.storage_region, version.storage_prefix, \
                version.storage_encryption_mode, version.storage_kms_key_arn, \
                storage_config.role_arn AS storage_role_arn, \
                version.object_key, version.object_version_id \
           FROM briefcase.search_documents AS document \
           JOIN briefcase.entries AS entry \
             ON entry.org_id = document.org_id AND entry.entry_id = document.entry_id \
           JOIN briefcase.entry_versions AS version \
             ON version.org_id = entry.org_id AND version.entry_id = entry.entry_id \
            AND version.version_id = entry.current_version_id \
      LEFT JOIN briefcase.organization_storage_configs AS storage_config \
             ON storage_config.org_id = version.org_id \
            AND storage_config.storage_config_id = version.storage_config_id \
          WHERE document.extraction_status = 'pending' \
            AND entry.entry_type = 'file' \
            AND entry.deleted_at IS NULL \
          ORDER BY document.updated_at, document.entry_id \
          LIMIT $1",
    )
    .bind(batch_size)
    .fetch_all(pool)
    .await?;

    let mut stats = ExtractionStats::default();
    for document in pending {
        if !is_extractable_text(document.content_type.as_deref()) {
            settle_unsupported(pool, &document).await?;
            stats.unsupported += 1;
            continue;
        }
        let Some((target, key)) = storage_location(&document) else {
            warn!(
                event = "extraction_descriptor_invalid",
                error_code = "extraction_descriptor_invalid",
                "search extraction skipped an unreadable storage descriptor"
            );
            settle_unsupported(pool, &document).await?;
            stats.unsupported += 1;
            continue;
        };
        match read_text(
            objects,
            &target,
            &key,
            document.object_version_id.as_deref(),
        )
        .await
        {
            Ok(Some(text)) => {
                settle_indexed(pool, &document, &text).await?;
                stats.indexed += 1;
            }
            Ok(None) => {
                settle_unsupported(pool, &document).await?;
                stats.unsupported += 1;
            }
            Err(ReadFailure::Missing) => {
                // The bytes this version points at are gone. Retrying forever
                // would keep the row at the head of every batch, so the answer
                // is recorded and the file stays served exactly as before.
                settle_failed(pool, &document, "object_not_found").await?;
                stats.unsupported += 1;
            }
            Err(ReadFailure::Deferred) => {
                // Storage was unreachable or refused the read. The row stays
                // pending so the next pass tries again; nothing about the file
                // itself has been decided.
                stats.deferred += 1;
            }
        }
    }
    Ok(stats)
}

/// Why an object's text could not be read.
enum ReadFailure {
    /// The object is not there, which no amount of retrying will change.
    Missing,
    /// Storage could not answer; the document stays pending.
    Deferred,
}

/// Reads the opening text of an object, or `None` when the bytes are not text.
async fn read_text<O>(
    objects: &O,
    target: &StorageTarget,
    key: &ObjectKey,
    provider_version_id: Option<&str>,
) -> Result<Option<String>, ReadFailure>
where
    O: ObjectStore + ?Sized,
{
    let opened = objects
        .open_object(OpenObjectRequest {
            target,
            key,
            provider_version_id,
            range: None,
        })
        .await
        .map_err(|error| {
            warn!(
                event = "extraction_object_unreadable",
                error_code = object_error_code(&error),
                "search extraction could not open an object"
            );
            match error {
                ObjectStoreError::NotFound => ReadFailure::Missing,
                _ => ReadFailure::Deferred,
            }
        })?;

    let mut buffer = Vec::with_capacity(MAX_EXTRACTED_BYTES.min(64 * 1024));
    let mut body = opened.body;
    while buffer.len() < MAX_EXTRACTED_BYTES {
        let Some(chunk) = body.next().await else {
            break;
        };
        let chunk = chunk.map_err(|_| ReadFailure::Deferred)?;
        let room = MAX_EXTRACTED_BYTES - buffer.len();
        buffer.extend_from_slice(&chunk[..chunk.len().min(room)]);
    }
    drop(body);

    match std::str::from_utf8(&buffer) {
        Ok(text) => Ok(Some(text.to_owned())),
        // A multi-byte character split by the size cap is expected; anything
        // else means the bytes are not the text the media type claimed.
        Err(error) if error.error_len().is_none() => Ok(Some(
            String::from_utf8_lossy(&buffer[..error.valid_up_to()]).into_owned(),
        )),
        Err(_) => Ok(None),
    }
}

const fn object_error_code(error: &ObjectStoreError) -> &'static str {
    match error {
        ObjectStoreError::NotFound => "object_not_found",
        ObjectStoreError::Conflict => "object_conflict",
        ObjectStoreError::InvalidConfiguration => "storage_configuration_invalid",
        ObjectStoreError::Unavailable => "object_storage_unavailable",
        ObjectStoreError::Internal(_) => "object_storage_internal",
    }
}

fn storage_location(document: &PendingDocument) -> Option<(StorageTarget, ObjectKey)> {
    let encryption = match document.storage_encryption_mode.as_str() {
        "sse_s3" if document.storage_kms_key_arn.is_none() => EncryptionMode::SseS3,
        "sse_kms" if document.storage_kms_key_arn.is_some() => EncryptionMode::SseKms,
        _ => return None,
    };
    // Briefcase derives the organization External ID itself and never reads it
    // from a row, exactly as destructive cleanup does.
    let (role_arn, external_id) = match document.storage_backend.as_str() {
        "platform" if document.storage_config_id.is_none() => (None, None),
        "organization" if document.storage_config_id.is_some() => (
            Some(document.storage_role_arn.clone()?),
            Some(organization_storage_external_id(&document.org_id)),
        ),
        _ => return None,
    };
    let key = ObjectKey::new(document.object_key.clone()).ok()?;
    Some((
        StorageTarget {
            bucket: document.bucket_name.clone(),
            region: document.storage_region.clone(),
            prefix: document.storage_prefix.clone(),
            role_arn,
            external_id,
            encryption,
            kms_key_arn: document.storage_kms_key_arn.clone(),
        },
        key,
    ))
}

async fn settle_indexed(
    pool: &PgPool,
    document: &PendingDocument,
    text: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE briefcase.search_documents \
            SET extracted_content = $3, extraction_status = 'indexed', \
                extraction_error_code = NULL, indexed_at = clock_timestamp() \
          WHERE org_id = $1 AND entry_id = $2 AND extraction_status = 'pending'",
    )
    .bind(&document.org_id)
    .bind(document.entry_id)
    .bind(text)
    .execute(pool)
    .await
    .map(drop)
}

async fn settle_failed(
    pool: &PgPool,
    document: &PendingDocument,
    error_code: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE briefcase.search_documents \
            SET extracted_content = NULL, extraction_status = 'failed', \
                extraction_error_code = $3, indexed_at = NULL \
          WHERE org_id = $1 AND entry_id = $2 AND extraction_status = 'pending'",
    )
    .bind(&document.org_id)
    .bind(document.entry_id)
    .bind(error_code)
    .execute(pool)
    .await
    .map(drop)
}

async fn settle_unsupported(pool: &PgPool, document: &PendingDocument) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE briefcase.search_documents \
            SET extracted_content = NULL, extraction_status = 'unsupported', \
                extraction_error_code = NULL, indexed_at = NULL \
          WHERE org_id = $1 AND entry_id = $2 AND extraction_status = 'pending'",
    )
    .bind(&document.org_id)
    .bind(document.entry_id)
    .execute(pool)
    .await
    .map(drop)
}

#[cfg(test)]
mod tests {
    use super::{PendingDocument, storage_location};

    fn document(backend: &str, config: Option<uuid::Uuid>, role: Option<&str>) -> PendingDocument {
        PendingDocument {
            org_id: "tos".to_owned(),
            entry_id: uuid::Uuid::now_v7(),
            content_type: Some("text/plain".to_owned()),
            storage_backend: backend.to_owned(),
            storage_config_id: config,
            bucket_name: "briefcase".to_owned(),
            storage_region: "ap-south-1".to_owned(),
            storage_prefix: "organizations".to_owned(),
            storage_encryption_mode: "sse_s3".to_owned(),
            storage_kms_key_arn: None,
            storage_role_arn: role.map(ToOwned::to_owned),
            object_key: "entries/a/versions/b".to_owned(),
            object_version_id: None,
        }
    }

    #[test]
    fn platform_storage_assumes_no_role() {
        let (target, _) = storage_location(&document("platform", None, None))
            .unwrap_or_else(|| panic!("platform descriptor must resolve"));
        assert!(target.role_arn.is_none());
        assert!(target.external_id.is_none());
    }

    #[test]
    fn organization_storage_derives_its_external_id() {
        let source = document(
            "organization",
            Some(uuid::Uuid::now_v7()),
            Some("arn:aws:iam::1:role/briefcase"),
        );
        let (target, _) =
            storage_location(&source).unwrap_or_else(|| panic!("organization must resolve"));
        assert_eq!(
            target.role_arn.as_deref(),
            Some("arn:aws:iam::1:role/briefcase")
        );
        assert!(target.external_id.is_some());
    }

    #[test]
    fn an_incoherent_descriptor_is_refused() {
        assert!(storage_location(&document("organization", None, None)).is_none());
        assert!(
            storage_location(&document("platform", Some(uuid::Uuid::now_v7()), None)).is_none()
        );
    }
}
