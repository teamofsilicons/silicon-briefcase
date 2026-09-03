//! Object-store checks that run against a live S3-compatible service.
//!
//! Content delivery streams provider bytes straight to the caller, including
//! one exact range for media seeking, so the relay is verified against a real
//! provider rather than a stub. Skipped unless `BRIEFCASE_TEST_S3_BUCKET` is
//! set; the AWS credential chain and endpoint come from the same
//! `BRIEFCASE_S3_*` variables the services use.
//!
//! ```bash
//! docker compose up -d minio minio-init
//! set -a && source .env.example && set +a
//! BRIEFCASE_TEST_S3_BUCKET=briefcase-local cargo test --test s3_object_store
//! ```

use std::time::Duration;

use futures::TryStreamExt as _;
use sha2::{Digest as _, Sha256};
use silicon_briefcase::{
    application::ports::{ByteRange, ObjectKey, ObjectStore, OpenObjectRequest, StorageTarget},
    config::{S3Encryption, S3Settings},
    domain::storage::EncryptionMode,
    infrastructure::s3::S3ObjectStore,
};
use tokio::io::AsyncWriteExt as _;
use uuid::Uuid;

const CONTENT: &[u8] = b"apple cat banana apple";

fn settings(bucket: String) -> anyhow::Result<S3Settings> {
    let endpoint_url = match std::env::var("BRIEFCASE_S3_ENDPOINT_URL") {
        Ok(value) => Some(value.parse()?),
        Err(_) => None,
    };
    Ok(S3Settings {
        region: std::env::var("BRIEFCASE_S3_REGION").unwrap_or_else(|_| "us-east-1".to_owned()),
        bucket,
        key_prefix: "tests".to_owned(),
        endpoint_url,
        force_path_style: true,
        encryption: S3Encryption::SseS3,
        temporary_directory: std::env::temp_dir(),
        operation_timeout: Duration::from_secs(30),
    })
}

#[tokio::test]
async fn opening_an_object_streams_the_whole_body_and_exact_ranges() -> anyhow::Result<()> {
    let Ok(bucket) = std::env::var("BRIEFCASE_TEST_S3_BUCKET") else {
        eprintln!("skipping: BRIEFCASE_TEST_S3_BUCKET is not set");
        return Ok(());
    };
    let settings = settings(bucket.clone())?;
    let store = S3ObjectStore::from_settings(&settings).await;
    let target = StorageTarget {
        bucket,
        region: settings.region.clone(),
        prefix: settings.key_prefix.clone(),
        role_arn: None,
        external_id: None,
        encryption: EncryptionMode::SseS3,
        kms_key_arn: None,
    };
    let key = ObjectKey::new(format!("objects/{}", Uuid::now_v7()))?;

    let staged = std::env::temp_dir().join(format!("briefcase-test-{}", Uuid::now_v7()));
    let mut file = tokio::fs::File::create(&staged).await?;
    file.write_all(CONTENT).await?;
    file.flush().await?;
    drop(file);
    let digest: [u8; 32] = Sha256::digest(CONTENT).into();
    let size = u64::try_from(CONTENT.len())?;
    store
        .put_file(&target, &key, &staged, "text/plain", size, &digest)
        .await?;
    tokio::fs::remove_file(&staged).await?;

    // The complete object streams back byte for byte.
    let whole = store
        .open_object(OpenObjectRequest {
            target: &target,
            key: &key,
            provider_version_id: None,
            range: None,
        })
        .await?;
    assert_eq!(whole.total_size, size);
    assert!(whole.range.is_none());
    assert_eq!(collect(whole.body).await?, CONTENT);

    // A range returns exactly those bytes and still reports the complete size,
    // which is what a seeking media player needs.
    let range = ByteRange { start: 6, end: 8 };
    let partial = store
        .open_object(OpenObjectRequest {
            target: &target,
            key: &key,
            provider_version_id: None,
            range: Some(range),
        })
        .await?;
    assert_eq!(partial.total_size, size, "the complete size is reported");
    assert_eq!(partial.range, Some(range));
    assert_eq!(collect(partial.body).await?, b"cat");

    store.delete(&target, &key, None).await?;
    Ok(())
}

async fn collect(
    body: futures::stream::BoxStream<'static, std::io::Result<bytes::Bytes>>,
) -> anyhow::Result<Vec<u8>> {
    let chunks: Vec<bytes::Bytes> = body.try_collect().await?;
    Ok(chunks.concat())
}
