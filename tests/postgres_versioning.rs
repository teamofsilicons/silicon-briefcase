//! Versioning and bin checks that run against a live PostgreSQL instance.
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
            ListBinQuery, ListVersionsQuery, MetadataRepository, MutationMetadata, PageRequest,
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
    },
    infrastructure::postgres::{self, PostgresContentRepository, PostgresRepository},
};
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

    // The first request reconciles the reserved containers, so the caller's
    // own private folder is a destination that always exists.
    let parent_path = EntryPath::new(format!("private/{ACTOR_ID}"))?;
    metadata
        .list_active_children(
            &context,
            &silicon_briefcase::application::service::ListEntriesQuery {
                parent_id: None,
                filter: None,
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    let parent = metadata
        .find_active_entry_by_path(&context, &parent_path)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the caller's private folder must be reconciled"))?;

    let created = publish(&files, &context, parent.entry.id, "upload-one", b"first").await?;
    let updated = publish(&files, &context, parent.entry.id, "upload-two", b"second").await?;
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
