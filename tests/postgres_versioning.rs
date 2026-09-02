//! Versioning, bin, and upload-allowance checks against a live PostgreSQL.
//!
//! Uploading over an existing file name publishes that file's next version,
//! and the bin pages like every other listing. Both are SQL the repository
//! builds itself, so they are exercised here rather than trusted to type
//! checking. The file is skipped unless `BRIEFCASE_TEST_DATABASE_URL` names a
//! database whose role may create the `briefcase` schema.
//!
//! ```bash
//! docker compose up -d postgres
//! BRIEFCASE_TEST_DATABASE_URL=postgres://briefcase:briefcase-local-only@127.0.0.1:5433/briefcase \
//!   cargo test --test postgres_versioning
//! ```

use std::{num::NonZeroU32, path::PathBuf, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use secrecy::SecretString;
use sha2::{Digest, Sha256};
use silicon_briefcase::{
    application::{
        content::{ContentRepository, Prepared, SmallUploadCommand, StagedContent},
        context::ExecutionContext,
        idempotency::IdempotencyKey,
        ports::{ObjectChecksum, ObjectChecksumAlgorithm, ObjectChecksumType, StoredObject},
        service::{
            ListBinQuery, ListEntriesQuery, ListVersionsQuery, MetadataRepository,
            MutationMetadata, PageRequest,
        },
    },
    config::{DatabaseSettings, S3Encryption, S3Settings},
    domain::{
        actor::{
            ActorId, ActorKind, ActorRef, AuthenticationMode, OrganizationId, OrganizationRole,
            RequestAuthContext,
        },
        entry::{EntryName, EntryPath},
        ids::EntryId,
        permission::Capability,
        quota::DAILY_UPLOAD_LIMIT_BYTES,
    },
    infrastructure::postgres::{self, PostgresContentRepository, PostgresRepository},
};
use sqlx::PgPool;
use uuid::Uuid;

const ACTOR_ID: &str = "cos:tos";
const FILE_NAME: &str = "report.md";

fn database_settings(url: String) -> DatabaseSettings {
    DatabaseSettings {
        url: SecretString::from(url),
        max_connections: NonZeroU32::new(4).unwrap_or(NonZeroU32::MIN),
        min_connections: 0,
        acquire_timeout: Duration::from_secs(10),
        statement_timeout: Duration::from_secs(30),
    }
}

fn storage_settings() -> S3Settings {
    // No object is stored here: publishing metadata is the behavior under
    // test, and the storage target only has to be well-formed.
    S3Settings {
        region: "us-east-1".to_owned(),
        bucket: "briefcase-tests".to_owned(),
        key_prefix: "orgs".to_owned(),
        endpoint_url: None,
        force_path_style: true,
        encryption: S3Encryption::SseS3,
        temporary_directory: PathBuf::from("/tmp"),
        operation_timeout: Duration::from_secs(10),
    }
}

fn execution_context(organization: &str) -> anyhow::Result<ExecutionContext> {
    let organization_id = OrganizationId::new(organization.to_owned())?;
    let actor = ActorRef::new(ActorKind::Carbon, ActorId::new(ACTOR_ID)?);
    Ok(ExecutionContext::new(
        RequestAuthContext::new(
            organization_id,
            actor,
            OrganizationRole::Owner,
            Vec::new(),
            AuthenticationMode::Bearer,
        ),
        "postgres-versioning-test",
    ))
}

/// Returns the caller's own private folder, which the first request reconciles.
async fn private_root(
    metadata: &PostgresRepository,
    context: &ExecutionContext,
) -> anyhow::Result<EntryId> {
    metadata
        .list_active_children(
            context,
            &ListEntriesQuery {
                parent_id: None,
                filter: None,
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    let path = EntryPath::new(format!("private/{ACTOR_ID}"))?;
    let root = metadata
        .find_active_entry_by_path(context, &path)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the caller's private folder must be reconciled"))?;
    Ok(root.entry.id)
}

/// Reads the organization's charged bytes with the tenant setting applied.
async fn charged_bytes(pool: &PgPool, organization: &str) -> anyhow::Result<(i64, i64)> {
    let mut connection = tenant_connection(pool, organization).await?;
    let row = sqlx::query_as::<_, (i64, i64)>(
        "SELECT daily_bytes, total_bytes FROM briefcase.organization_upload_usage \
          WHERE org_id = briefcase.current_org_id()",
    )
    .fetch_one(&mut *connection)
    .await?;
    Ok(row)
}

async fn tenant_connection(
    pool: &PgPool,
    organization: &str,
) -> anyhow::Result<sqlx::pool::PoolConnection<sqlx::Postgres>> {
    let mut connection = pool.acquire().await?;
    sqlx::query("SELECT set_config('briefcase.org_id', $1, false)")
        .bind(organization)
        .execute(&mut *connection)
        .await?;
    Ok(connection)
}

async fn publish(
    repository: &PostgresContentRepository,
    context: &ExecutionContext,
    parent_id: EntryId,
    key: &str,
    bytes: &[u8],
) -> anyhow::Result<EntryId> {
    let staged = std::env::temp_dir().join(format!("briefcase-version-{}", Uuid::now_v7()));
    tokio::fs::write(&staged, bytes).await?;
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let payload = StagedContent {
        path: staged.as_path(),
        offset: 0,
        size: bytes.len() as u64,
        sha256: digest,
    };
    let command = SmallUploadCommand {
        parent_id,
        name: EntryName::new(FILE_NAME)?,
        content_type: "text/markdown".to_owned(),
        idempotency_key: IdempotencyKey::new(key.to_owned())?,
        request_hash: [3; 32],
    };
    let published = match repository
        .prepare_small_upload(context, &command, &payload)
        .await?
    {
        Prepared::Acquired(preparation) => {
            let stored = StoredObject {
                key: preparation.key.clone(),
                etag: Some("\"published\"".to_owned()),
                provider_version_id: None,
                size: payload.size,
                checksum: Some(ObjectChecksum::new(
                    ObjectChecksumAlgorithm::Sha256,
                    ObjectChecksumType::FullObject,
                    STANDARD.encode(digest),
                )?),
            };
            repository
                .commit_small_upload(context, &command, &payload, &preparation, &stored)
                .await?
        }
        Prepared::Replay(entry_id) => entry_id,
    };
    tokio::fs::remove_file(&staged).await?;
    Ok(published)
}

#[tokio::test]
async fn re_uploading_a_name_versions_the_same_file_and_the_bin_pages() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
        eprintln!("skipping: BRIEFCASE_TEST_DATABASE_URL is not set");
        return Ok(());
    };
    let pool = postgres::connect(&database_settings(url), "briefcase-tests").await?;
    postgres::migrate(&pool).await?;
    let metadata = PostgresRepository::new(pool.clone());
    let files = PostgresContentRepository::new(metadata.clone(), storage_settings());

    // A fresh organization per run keeps the test additive.
    let organization = format!("test-{}", Uuid::now_v7().simple());
    let context = execution_context(&organization)?;

    let parent = private_root(&metadata, &context).await?;
    let created = publish(&files, &context, parent, "upload-one", b"first").await?;
    let updated = publish(&files, &context, parent, "upload-two", b"second").await?;
    assert_eq!(
        created, updated,
        "uploading the same name updates the file rather than creating another"
    );

    let versions = metadata
        .list_file_versions(
            &context,
            &ListVersionsQuery {
                entry_id: created,
                page: PageRequest::new(None, 50)?,
            },
        )
        .await?;
    assert_eq!(
        versions.items.len(),
        2,
        "the update is retained as the file's second version"
    );
    let numbers: Vec<u64> = versions
        .items
        .iter()
        .map(|version| version.number.get())
        .collect();
    assert_eq!(
        numbers,
        vec![2, 1],
        "history reads newest first and numbers are consecutive"
    );

    // Deleting both the file and its folder fills the bin enough to page it.
    let mutation = MutationMetadata::new(
        Some(IdempotencyKey::new("delete-file".to_owned())?),
        [5; 32],
    );
    metadata
        .soft_delete_entry(&context, created, &mutation, Capability::Delete)
        .await?;

    let first_page = metadata
        .list_bin_entries(
            &context,
            &ListBinQuery {
                page: PageRequest::new(None, 1)?,
            },
        )
        .await?;
    assert_eq!(
        first_page.items.len(),
        1,
        "the bin honors the requested page size"
    );
    assert_eq!(
        first_page.items[0].entry.id, created,
        "the most recent deletion is listed first"
    );
    if let Some(cursor) = first_page.next_cursor {
        let second_page = metadata
            .list_bin_entries(
                &context,
                &ListBinQuery {
                    page: PageRequest::new(Some(cursor), 1)?,
                },
            )
            .await?;
        assert!(
            second_page
                .items
                .iter()
                .all(|entry| entry.entry.id != created),
            "a cursor advances past what the previous page returned"
        );
    }

    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn uploads_are_charged_to_the_organization_and_refused_once_spent() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
        eprintln!("skipping: BRIEFCASE_TEST_DATABASE_URL is not set");
        return Ok(());
    };
    let pool = postgres::connect(&database_settings(url), "briefcase-tests").await?;
    postgres::migrate(&pool).await?;
    let metadata = PostgresRepository::new(pool.clone());
    let files = PostgresContentRepository::new(metadata.clone(), storage_settings());

    let organization = format!("test-{}", Uuid::now_v7().simple());
    let context = execution_context(&organization)?;
    let parent = private_root(&metadata, &context).await?;

    let first = b"a modest upload";
    let first_bytes = i64::try_from(first.len())?;
    publish(&files, &context, parent, "charge-one", first).await?;
    let (daily, total) = charged_bytes(&pool, &organization).await?;
    assert_eq!(
        (daily, total),
        (first_bytes, first_bytes),
        "an upload charges both the daily window and the organization total"
    );

    let second = b"another modest upload";
    publish(&files, &context, parent, "charge-two", second).await?;
    let (daily, total) = charged_bytes(&pool, &organization).await?;
    let expected = first_bytes + i64::try_from(second.len())?;
    assert_eq!(
        (daily, total),
        (expected, expected),
        "publishing a new version of the same file is charged like any upload"
    );

    // Spending the daily allowance must refuse the next upload before the
    // bytes are asked for, and must leave the counters exactly as they were.
    let mut connection = tenant_connection(&pool, &organization).await?;
    sqlx::query(
        "UPDATE briefcase.organization_upload_usage SET daily_bytes = $1 \
          WHERE org_id = briefcase.current_org_id()",
    )
    .bind(i64::try_from(DAILY_UPLOAD_LIMIT_BYTES)?)
    .execute(&mut *connection)
    .await?;
    let refused = publish(
        &files,
        &context,
        parent,
        "charge-three",
        b"one byte too many",
    )
    .await;
    match refused {
        Err(error) => assert!(
            error.to_string().contains("daily_upload_limit_exhausted"),
            "the daily allowance is what refused the upload: {error}"
        ),
        Ok(_) => panic!("an organization with a spent daily allowance must not publish"),
    }
    let (daily, total) = charged_bytes(&pool, &organization).await?;
    assert_eq!(
        (daily, total),
        (i64::try_from(DAILY_UPLOAD_LIMIT_BYTES)?, expected),
        "a refused upload charges nothing"
    );

    // Yesterday's window is spent allowance that has already returned.
    sqlx::query(
        "UPDATE briefcase.organization_upload_usage \
            SET daily_window = daily_window - 1 WHERE org_id = briefcase.current_org_id()",
    )
    .execute(&mut *connection)
    .await?;
    // The pool cannot close while a borrowed connection is still held.
    drop(connection);
    let third = b"today is a new day";
    let third_bytes = i64::try_from(third.len())?;
    publish(&files, &context, parent, "charge-four", third).await?;
    let (daily, total) = charged_bytes(&pool, &organization).await?;
    assert_eq!(
        daily, third_bytes,
        "midnight UTC restarts the daily counter rather than resuming it"
    );
    assert_eq!(
        total,
        expected + third_bytes,
        "the organization total never resets"
    );

    pool.close().await;
    Ok(())
}
