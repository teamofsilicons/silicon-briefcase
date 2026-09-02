//! Versioning, bin, and usage-limit checks against a live PostgreSQL.
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
        quota::{DEFAULT_DAILY_UPLOAD_LIMIT_BYTES, DEFAULT_STORAGE_LIMIT_BYTES},
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

/// Reads the day's uploaded bytes and the stored bytes, as the tenant.
async fn usage(pool: &PgPool, organization: &str) -> anyhow::Result<(i64, i64)> {
    let mut connection = tenant_connection(pool, organization).await?;
    let row = sqlx::query_as::<_, (i64, i64)>(
        "SELECT daily_upload_bytes, stored_bytes FROM briefcase.organization_usage \
          WHERE org_id = briefcase.current_org_id()",
    )
    .fetch_one(&mut *connection)
    .await?;
    Ok(row)
}

fn assert_refused(result: anyhow::Result<EntryId>, code: &str) {
    match result {
        Err(error) => assert!(
            error.to_string().contains(code),
            "expected {code}, got: {error}"
        ),
        Ok(_) => panic!("an organization over {code} must not publish"),
    }
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
async fn purging_the_bin_returns_the_space_that_binning_alone_still_holds() -> anyhow::Result<()> {
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

    let file = b"space that comes back later";
    let stored_bytes = i64::try_from(file.len())?;
    let entry = publish(&files, &context, parent, "bin-upload-one", file).await?;
    assert_eq!(usage(&pool, &organization).await?.1, stored_bytes);

    // Binning holds the space: the object is still in storage, recoverable for
    // the whole retention window.
    let mutation = MutationMetadata::new(
        Some(IdempotencyKey::new("bin-delete-one".to_owned())?),
        [9; 32],
    );
    metadata
        .soft_delete_entry(&context, entry, &mutation, Capability::Delete)
        .await?;
    assert_eq!(
        usage(&pool, &organization).await?.1,
        stored_bytes,
        "a binned file is still stored, so it still consumes the organization's space"
    );

    // Purging is what returns it. This is the statement the worker runs once
    // the retention window has passed and every object is confirmed deleted.
    let mut connection = tenant_connection(&pool, &organization).await?;
    let batch: Uuid = sqlx::query_scalar(
        "SELECT deletion_batch_id FROM briefcase.entries \
          WHERE org_id = briefcase.current_org_id() AND entry_id = $1",
    )
    .bind(entry.as_uuid())
    .fetch_one(&mut *connection)
    .await?;
    let purged = sqlx::query(
        "DELETE FROM briefcase.entries \
          WHERE org_id = briefcase.current_org_id() AND deletion_batch_id = $1",
    )
    .bind(batch)
    .execute(&mut *connection)
    .await?
    .rows_affected();
    assert_eq!(purged, 1, "the binned file is the whole deletion batch");
    assert_eq!(
        usage(&pool, &organization).await?.1,
        0,
        "permanent deletion returns the space to the organization"
    );

    drop(connection);
    let reported = metadata.load_organization_usage(&context).await?;
    assert_eq!(
        reported.storage_remaining(),
        reported.storage_allowance(),
        "the whole ceiling is available again"
    );

    pool.close().await;
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn usage_tracks_uploads_and_storage_and_refuses_what_exceeds_a_limit() -> anyhow::Result<()> {
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
    let entry = publish(&files, &context, parent, "charge-one", first).await?;
    assert_eq!(
        usage(&pool, &organization).await?,
        (first_bytes, first_bytes),
        "an upload charges the day and the bytes it stores"
    );

    let second = b"another modest upload";
    let both_bytes = first_bytes + i64::try_from(second.len())?;
    publish(&files, &context, parent, "charge-two", second).await?;
    assert_eq!(
        usage(&pool, &organization).await?,
        (both_bytes, both_bytes),
        "a new version is charged like any upload, and both versions stay stored"
    );

    // Spending the day must refuse the next upload before its bytes are
    // stored, and must leave every counter as it was.
    let mut connection = tenant_connection(&pool, &organization).await?;
    sqlx::query(
        "UPDATE briefcase.organization_usage SET daily_upload_bytes = $1 \
          WHERE org_id = briefcase.current_org_id()",
    )
    .bind(i64::try_from(DEFAULT_DAILY_UPLOAD_LIMIT_BYTES)?)
    .execute(&mut *connection)
    .await?;
    assert_refused(
        publish(
            &files,
            &context,
            parent,
            "charge-three",
            b"one byte too many",
        )
        .await,
        "daily_upload_limit_exhausted",
    );
    assert_eq!(
        usage(&pool, &organization).await?,
        (i64::try_from(DEFAULT_DAILY_UPLOAD_LIMIT_BYTES)?, both_bytes),
        "a refused upload charges nothing"
    );

    // Yesterday's window is allowance that has already returned.
    sqlx::query(
        "UPDATE briefcase.organization_usage \
            SET daily_window = daily_window - 1 WHERE org_id = briefcase.current_org_id()",
    )
    .execute(&mut *connection)
    .await?;
    let third = b"today is a new day";
    let third_bytes = i64::try_from(third.len())?;
    publish(&files, &context, parent, "charge-four", third).await?;
    assert_eq!(
        usage(&pool, &organization).await?,
        (third_bytes, both_bytes + third_bytes),
        "midnight UTC restarts the day's counter while stored bytes keep climbing"
    );

    // A ceiling configured for this organization alone is the one enforced.
    sqlx::query(
        "UPDATE briefcase.organization_usage SET storage_limit_bytes = stored_bytes \
          WHERE org_id = briefcase.current_org_id()",
    )
    .execute(&mut *connection)
    .await?;
    assert_refused(
        publish(&files, &context, parent, "charge-five", b"no room left").await,
        "storage_limit_exhausted",
    );
    sqlx::query(
        "UPDATE briefcase.organization_usage SET storage_limit_bytes = NULL \
          WHERE org_id = briefcase.current_org_id()",
    )
    .execute(&mut *connection)
    .await?;

    // Deleting a retained version returns its bytes, whichever process does it.
    let removed = sqlx::query(
        "DELETE FROM briefcase.entry_versions \
          WHERE org_id = briefcase.current_org_id() AND entry_id = $1 AND version_number = 1",
    )
    .bind(entry.as_uuid())
    .execute(&mut *connection)
    .await?
    .rows_affected();
    assert_eq!(
        removed, 1,
        "the first version must still have been retained"
    );
    assert_eq!(
        usage(&pool, &organization).await?.1,
        both_bytes + third_bytes - first_bytes,
        "storage falls by exactly what the deleted version weighed"
    );

    // What the usage endpoint reports is what the limits enforce.
    drop(connection);
    let reported = metadata.load_organization_usage(&context).await?;
    assert_eq!(
        (
            i64::try_from(reported.daily_upload_bytes)?,
            i64::try_from(reported.stored_bytes)?
        ),
        usage(&pool, &organization).await?
    );
    assert_eq!(
        reported.storage_allowance(),
        DEFAULT_STORAGE_LIMIT_BYTES,
        "clearing an override restores the platform default"
    );

    pool.close().await;
    Ok(())
}
