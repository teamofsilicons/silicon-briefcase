//! Live PostgreSQL checks for the testing-environment control and data planes.
//!
//! The test is deliberately gated behind two explicit administrator URLs. It
//! migrates both disposable databases, then exercises the runtime store while
//! retaining enough privilege to remove every row it creates. The local
//! compose services can be used as follows:
//!
//! ```bash
//! docker compose up -d postgres postgres-test
//! BRIEFCASE_TEST_CONTROL_DATABASE_URL=postgres://briefcase:briefcase-local-only@127.0.0.1:5433/briefcase \
//! BRIEFCASE_TEST_DATA_DATABASE_URL=postgres://briefcase:briefcase-local-only@127.0.0.1:5434/briefcase_test \
//!   cargo test --test postgres_testing_environments
//! ```

use std::{
    panic::{AssertUnwindSafe, resume_unwind},
    path::Path,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use async_trait::async_trait;
use futures::FutureExt as _;
use secrecy::{ExposeSecret as _, SecretString};
use sha2::{Digest as _, Sha256};
use silicon_briefcase::{
    application::{
        context::{ExecutionContext, TestingEnvironmentContext},
        idempotency::{IdempotencyKey, bytes_fingerprint},
        ports::{
            CopyObjectRequest, DownloadRangeRequest, ObjectKey, ObjectMetadata, ObjectStore,
            ObjectStoreError, OpenObject, OpenObjectRequest, StorageTarget, StorageValidation,
            StoredObject, StoredPart, UploadPartRequest,
        },
        service::{
            CreateFolderCommand, CreateFolderMutation, GrantPermissionCommand, ListEntriesQuery,
            ListPermissionsQuery, MetadataRepository, MetadataRepositoryError, MutationMetadata,
            PageRequest,
        },
        testing::{
            TestingEnvironmentCreate, TestingEnvironmentIamPairing, TestingEnvironmentStatus,
            TestingEnvironmentWithKey,
        },
    },
    config::DatabaseSettings,
    domain::actor::{
        ActorId, ActorKind, ActorRef, AuthenticationMode, OrganizationId, OrganizationRole,
        RequestAuthContext,
    },
    domain::{
        entry::{EntryBoundary, EntryName},
        ids::EntryId,
        permission::{Capability, EntryVisibility, GrantedAccess},
    },
    error::AppError,
    infrastructure::{
        postgres::{self, PostgresRepository, TenantContext, begin_tenant_transaction},
        testing::{TestingEnvironmentStore, maintain_testing_environments},
    },
};
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::Barrier;
use uuid::Uuid;

const ROOT_LIMIT: i64 = 10;
const TEST_STORAGE_LIMIT_BYTES: i64 = 2 * 1024 * 1024 * 1024;
const IAM_APPLICATION_SECRET: &str = "ask_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const REPLACEMENT_IAM_APPLICATION_SECRET: &str = "ask_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[derive(Clone, Debug, Eq, PartialEq)]
enum StorageCall {
    Abort {
        prefix: String,
        key: String,
        upload_id: String,
    },
    Delete {
        prefix: String,
        key: String,
        version_id: Option<String>,
    },
}

#[derive(Default)]
struct RecordingObjectStore {
    calls: Mutex<Vec<StorageCall>>,
    delete_barrier: Option<Arc<Barrier>>,
    cleanup_returns_not_found: bool,
}

impl RecordingObjectStore {
    fn calls(&self) -> Vec<StorageCall> {
        match self.calls.lock() {
            Ok(calls) => calls.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn record(&self, call: StorageCall) {
        match self.calls.lock() {
            Ok(mut calls) => calls.push(call),
            Err(poisoned) => poisoned.into_inner().push(call),
        }
    }
}

fn unexpected_storage_call() -> ObjectStoreError {
    ObjectStoreError::Internal(anyhow::anyhow!(
        "the testing-environment cleaner made an unexpected object-store call"
    ))
}

#[async_trait]
impl ObjectStore for RecordingObjectStore {
    async fn put_file(
        &self,
        _target: &StorageTarget,
        _key: &ObjectKey,
        _path: &Path,
        _content_type: &str,
        _size: u64,
        _checksum_sha256: &[u8; 32],
    ) -> Result<StoredObject, ObjectStoreError> {
        Err(unexpected_storage_call())
    }

    async fn open_object(
        &self,
        _request: OpenObjectRequest<'_>,
    ) -> Result<OpenObject, ObjectStoreError> {
        Err(unexpected_storage_call())
    }

    async fn get_to_file(
        &self,
        _target: &StorageTarget,
        _key: &ObjectKey,
        _provider_version_id: Option<&str>,
        _path: &Path,
    ) -> Result<ObjectMetadata, ObjectStoreError> {
        Err(unexpected_storage_call())
    }

    async fn get_range_to_file(
        &self,
        _request: DownloadRangeRequest<'_>,
    ) -> Result<(), ObjectStoreError> {
        Err(unexpected_storage_call())
    }

    async fn head(
        &self,
        _target: &StorageTarget,
        _key: &ObjectKey,
        _provider_version_id: Option<&str>,
    ) -> Result<ObjectMetadata, ObjectStoreError> {
        Err(unexpected_storage_call())
    }

    async fn copy(
        &self,
        _request: CopyObjectRequest<'_>,
    ) -> Result<StoredObject, ObjectStoreError> {
        Err(unexpected_storage_call())
    }

    async fn delete(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_version_id: Option<&str>,
    ) -> Result<(), ObjectStoreError> {
        self.record(StorageCall::Delete {
            prefix: target.prefix.clone(),
            key: key.as_str().to_owned(),
            version_id: provider_version_id.map(str::to_owned),
        });
        if let Some(barrier) = &self.delete_barrier {
            barrier.wait().await;
            barrier.wait().await;
        }
        if self.cleanup_returns_not_found {
            Err(ObjectStoreError::NotFound)
        } else {
            Ok(())
        }
    }

    async fn create_multipart(
        &self,
        _target: &StorageTarget,
        _key: &ObjectKey,
        _content_type: &str,
    ) -> Result<String, ObjectStoreError> {
        Err(unexpected_storage_call())
    }

    async fn upload_part(
        &self,
        _request: UploadPartRequest<'_>,
    ) -> Result<String, ObjectStoreError> {
        Err(unexpected_storage_call())
    }

    async fn complete_multipart(
        &self,
        _target: &StorageTarget,
        _key: &ObjectKey,
        _provider_upload_id: &str,
        _parts: &[StoredPart],
        _expected_size: u64,
    ) -> Result<StoredObject, ObjectStoreError> {
        Err(unexpected_storage_call())
    }

    async fn abort_multipart(
        &self,
        target: &StorageTarget,
        key: &ObjectKey,
        provider_upload_id: &str,
    ) -> Result<(), ObjectStoreError> {
        self.record(StorageCall::Abort {
            prefix: target.prefix.clone(),
            key: key.as_str().to_owned(),
            upload_id: provider_upload_id.to_owned(),
        });
        if self.cleanup_returns_not_found {
            Err(ObjectStoreError::NotFound)
        } else {
            Ok(())
        }
    }

    async fn validate_configuration(
        &self,
        _target: &StorageTarget,
        _expected_account_id: &str,
    ) -> Result<StorageValidation, ObjectStoreError> {
        Err(unexpected_storage_call())
    }
}

#[derive(sqlx::FromRow)]
struct EncryptedEnvironmentRow {
    root_key_digest: Vec<u8>,
    root_key_ciphertext: Vec<u8>,
    iam_environment_key_digest: Vec<u8>,
    iam_environment_key_ciphertext: Vec<u8>,
    iam_app_secret_ciphertext: Vec<u8>,
}

#[derive(Debug, Eq, PartialEq, sqlx::FromRow)]
struct QueuedCleanupRow {
    cleanup_kind: String,
    storage_prefix: String,
    object_key: String,
    object_version_id: Option<String>,
    provider_upload_id: Option<String>,
}

fn testing_environment_fence_key(environment_id: Uuid) -> i64 {
    let mut digest = Sha256::new();
    digest.update(b"silicon-briefcase/testing-environment-clean-fence/v1");
    digest.update(environment_id.as_bytes());
    let digest = digest.finalize();
    let mut key = [0_u8; 8];
    key.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(key)
}

fn settings(url: String) -> DatabaseSettings {
    DatabaseSettings {
        url: SecretString::from(url),
        max_connections: std::num::NonZeroU32::new(8).unwrap_or(std::num::NonZeroU32::MIN),
        min_connections: 0,
        acquire_timeout: Duration::from_secs(10),
        statement_timeout: Duration::from_secs(30),
    }
}

fn execution(
    organization: &str,
    actor_id: &str,
    testing_environment: Option<TestingEnvironmentContext>,
) -> anyhow::Result<ExecutionContext> {
    let authorization = RequestAuthContext::new(
        OrganizationId::new(organization.to_owned())?,
        ActorRef::new(ActorKind::Carbon, ActorId::new(actor_id.to_owned())?),
        // A regular member may create and, as creator, manage an environment.
        OrganizationRole::Member,
        Vec::new(),
        AuthenticationMode::Bearer,
    );
    Ok(match testing_environment {
        Some(testing_environment) => ExecutionContext::in_testing_environment(
            authorization,
            "testing-environment-live-test",
            testing_environment,
        ),
        None => ExecutionContext::new(authorization, "testing-environment-live-test"),
    })
}

fn create_input(organization: &str, name: String) -> TestingEnvironmentCreate {
    TestingEnvironmentCreate {
        name,
        description: Some("isolated integration sandbox".to_owned()),
        iam_environment_id: Uuid::now_v7(),
        iam_environment_key: SecretString::from(Uuid::new_v4().simple().to_string()),
        iam_app_id: format!("{organization}>briefcase"),
        iam_app_secret: SecretString::from(IAM_APPLICATION_SECRET),
    }
}

fn mutation(key: impl Into<String>, fingerprint: &[u8]) -> anyhow::Result<MutationMetadata> {
    Ok(MutationMetadata::new(
        Some(IdempotencyKey::new(key.into())?),
        bytes_fingerprint("postgres-testing-environments", fingerprint),
    ))
}

fn assert_conflict(error: AppError, expected: &str) {
    match error {
        AppError::Conflict { code } => assert_eq!(code, expected),
        other => panic!("expected conflict {expected}, got {other:?}"),
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

async fn reconcile_roots(
    repository: &PostgresRepository,
    context: &ExecutionContext,
) -> anyhow::Result<()> {
    let page = repository
        .list_active_children(
            context,
            &ListEntriesQuery {
                parent_id: None,
                filter: None,
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    assert_eq!(
        page.items.len(),
        2,
        "public and private roots are reconciled"
    );
    for entry in &page.items {
        assert_eq!(
            &entry.entry.organization_id,
            context.authorization().organization_id(),
            "storage tenant namespaces must never escape into domain metadata"
        );
        assert_ne!(
            entry.authorization(context.authorization()).visibility(),
            EntryVisibility::Hidden,
            "a valid test-plane caller must authorize against the public IAM organization"
        );
    }
    Ok(())
}

async fn stored_secrets(
    pool: &PgPool,
    control_context: &ExecutionContext,
    environment_id: Uuid,
) -> anyhow::Result<EncryptedEnvironmentRow> {
    let tenant = TenantContext::from_execution(control_context);
    let mut transaction = begin_tenant_transaction(pool, &tenant).await?;
    let row = sqlx::query_as::<_, EncryptedEnvironmentRow>(
        "SELECT root_key_digest, root_key_ciphertext, iam_environment_key_digest, \
                iam_environment_key_ciphertext, iam_app_secret_ciphertext \
           FROM briefcase.testing_environments \
          WHERE environment_id = $1 AND org_id = briefcase.current_org_id()",
    )
    .bind(environment_id)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(row)
}

async fn stored_idempotency_response(
    pool: &PgPool,
    control_context: &ExecutionContext,
    environment_id: Uuid,
) -> anyhow::Result<Vec<u8>> {
    let tenant = TenantContext::from_execution(control_context);
    let mut transaction = begin_tenant_transaction(pool, &tenant).await?;
    let response = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT response_ciphertext \
           FROM briefcase.testing_environment_idempotency \
          WHERE org_id = briefcase.current_org_id() AND environment_id = $1 \
            AND operation = 'testing_environment.create' AND status = 'completed'",
    )
    .bind(environment_id)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(response)
}

async fn stored_last_activity(
    pool: &PgPool,
    control_context: &ExecutionContext,
    environment_id: Uuid,
) -> anyhow::Result<time::OffsetDateTime> {
    let tenant = TenantContext::from_execution(control_context);
    let mut transaction = begin_tenant_transaction(pool, &tenant).await?;
    let last_activity = sqlx::query_scalar::<_, time::OffsetDateTime>(
        "SELECT last_activity_at FROM briefcase.testing_environments \
          WHERE org_id = briefcase.current_org_id() AND environment_id = $1",
    )
    .bind(environment_id)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(last_activity)
}

async fn expire_clean_idempotency_lease(
    pool: &PgPool,
    control_context: &ExecutionContext,
    environment_id: Uuid,
) -> anyhow::Result<()> {
    let tenant = TenantContext::from_execution(control_context);
    let mut transaction = begin_tenant_transaction(pool, &tenant).await?;
    let updated = sqlx::query(
        "UPDATE briefcase.testing_environment_idempotency \
            SET locked_until = clock_timestamp() - INTERVAL '1 second' \
          WHERE org_id = briefcase.current_org_id() AND environment_id = $1 \
            AND operation = 'testing_environment.clean' AND status = 'in_progress'",
    )
    .bind(environment_id)
    .execute(&mut *transaction)
    .await?;
    anyhow::ensure!(
        updated.rows_affected() == 1,
        "active clean lease was not found"
    );
    transaction.commit().await?;
    Ok(())
}

async fn seed_provider_state(
    data: &PgPool,
    context: &ExecutionContext,
    label: &str,
) -> anyhow::Result<(String, String, String, String)> {
    let environment_id = context
        .testing_environment()
        .ok_or_else(|| anyhow::anyhow!("test context is missing its environment"))?
        .id();
    let tenant = TenantContext::from_execution(context);
    let mut transaction = begin_tenant_transaction(data, &tenant).await?;
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *transaction)
        .await?;
    let public_root = sqlx::query_scalar::<_, Uuid>(
        "SELECT entry_id FROM briefcase.entries \
          WHERE org_id = briefcase.current_org_id() AND system_kind = 'public_root'",
    )
    .fetch_one(&mut *transaction)
    .await?;

    let entry_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let object_key = format!("objects/{entry_id}/{version_id}");
    let prefix = format!("testing/{environment_id}");
    sqlx::query(
        "INSERT INTO briefcase.entries ( \
             org_id, entry_id, parent_id, entry_type, name, root_type, owner_type, owner_id, \
             content_type, size_bytes, current_version_id, created_by_type, created_by_id, \
             updated_by_type, updated_by_id \
         ) VALUES ( \
             briefcase.current_org_id(), $1, $2, 'file', $3, 'public', \
             'carbon', $4, 'text/plain', 1, $5, 'carbon', $4, 'carbon', $4 \
         )",
    )
    .bind(entry_id)
    .bind(public_root)
    .bind(format!("{label}.txt"))
    .bind(context.authorization().actor().id().as_str())
    .bind(version_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO briefcase.entry_versions ( \
             org_id, entry_id, version_id, version_number, source, storage_backend, \
             bucket_name, storage_region, storage_prefix, storage_encryption_mode, object_key, \
             object_version_id, checksum_algorithm, checksum_type, checksum_value, size_bytes, \
             content_type, created_by_type, created_by_id \
         ) VALUES ( \
             briefcase.current_org_id(), $1, $2, 1, 'upload', 'platform', 'test-bucket', \
             'us-east-1', $3, 'sse_s3', $4, 'object-version-1', 'sha256', 'full_object', \
             'test-checksum', 1, 'text/plain', 'carbon', $5 \
         )",
    )
    .bind(entry_id)
    .bind(version_id)
    .bind(&prefix)
    .bind(&object_key)
    .bind(context.authorization().actor().id().as_str())
    .execute(&mut *transaction)
    .await?;

    let upload_id = Uuid::now_v7();
    let multipart_key = format!("multiparts/{upload_id}");
    let provider_upload_id = format!("provider-{upload_id}");
    sqlx::query(
        "INSERT INTO briefcase.multipart_uploads ( \
             org_id, upload_id, parent_entry_id, owner_type, owner_id, name, content_type, \
             declared_size_bytes, part_size_bytes, expected_part_count, storage_backend, \
             bucket_name, storage_region, storage_prefix, storage_encryption_mode, object_key, \
             provider_upload_id, expires_at \
         ) VALUES ( \
             briefcase.current_org_id(), $1, $2, 'carbon', $3, $4, \
             'application/octet-stream', 104857601, 8388608, 13, 'platform', 'test-bucket', \
             'us-east-1', $5, 'sse_s3', $6, $7, clock_timestamp() + INTERVAL '1 hour' \
         )",
    )
    .bind(upload_id)
    .bind(public_root)
    .bind(context.authorization().actor().id().as_str())
    .bind(format!("{label}.bin"))
    .bind(&prefix)
    .bind(&multipart_key)
    .bind(&provider_upload_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((prefix, object_key, multipart_key, provider_upload_id))
}

async fn seed_claimed_version_cleanup(
    data: &PgPool,
    context: &ExecutionContext,
    object_key: &str,
) -> anyhow::Result<(Uuid, Uuid)> {
    let cleanup_id = Uuid::now_v7();
    let lease_token = Uuid::now_v7();
    let tenant = TenantContext::from_execution(context);
    let mut transaction = begin_tenant_transaction(data, &tenant).await?;
    let inserted = sqlx::query(
        "INSERT INTO briefcase.object_cleanup_jobs ( \
             org_id, cleanup_id, cleanup_kind, source_entry_id, source_version_id, \
             source_upload_id, deletion_batch_id, storage_backend, storage_config_id, \
             bucket_name, storage_region, storage_prefix, storage_role_arn, \
             storage_encryption_mode, storage_kms_key_arn, object_key, \
             object_version_id, provider_upload_id \
         ) \
         SELECT version.org_id, $1, 'version_delete', version.entry_id, version.version_id, \
                NULL, entry.deletion_batch_id, version.storage_backend, \
                version.storage_config_id, version.bucket_name, version.storage_region, \
                version.storage_prefix, configuration.role_arn, \
                version.storage_encryption_mode, version.storage_kms_key_arn, \
                version.object_key, version.object_version_id, NULL \
           FROM briefcase.entry_versions AS version \
           JOIN briefcase.entries AS entry \
             ON entry.org_id = version.org_id AND entry.entry_id = version.entry_id \
           LEFT JOIN briefcase.organization_storage_configs AS configuration \
             ON configuration.org_id = version.org_id \
            AND configuration.storage_config_id = version.storage_config_id \
          WHERE version.org_id = briefcase.current_org_id() AND version.object_key = $2",
    )
    .bind(cleanup_id)
    .bind(object_key)
    .execute(&mut *transaction)
    .await?;
    anyhow::ensure!(
        inserted.rows_affected() == 1,
        "cleanup source was not found"
    );
    let claimed = sqlx::query(
        "UPDATE briefcase.object_cleanup_jobs \
            SET status = 'processing', attempt_count = 1, lease_token = $2, \
                lease_expires_at = clock_timestamp() + INTERVAL '5 minutes' \
          WHERE org_id = briefcase.current_org_id() AND cleanup_id = $1",
    )
    .bind(cleanup_id)
    .bind(lease_token)
    .execute(&mut *transaction)
    .await?;
    anyhow::ensure!(
        claimed.rows_affected() == 1,
        "cleanup claim was not created"
    );
    transaction.commit().await?;
    Ok((cleanup_id, lease_token))
}

async fn cancel_claim(
    data: &PgPool,
    context: &ExecutionContext,
    cleanup_id: Uuid,
    lease_token: Uuid,
) -> anyhow::Result<u64> {
    let environment_id = context
        .testing_environment()
        .ok_or_else(|| anyhow::anyhow!("test context is missing its environment"))?
        .id();
    let org_id = format!(
        "{environment_id}:{}",
        context.authorization().organization_id().as_str()
    );
    let cancelled = sqlx::query(
        "DELETE FROM briefcase.object_cleanup_jobs \
          WHERE org_id = $1 AND cleanup_id = $2 \
            AND status = 'processing' AND lease_token = $3",
    )
    .bind(org_id)
    .bind(cleanup_id)
    .bind(lease_token)
    .execute(data)
    .await?;
    Ok(cancelled.rows_affected())
}

async fn cleanup_claim_state(
    data: &PgPool,
    context: &ExecutionContext,
    cleanup_id: Uuid,
) -> anyhow::Result<Option<(String, Option<Uuid>)>> {
    let environment_id = context
        .testing_environment()
        .ok_or_else(|| anyhow::anyhow!("test context is missing its environment"))?
        .id();
    let org_id = format!(
        "{environment_id}:{}",
        context.authorization().organization_id().as_str()
    );
    let state = sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT status, lease_token FROM briefcase.object_cleanup_jobs \
          WHERE org_id = $1 AND cleanup_id = $2",
    )
    .bind(org_id)
    .bind(cleanup_id)
    .fetch_optional(data)
    .await?;
    Ok(state)
}

async fn cleanable_row_count(data: &PgPool, context: &ExecutionContext) -> anyhow::Result<i64> {
    let tenant = TenantContext::from_execution(context);
    let mut transaction = begin_tenant_transaction(data, &tenant).await?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT \
            (SELECT count(*) FROM briefcase.notifications WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.access_requests WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.permission_grants WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.multipart_parts WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.multipart_uploads WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.search_documents WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.entry_closure WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.idempotency_records WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.audit_events WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.outbox_events WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.webhook_receipts WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.object_cleanup_jobs \
              WHERE org_id = briefcase.current_org_id() AND status = 'object_deleted') + \
            (SELECT count(*) FROM briefcase.entry_versions WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.entries WHERE org_id = briefcase.current_org_id()) + \
            (SELECT count(*) FROM briefcase.organization_storage_configs WHERE org_id = briefcase.current_org_id())",
    )
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(count)
}

async fn queued_cleanup_rows(
    data: &PgPool,
    context: &ExecutionContext,
) -> anyhow::Result<Vec<QueuedCleanupRow>> {
    let tenant = TenantContext::from_execution(context);
    let mut transaction = begin_tenant_transaction(data, &tenant).await?;
    let rows = sqlx::query_as::<_, QueuedCleanupRow>(
        "SELECT cleanup_kind, storage_prefix, object_key, object_version_id, \
                provider_upload_id \
           FROM briefcase.object_cleanup_jobs \
          WHERE org_id = briefcase.current_org_id() \
          ORDER BY cleanup_kind, object_key",
    )
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(rows)
}

async fn mark_multipart_aborted(
    data: &PgPool,
    context: &ExecutionContext,
    provider_upload_id: &str,
) -> anyhow::Result<()> {
    let tenant = TenantContext::from_execution(context);
    let mut transaction = begin_tenant_transaction(data, &tenant).await?;
    sqlx::query(
        "UPDATE briefcase.multipart_uploads \
            SET status = 'aborted', aborted_at = clock_timestamp() \
          WHERE org_id = briefcase.current_org_id() AND provider_upload_id = $1",
    )
    .bind(provider_upload_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn assert_namespaced_and_isolated(
    data: &PgPool,
    organization: &str,
    environment_id: Uuid,
    other_environment_id: Uuid,
) -> anyhow::Result<()> {
    let actor_id = format!("carbon:{organization}");
    let context = execution(
        organization,
        &actor_id,
        Some(TestingEnvironmentContext::new(environment_id, 1)),
    )?;
    let tenant = TenantContext::from_execution(&context);
    let mut transaction = begin_tenant_transaction(data, &tenant).await?;

    // The compose administrator is a superuser. Assume the deliberately
    // NOBYPASSRLS runtime role for this assertion so it verifies the policy,
    // not merely the explicit predicates used by repository queries.
    let current_is_privileged = sqlx::query_scalar::<_, bool>(
        "SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(&mut *transaction)
    .await?;
    if current_is_privileged {
        let runtime_role_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'briefcase_api')",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if runtime_role_exists {
            sqlx::query("SET LOCAL ROLE briefcase_api")
                .execute(&mut *transaction)
                .await?;
        }
    }

    let expected_org_id = format!("{environment_id}:{organization}");
    let observed = sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT org_id, testing_environment_id FROM briefcase.organizations",
    )
    .fetch_all(&mut *transaction)
    .await?;
    assert_eq!(observed, vec![(expected_org_id, Some(environment_id))]);
    let leaked = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM briefcase.organizations \
          WHERE testing_environment_id = $1",
    )
    .bind(other_environment_id)
    .fetch_one(&mut *transaction)
    .await?;
    assert_eq!(leaked, 0, "the other sandbox is hidden by tenant RLS");
    transaction.commit().await?;
    Ok(())
}

async fn assert_test_storage_limit(
    data: &PgPool,
    context: &ExecutionContext,
) -> anyhow::Result<()> {
    let tenant = TenantContext::from_execution(context);
    let mut transaction = begin_tenant_transaction(data, &tenant).await?;
    let limit = sqlx::query_scalar::<_, i64>(
        "SELECT storage_limit_bytes FROM briefcase.organization_usage \
          WHERE org_id = briefcase.current_org_id()",
    )
    .fetch_optional(&mut *transaction)
    .await?;
    transaction.commit().await?;
    assert_eq!(
        limit,
        Some(TEST_STORAGE_LIMIT_BYTES),
        "every path that materializes a test organization must install its fixed 2-GiB ceiling"
    );
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn hard_cleanup(
    production: &PgPool,
    data: &PgPool,
    organization: &str,
    actor_id: &str,
    environment_ids: &[Uuid],
) -> anyhow::Result<()> {
    for environment_id in environment_ids {
        let test_context = execution(
            organization,
            actor_id,
            Some(TestingEnvironmentContext::new(*environment_id, 1)),
        )?;
        let test_tenant = TenantContext::from_execution(&test_context);
        let mut test_transaction = begin_tenant_transaction(data, &test_tenant).await?;
        // Keep teardown reliable even when this test has just exposed a bug in
        // the production cleaner. The version/member references form a cycle
        // around entries, so remove version and multipart children explicitly
        // before deleting the owning organization.
        sqlx::query("SET CONSTRAINTS ALL DEFERRED")
            .execute(&mut *test_transaction)
            .await?;
        sqlx::query(
            "DELETE FROM briefcase.notifications WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query(
            "DELETE FROM briefcase.access_requests WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query(
            "DELETE FROM briefcase.permission_grants WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query(
            "DELETE FROM briefcase.multipart_parts WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query(
            "DELETE FROM briefcase.multipart_uploads WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query(
            "DELETE FROM briefcase.search_documents WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query(
            "DELETE FROM briefcase.entry_closure WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query(
            "DELETE FROM briefcase.idempotency_records WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query("DELETE FROM briefcase.audit_events WHERE org_id = briefcase.current_org_id()")
            .execute(&mut *test_transaction)
            .await?;
        sqlx::query(
            "DELETE FROM briefcase.outbox_events WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query(
            "DELETE FROM briefcase.webhook_receipts WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query(
            "DELETE FROM briefcase.object_cleanup_jobs WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query(
            "DELETE FROM briefcase.entry_versions WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query("DELETE FROM briefcase.entries WHERE org_id = briefcase.current_org_id()")
            .execute(&mut *test_transaction)
            .await?;
        sqlx::query(
            "DELETE FROM briefcase.organization_storage_configs \
              WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        sqlx::query(
            "DELETE FROM briefcase.organizations WHERE org_id = briefcase.current_org_id()",
        )
        .execute(&mut *test_transaction)
        .await?;
        test_transaction.commit().await?;
    }

    let control_context = execution(organization, actor_id, None)?;
    let control_tenant = TenantContext::from_execution(&control_context);
    let mut transaction = begin_tenant_transaction(production, &control_tenant).await?;
    sqlx::query(
        "DELETE FROM briefcase.testing_environments \
          WHERE org_id = briefcase.current_org_id() AND environment_id = ANY($1)",
    )
    .bind(environment_ids)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("DELETE FROM briefcase.organizations WHERE org_id = briefcase.current_org_id()")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[allow(
    clippy::manual_assert_eq,
    reason = "credential comparisons must not include plaintext values in panic output"
)]
async fn assert_initialized_pairing_is_preserved(
    store: &TestingEnvironmentStore,
    repository: &PostgresRepository,
    control_context: &ExecutionContext,
    owner_context: &ExecutionContext,
    environment: &TestingEnvironmentWithKey,
    entry_id: EntryId,
) -> anyhow::Result<()> {
    let environment_id = environment.environment.id;
    let existing = store
        .resolve_root_key(&SecretString::from(environment.key.clone()))
        .await?;
    let different_realm = TestingEnvironmentIamPairing {
        iam_environment_id: Uuid::now_v7(),
        iam_environment_key: SecretString::from(Uuid::new_v4().simple().to_string()),
        iam_app_id: existing.iam_app_id.clone(),
        iam_app_secret: SecretString::from(REPLACEMENT_IAM_APPLICATION_SECRET),
    };
    let rejected_mutation = mutation(format!("reject-realm-{environment_id}"), b"reject-realm")?;
    let rejected = store
        .replace_iam_pairing(
            control_context,
            environment_id,
            &different_realm,
            &rejected_mutation,
        )
        .await;
    let Err(error) = rejected else {
        panic!("an initialized plane must not switch immutable IAM identities");
    };
    assert_conflict(
        error,
        "testing_environment_iam_rebind_requires_new_environment",
    );
    assert!(
        store
            .replay_iam_pairing(control_context, environment_id, &rejected_mutation)
            .await?
            .is_none()
    );
    let unchanged = store
        .resolve_root_key(&SecretString::from(environment.key.clone()))
        .await?;
    assert_eq!(unchanged.control_version, existing.control_version);
    assert_eq!(unchanged.iam_environment_id, existing.iam_environment_id);
    assert_eq!(unchanged.key_generation, existing.key_generation);
    assert!(
        unchanged.iam_environment_key.expose_secret()
            == existing.iam_environment_key.expose_secret()
    );
    assert!(unchanged.iam_app_secret.expose_secret() == existing.iam_app_secret.expose_secret());
    assert!(
        repository
            .find_active_entry(owner_context, entry_id)
            .await?
            .is_some()
    );

    let rotated_input = TestingEnvironmentIamPairing {
        iam_environment_id: existing.iam_environment_id,
        ..different_realm
    };
    let rotation = mutation(format!("rotate-realm-{environment_id}"), b"rotate-realm")?;
    let rotated = store
        .replace_iam_pairing(control_context, environment_id, &rotated_input, &rotation)
        .await?;
    assert_eq!(rotated.version, existing.control_version + 1);
    assert_eq!(rotated.iam_environment_id, existing.iam_environment_id);
    assert_eq!(rotated.key_generation, existing.key_generation);
    let replayed = store
        .replace_iam_pairing(control_context, environment_id, &rotated_input, &rotation)
        .await?;
    assert_eq!(replayed.version, rotated.version);
    let fresh = execution(
        owner_context.authorization().organization_id().as_str(),
        owner_context.authorization().actor().id().as_str(),
        Some(TestingEnvironmentContext::new(
            environment_id,
            rotated.version,
        )),
    )?;
    assert!(
        repository
            .find_active_entry(&fresh, entry_id)
            .await?
            .is_some()
    );
    let grants = repository
        .list_permission_grants(
            &fresh,
            &ListPermissionsQuery {
                entry_id,
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    assert_eq!(
        grants.items.len(),
        1,
        "same-realm rotation must preserve grants"
    );
    Ok(())
}

async fn identity_probe_under_rls(
    data: &PgPool,
    context: &ExecutionContext,
) -> anyhow::Result<(bool, i64)> {
    let tenant = TenantContext::from_execution(context);
    let mut transaction = begin_tenant_transaction(data, &tenant).await?;
    sqlx::query("SET LOCAL ROLE briefcase_api")
        .execute(&mut *transaction)
        .await?;
    let result = sqlx::query_as::<_, (bool, i64)>(
        "SELECT briefcase.current_testing_environment_has_iam_projection(), \
                (SELECT count(*) FROM briefcase.organizations)",
    )
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(result)
}

async fn assert_identity_probe_rejects_context(
    data: &PgPool,
    context: &ExecutionContext,
    mismatched_namespace: bool,
) -> anyhow::Result<()> {
    let tenant = TenantContext::from_execution(context);
    let mut transaction = begin_tenant_transaction(data, &tenant).await?;
    sqlx::query("SET LOCAL ROLE briefcase_api")
        .execute(&mut *transaction)
        .await?;
    if mismatched_namespace {
        sqlx::query("SELECT set_config('briefcase.org_id', 'wrong-namespace', true)")
            .execute(&mut *transaction)
            .await?;
    }
    let result = sqlx::query_scalar::<_, bool>(
        "SELECT briefcase.current_testing_environment_has_iam_projection()",
    )
    .fetch_one(&mut *transaction)
    .await;
    transaction.rollback().await?;
    let Err(error) = result else {
        panic!("the projection probe must reject missing or mismatched sandbox context");
    };
    assert_eq!(
        error
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("42501")
    );
    Ok(())
}

async fn set_identity_probe_fixture(
    data: &PgPool,
    context: &ExecutionContext,
    present: bool,
) -> anyhow::Result<()> {
    let tenant = TenantContext::from_execution(context);
    let mut transaction = begin_tenant_transaction(data, &tenant).await?;
    let statement = if present {
        "INSERT INTO briefcase.organizations (org_id) VALUES (briefcase.current_org_id())"
    } else {
        "DELETE FROM briefcase.organizations WHERE org_id = briefcase.current_org_id()"
    };
    sqlx::query(statement).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn assert_foreign_projection_prevents_rebinding(
    data: &PgPool,
    store: &TestingEnvironmentStore,
    control_context: &ExecutionContext,
    environment: &TestingEnvironmentWithKey,
) -> anyhow::Result<()> {
    let environment_id = environment.environment.id;
    let selector = TestingEnvironmentContext::new(environment_id, environment.environment.version);
    let owner_org = control_context.authorization().organization_id().as_str();
    let actor = control_context.authorization().actor().id().as_str();
    let owner_context = execution(owner_org, actor, Some(selector))?;
    let foreign_org = format!("foreign-{}", Uuid::new_v4().simple());
    let foreign_context = execution(&foreign_org, actor, Some(selector))?;
    let other_context = execution(
        &foreign_org,
        actor,
        Some(TestingEnvironmentContext::new(Uuid::now_v7(), 1)),
    )?;
    let outcome = AssertUnwindSafe(async {
        assert_identity_probe_rejects_context(data, control_context, false).await?;
        assert_identity_probe_rejects_context(data, &owner_context, true).await?;
        set_identity_probe_fixture(data, &other_context, true).await?;
        assert_eq!(
            identity_probe_under_rls(data, &owner_context).await?,
            (false, 0),
            "another environment must not block a genuinely empty plane"
        );
        set_identity_probe_fixture(data, &foreign_context, true).await?;
        assert_eq!(
            identity_probe_under_rls(data, &owner_context).await?,
            (true, 0),
            "the boolean probe must see foreign projections without exposing their rows"
        );
        let pairing = TestingEnvironmentIamPairing {
            iam_environment_id: Uuid::now_v7(),
            iam_environment_key: SecretString::from(Uuid::new_v4().simple().to_string()),
            iam_app_id: environment.environment.iam_app_id.clone(),
            iam_app_secret: SecretString::from(REPLACEMENT_IAM_APPLICATION_SECRET),
        };
        let metadata = mutation(format!("foreign-realm-{environment_id}"), b"foreign-realm")?;
        let Err(error) = store
            .replace_iam_pairing(control_context, environment_id, &pairing, &metadata)
            .await
        else {
            panic!("a non-owner organization projection must block IAM realm replacement");
        };
        assert_conflict(
            error,
            "testing_environment_iam_rebind_requires_new_environment",
        );
        let unchanged = store.get(control_context, environment_id).await?;
        assert_eq!(unchanged.version, environment.environment.version);
        assert_eq!(
            unchanged.iam_environment_id,
            environment.environment.iam_environment_id
        );
        Ok::<(), anyhow::Error>(())
    })
    .catch_unwind()
    .await;
    let foreign_cleanup = set_identity_probe_fixture(data, &foreign_context, false).await;
    let other_cleanup = set_identity_probe_fixture(data, &other_context, false).await;
    match outcome {
        Ok(result) => {
            foreign_cleanup?;
            other_cleanup?;
            result
        }
        Err(panic) => resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn sandbox_entries_and_grants_use_the_public_organization() -> anyhow::Result<()> {
    let Ok(control_url) = std::env::var("BRIEFCASE_TEST_CONTROL_DATABASE_URL") else {
        eprintln!("skipping: BRIEFCASE_TEST_CONTROL_DATABASE_URL is not set");
        return Ok(());
    };
    let Ok(data_url) = std::env::var("BRIEFCASE_TEST_DATA_DATABASE_URL") else {
        eprintln!("skipping: BRIEFCASE_TEST_DATA_DATABASE_URL is not set");
        return Ok(());
    };
    anyhow::ensure!(
        control_url != data_url,
        "control and sandbox tests require separate PostgreSQL databases"
    );

    let production = postgres::connect(&settings(control_url), "briefcase-grant-control").await?;
    let data = postgres::connect(&settings(data_url), "briefcase-grant-data").await?;
    postgres::migrate(&production).await?;
    postgres::migrate(&data).await?;

    let suffix = Uuid::new_v4().simple().to_string();
    let organization = format!("grant-{suffix}");
    let owner_id = format!("owner:{suffix}");
    let peer_id = format!("peer:{suffix}");
    let control_context = execution(&organization, &owner_id, None)?;
    let repository = PostgresRepository::new(production.clone()).with_test_pool(data.clone());
    reconcile_roots(&repository, &control_context).await?;
    let master_key = SecretString::from("MDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDA=");
    let store = TestingEnvironmentStore::new(production.clone(), data.clone(), &master_key)?;
    let environment = store
        .create(
            &control_context,
            &create_input(&organization, format!("grant-environment-{suffix}")),
            &mutation(format!("grant-create-{suffix}"), b"grant-create")?,
        )
        .await?;
    let environment_id = environment.environment.id;
    let selector = TestingEnvironmentContext::new(environment_id, environment.environment.version);
    let owner_context = execution(&organization, &owner_id, Some(selector))?;
    let peer_context = execution(&organization, &peer_id, Some(selector))?;

    let result = AssertUnwindSafe(async {
        reconcile_roots(&repository, &owner_context).await?;
        reconcile_roots(&repository, &peer_context).await?;

        let private_container = repository
            .find_boundary_container(&owner_context, &EntryBoundary::Private)
            .await?
            .ok_or_else(|| anyhow::anyhow!("owner private container was not reconciled"))?;
        let entry_id = EntryId::new();
        let created = repository
            .create_folder(
                &owner_context,
                &CreateFolderMutation {
                    entry_id,
                    command: CreateFolderCommand::new(
                        EntryName::new(format!("shared-{suffix}"))?,
                        Some(private_container.entry.id),
                        None,
                        Vec::new(),
                    )?,
                    boundary: EntryBoundary::Private,
                    owner: owner_context.authorization().actor().clone(),
                    origin_application_id: None,
                },
                &mutation(format!("grant-folder-{suffix}"), b"grant-folder")?,
                Some(Capability::CreateChild),
            )
            .await?;
        assert_eq!(
            &created.entry.organization_id,
            owner_context.authorization().organization_id()
        );

        let grant = repository
            .grant_permission(
                &owner_context,
                &GrantPermissionCommand {
                    entry_id,
                    principal: peer_context.authorization().actor().clone(),
                    access: GrantedAccess::READ_ONLY,
                    inherits_to_descendants: false,
                },
                &mutation(format!("grant-peer-{suffix}"), b"grant-peer")?,
                Capability::ManagePermissions,
            )
            .await?;
        assert_eq!(
            grant.organization_id(),
            owner_context.authorization().organization_id()
        );

        let grants = repository
            .list_permission_grants(
                &owner_context,
                &ListPermissionsQuery {
                    entry_id,
                    page: PageRequest::new(None, 100)?,
                },
            )
            .await?;
        assert_eq!(grants.items.len(), 1);
        assert_eq!(
            grants.items[0].organization_id(),
            owner_context.authorization().organization_id()
        );

        let visible_to_peer = repository
            .find_active_entry(&peer_context, entry_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("granted entry was not found"))?;
        let authorization = visible_to_peer.authorization(peer_context.authorization());
        assert_eq!(authorization.visibility(), EntryVisibility::Full);
        assert!(authorization.allows(Capability::Read));
        assert_initialized_pairing_is_preserved(
            &store,
            &repository,
            &control_context,
            &owner_context,
            &environment,
            entry_id,
        )
        .await?;
        Ok(())
    })
    .catch_unwind()
    .await;

    let cleanup = hard_cleanup(
        &production,
        &data,
        &organization,
        &owner_id,
        &[environment_id],
    )
    .await;
    match result {
        Ok(result) => {
            cleanup?;
            result
        }
        Err(panic) => {
            if let Err(error) = cleanup {
                eprintln!("sandbox grant cleanup also failed: {error:#}");
            }
            resume_unwind(panic)
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn testing_environments_are_encrypted_idempotent_and_isolated() -> anyhow::Result<()> {
    let Ok(control_url) = std::env::var("BRIEFCASE_TEST_CONTROL_DATABASE_URL") else {
        eprintln!("skipping: BRIEFCASE_TEST_CONTROL_DATABASE_URL is not set");
        return Ok(());
    };
    let Ok(data_url) = std::env::var("BRIEFCASE_TEST_DATA_DATABASE_URL") else {
        eprintln!("skipping: BRIEFCASE_TEST_DATA_DATABASE_URL is not set");
        return Ok(());
    };
    anyhow::ensure!(
        control_url != data_url,
        "control and sandbox tests require separate PostgreSQL databases"
    );

    let production = postgres::connect(&settings(control_url), "briefcase-test-control").await?;
    let data = postgres::connect(&settings(data_url), "briefcase-test-data").await?;
    postgres::migrate(&production).await?;
    postgres::migrate(&data).await?;

    let suffix = Uuid::new_v4().simple().to_string();
    let organization = format!("tst-{suffix}");
    let actor_id = format!("carbon:{suffix}");
    let control_context = execution(&organization, &actor_id, None)?;
    let repository = PostgresRepository::new(production.clone()).with_test_pool(data.clone());
    reconcile_roots(&repository, &control_context).await?;

    let active_before =
        sqlx::query_scalar::<_, i64>("SELECT briefcase.active_testing_environment_count()")
            .fetch_one(&production)
            .await?;
    let delete_barrier = Arc::new(Barrier::new(2));
    let object_store = Arc::new(RecordingObjectStore {
        calls: Mutex::default(),
        delete_barrier: Some(Arc::clone(&delete_barrier)),
        cleanup_returns_not_found: true,
    });
    let master_key = SecretString::from("MDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDA=");
    let store = TestingEnvironmentStore::new(production.clone(), data.clone(), &master_key)?;

    let mut created_environment_ids = Vec::new();
    let result = AssertUnwindSafe(async {
        let create_a = create_input(&organization, format!("environment-a-{suffix}"));
        let create_a_mutation = mutation(format!("create-a-{suffix}"), b"create-a")?;
        let environment_a = store
            .create(&control_context, &create_a, &create_a_mutation)
            .await?;
        created_environment_ids.push(environment_a.environment.id);
        assert_eq!(environment_a.key.len(), 32);
        assert!(environment_a.key.bytes().all(|byte| byte.is_ascii_alphanumeric()));
        assert_eq!(environment_a.environment.status, TestingEnvironmentStatus::Active);
        assert_eq!(environment_a.environment.created_by.actor_type, "carbon");
        assert_eq!(environment_a.environment.created_by.id, actor_id);

        let create_a_replay = store
            .create(&control_context, &create_a, &create_a_mutation)
            .await?;
        assert_same_secret_response(&environment_a, &create_a_replay);
        let create_a_early_replay = store
            .replay_create(&control_context, &create_a_mutation)
            .await?
            .ok_or_else(|| anyhow::anyhow!("completed create must be recoverable pre-validation"))?;
        assert_same_secret_response(&environment_a, &create_a_early_replay);

        let changed_fingerprint = mutation(format!("create-a-{suffix}"), b"changed-create-a")?;
        let Err(conflict) = store
            .replay_create(&control_context, &changed_fingerprint)
            .await
        else {
            panic!("pre-validation replay must bind the exact request body");
        };
        assert_conflict(conflict, "idempotency_key_reused");
        let changed_input = create_input(&organization, format!("changed-a-{suffix}"));
        let Err(conflict) = store
            .create(&control_context, &changed_input, &changed_fingerprint)
            .await
        else {
            panic!("a key cannot be reused for a changed create payload");
        };
        assert_conflict(conflict, "idempotency_key_reused");

        let encrypted = stored_secrets(
            &production,
            &control_context,
            environment_a.environment.id,
        )
        .await?;
        assert_eq!(encrypted.root_key_digest.len(), 32);
        assert_eq!(encrypted.iam_environment_key_digest.len(), 32);
        assert!(!contains_bytes(
            &encrypted.root_key_ciphertext,
            environment_a.key.as_bytes()
        ));
        assert!(!contains_bytes(
            &encrypted.iam_environment_key_ciphertext,
            create_a.iam_environment_key.expose_secret().as_bytes()
        ));
        assert!(!contains_bytes(
            &encrypted.iam_app_secret_ciphertext,
            create_a.iam_app_secret.expose_secret().as_bytes()
        ));
        let encrypted_replay = stored_idempotency_response(
            &production,
            &control_context,
            environment_a.environment.id,
        )
        .await?;
        assert!(
            !contains_bytes(&encrypted_replay, environment_a.key.as_bytes()),
            "a replayable one-time root key must also be encrypted at rest"
        );

        Box::pin(assert_foreign_projection_prevents_rebinding(
            &data,
            &store,
            &control_context,
            &environment_a,
        ))
        .await?;
        let replacement_pairing = TestingEnvironmentIamPairing {
            iam_environment_id: Uuid::now_v7(),
            iam_environment_key: SecretString::from(Uuid::new_v4().simple().to_string()),
            iam_app_id: create_a.iam_app_id.clone(),
            iam_app_secret: SecretString::from(REPLACEMENT_IAM_APPLICATION_SECRET),
        };
        let pairing_mutation = mutation(format!("pair-a-{suffix}"), b"pair-a")?;
        assert!(
            store
                .replay_iam_pairing(
                    &control_context,
                    environment_a.environment.id,
                    &pairing_mutation,
                )
                .await?
                .is_none(),
            "an unclaimed replacement cannot be replayed"
        );
        let pre_pair_access = store
            .resolve_root_key(&SecretString::from(environment_a.key.clone()))
            .await?;
        let (candidate_seen_tx, candidate_seen_rx) = mpsc::channel();
        let (candidate_release_tx, candidate_release_rx) = mpsc::channel();
        let old_iam_key = create_a.iam_environment_key.expose_secret().to_owned();
        let webhook_store = store.clone();
        let stale_webhook_task = tokio::spawn(async move {
            webhook_store
                .resolve_iam_webhook(move |candidate| {
                    if candidate.expose_secret() != old_iam_key.as_str() {
                        return false;
                    }
                    if candidate_seen_tx.send(()).is_err() {
                        return false;
                    }
                    tokio::task::block_in_place(|| candidate_release_rx.recv().is_ok())
                })
                .await
        });
        tokio::task::spawn_blocking(move || candidate_seen_rx.recv()).await??;

        // A credential-sensitive request that already accepted this generation
        // keeps a concurrent re-pair behind the same environment fence.
        let use_fence = store.acquire_use_fence(&pre_pair_access).await?;
        let pairing_store = store.clone();
        let pairing_context = control_context.clone();
        let pairing_input = replacement_pairing.clone();
        let pairing_metadata = pairing_mutation.clone();
        let environment_a_id = environment_a.environment.id;
        let mut pairing_task = tokio::spawn(async move {
            pairing_store
                .replace_iam_pairing(
                    &pairing_context,
                    environment_a_id,
                    &pairing_input,
                    &pairing_metadata,
                )
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut pairing_task)
                .await
                .is_err(),
            "re-pairing must wait for an accepted old-generation request"
        );
        use_fence.release().await?;
        let paired_result = pairing_task.await;
        candidate_release_tx.send(())?;
        let paired_a = paired_result??;
        assert!(
            stale_webhook_task.await??.is_none(),
            "a webhook key selected before re-pair must fail its exact-generation touch"
        );
        assert!(matches!(
            store.touch(&pre_pair_access).await,
            Err(AppError::Unauthenticated)
        ));
        assert_eq!(paired_a.version, environment_a.environment.version + 1);
        assert_eq!(
            paired_a.key_generation,
            environment_a.environment.key_generation,
            "re-pairing must preserve the Briefcase root key"
        );
        assert_eq!(
            paired_a.iam_environment_id,
            replacement_pairing.iam_environment_id
        );
        let paired_replay = store
            .replay_iam_pairing(
                &control_context,
                environment_a.environment.id,
                &pairing_mutation,
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("completed pairing must replay pre-validation"))?;
        assert_eq!(paired_replay.version, paired_a.version);
        let paired_mutation_replay = store
            .replace_iam_pairing(
                &control_context,
                environment_a.environment.id,
                &replacement_pairing,
                &pairing_mutation,
            )
            .await?;
        assert_eq!(paired_mutation_replay.version, paired_a.version);
        let paired_secrets = stored_secrets(
            &production,
            &control_context,
            environment_a.environment.id,
        )
        .await?;
        assert!(!contains_bytes(
            &paired_secrets.iam_environment_key_ciphertext,
            replacement_pairing
                .iam_environment_key
                .expose_secret()
                .as_bytes(),
        ));
        assert!(!contains_bytes(
            &paired_secrets.iam_app_secret_ciphertext,
            replacement_pairing.iam_app_secret.expose_secret().as_bytes(),
        ));
        assert!(
            store
                .resolve_iam_webhook(|candidate| {
                    candidate.expose_secret() == create_a.iam_environment_key.expose_secret()
                })
                .await?
                .is_none(),
            "the prior IAM root must stop routing webhooks immediately"
        );
        let replacement_webhook = store
            .resolve_iam_webhook(|candidate| {
                candidate.expose_secret()
                    == replacement_pairing.iam_environment_key.expose_secret()
            })
            .await?
            .ok_or_else(|| anyhow::anyhow!("replacement IAM root must route webhooks"))?;
        assert_eq!(replacement_webhook.0.id(), environment_a.environment.id);

        let unauthorized_context = execution(&organization, &format!("other:{suffix}"), None)?;
        assert!(matches!(
            store
                .replay_iam_pairing(
                    &unauthorized_context,
                    environment_a.environment.id,
                    &pairing_mutation,
                )
                .await,
            Err(AppError::Forbidden)
        ));

        let activity_before_lookup = stored_last_activity(
            &production,
            &control_context,
            environment_a.environment.id,
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(2)).await;
        let access_a = store
            .resolve_root_key(&SecretString::from(environment_a.key.clone()))
            .await?;
        assert_eq!(
            stored_last_activity(
                &production,
                &control_context,
                environment_a.environment.id,
            )
            .await?,
            activity_before_lookup,
            "matching a root selector alone must not count as authenticated activity"
        );
        store.touch(&access_a).await?;
        assert!(
            stored_last_activity(
                &production,
                &control_context,
                environment_a.environment.id,
            )
            .await?
                > activity_before_lookup,
            "accepted use records activity explicitly"
        );
        assert_eq!(access_a.environment_id, environment_a.environment.id);
        assert_eq!(access_a.owner_org_id, organization);
        assert_eq!(
            access_a.iam_environment_key.expose_secret(),
            replacement_pairing.iam_environment_key.expose_secret()
        );
        assert_eq!(
            access_a.iam_app_secret.expose_secret(),
            replacement_pairing.iam_app_secret.expose_secret()
        );

        let create_b = create_input(&organization, format!("environment-b-{suffix}"));
        let environment_b = store
            .create(
                &control_context,
                &create_b,
                &mutation(format!("create-b-{suffix}"), b"create-b")?,
            )
            .await?;
        created_environment_ids.push(environment_b.environment.id);
        let duplicate_iam_environment = TestingEnvironmentIamPairing {
            iam_environment_id: replacement_pairing.iam_environment_id,
            iam_environment_key: SecretString::from(Uuid::new_v4().simple().to_string()),
            iam_app_id: create_b.iam_app_id.clone(),
            iam_app_secret: SecretString::from(REPLACEMENT_IAM_APPLICATION_SECRET),
        };
        let Err(duplicate_pairing) = store
            .replace_iam_pairing(
                &control_context,
                environment_b.environment.id,
                &duplicate_iam_environment,
                &mutation(format!("pair-duplicate-{suffix}"), b"pair-duplicate")?,
            )
            .await
        else {
            panic!("one IAM testing environment cannot back two Briefcase environments");
        };
        assert_conflict(duplicate_pairing, "testing_environment_already_exists");

        let context_a = execution(
            &organization,
            &actor_id,
            Some(TestingEnvironmentContext::new(
                environment_a.environment.id,
                paired_a.version,
            )),
        )?;
        let context_b = execution(
            &organization,
            &actor_id,
            Some(TestingEnvironmentContext::new(
                environment_b.environment.id,
                environment_b.environment.version,
            )),
        )?;
        reconcile_roots(&repository, &context_a).await?;
        reconcile_roots(&repository, &context_b).await?;
        assert_test_storage_limit(&data, &context_a).await?;
        assert_test_storage_limit(&data, &context_b).await?;
        assert_namespaced_and_isolated(
            &data,
            &organization,
            environment_a.environment.id,
            environment_b.environment.id,
        )
        .await?;
        assert_namespaced_and_isolated(
            &data,
            &organization,
            environment_b.environment.id,
            environment_a.environment.id,
        )
        .await?;

        let (prefix, object_key, multipart_key, provider_upload_id) =
            seed_provider_state(&data, &context_a, "clean-first").await?;
        let (second_prefix, second_object_key, second_multipart_key, second_provider_upload_id) =
            seed_provider_state(&data, &context_a, "worker-first").await?;
        mark_multipart_aborted(&data, &context_a, &provider_upload_id).await?;
        mark_multipart_aborted(&data, &context_a, &second_provider_upload_id).await?;
        let (clean_first_claim_id, clean_first_lease_token) =
            seed_claimed_version_cleanup(&data, &context_a, &object_key).await?;
        let (worker_first_claim_id, worker_first_lease_token) =
            seed_claimed_version_cleanup(&data, &context_a, &second_object_key).await?;
        assert_eq!(
            cancel_claim(
                &data,
                &context_a,
                worker_first_claim_id,
                worker_first_lease_token,
            )
            .await?,
            1,
            "when cancellation wins first, clean must recreate the descriptor"
        );
        assert!(
            cleanup_claim_state(&data, &context_a, worker_first_claim_id)
                .await?
                .is_none()
        );
        let expected_erased_rows = cleanable_row_count(&data, &context_a).await?;

        let rotate_mutation = mutation(format!("rotate-a-{suffix}"), b"rotate-a")?;
        let rotated = store
            .rotate_key(
                &control_context,
                environment_a.environment.id,
                &rotate_mutation,
            )
            .await?;
        let rotated_replay = store
            .rotate_key(
                &control_context,
                environment_a.environment.id,
                &rotate_mutation,
            )
            .await?;
        assert_same_secret_response(&rotated, &rotated_replay);
        assert_ne!(rotated.key, environment_a.key);
        assert_eq!(rotated.environment.key_generation, 2);
        assert!(matches!(
            store
                .resolve_root_key(&SecretString::from(environment_a.key.clone()))
                .await,
            Err(AppError::Unauthenticated)
        ));
        let rotated_access = store
            .resolve_root_key(&SecretString::from(rotated.key.clone()))
            .await?;

        let clean_mutation = mutation(format!("clean-a-{suffix}"), b"clean-a")?;
        let stale_context = execution(
            &organization,
            &actor_id,
            Some(TestingEnvironmentContext::new(
                rotated_access.environment_id,
                rotated_access.control_version,
            )),
        )?;
        let first_clean_store = store.clone();
        let first_clean_access = rotated_access.clone();
        let first_clean_mutation = clean_mutation.clone();
        let fence_key = testing_environment_fence_key(rotated_access.environment_id);
        let mut lifecycle_blocker = data.acquire().await?;
        lifecycle_blocker.close_on_drop();
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(fence_key)
            .execute(&mut *lifecycle_blocker)
            .await?;
        let mut clean_task = tokio::spawn(async move {
            first_clean_store
                .clean(
                    &first_clean_access,
                    "clean-a-live-test",
                    &first_clean_mutation,
                )
                .await
        });

        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut clean_task)
                .await
                .is_err(),
            "clean must wait behind the environment lifecycle fence"
        );
        expire_clean_idempotency_lease(
            &production,
            &control_context,
            environment_a.environment.id,
        )
        .await?;

        // Simulate an identical retry after the ordinary five-minute lease was
        // stolen. It may claim the row, but must wait for the first cleaner and
        // then replay instead of repeating provider calls.
        let retry_clean_store = store.clone();
        let retry_clean_access = rotated_access.clone();
        let retry_clean_mutation = clean_mutation.clone();
        let mut replay_task = tokio::spawn(async move {
            retry_clean_store
                .clean(
                    &retry_clean_access,
                    "clean-a-expired-lease-retry",
                    &retry_clean_mutation,
                )
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut replay_task)
                .await
                .is_err(),
            "an expired-lease retry must wait behind the environment fence"
        );

        // A data-plane request authenticated before clean also waits on the
        // fence. Once released, its stale generation is rejected before it can
        // read, reconcile roots, or publish any state.
        let stale_repository = repository.clone();
        let mut stale_request = tokio::spawn(async move {
            stale_repository
                .list_active_children(
                    &stale_context,
                    &ListEntriesQuery {
                        parent_id: None,
                        filter: None,
                        page: PageRequest::new(None, 100)
                            .unwrap_or_else(|error| panic!("valid page: {error}")),
                    },
                )
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut stale_request)
                .await
                .is_err(),
            "an in-flight stale request must remain fenced until clean finishes"
        );

        let fence_released = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
            .bind(fence_key)
            .fetch_one(&mut *lifecycle_blocker)
            .await?;
        assert!(fence_released);
        drop(lifecycle_blocker);
        let cleaned = clean_task.await??;
        let clean_replay = replay_task.await??;
        assert!(matches!(
            stale_request.await?,
            Err(MetadataRepositoryError::Conflict)
        ));
        assert_eq!(cleaned.environment_id, clean_replay.environment_id);
        assert_eq!(cleaned.erased_rows, clean_replay.erased_rows);
        assert_eq!(cleaned.cleaned_at, clean_replay.cleaned_at);
        assert_eq!(cleaned.erased_rows, u64::try_from(expected_erased_rows)?);
        assert_eq!(
            cancel_claim(
                &data,
                &context_a,
                clean_first_claim_id,
                clean_first_lease_token,
            )
            .await?,
            0,
            "clean must invalidate a preflighted worker's stale cancellation lease"
        );
        assert_eq!(
            cleanup_claim_state(&data, &context_a, clean_first_claim_id).await?,
            Some(("pending".to_owned(), None)),
            "the sole provider descriptor must remain durably queued"
        );
        assert!(
            object_store.calls().is_empty(),
            "clean must commit its logical reset before any provider work"
        );
        let refreshed_access = store
            .resolve_root_key(&SecretString::from(rotated.key.clone()))
            .await?;
        assert_eq!(
            refreshed_access.control_version,
            rotated_access.control_version + 1
        );
        let refreshed_context_a = execution(
            &organization,
            &actor_id,
            Some(TestingEnvironmentContext::new(
                refreshed_access.environment_id,
                refreshed_access.control_version,
            )),
        )?;
        let mut expected_cleanup = vec![
            QueuedCleanupRow {
                cleanup_kind: "multipart_abort".to_owned(),
                storage_prefix: prefix.clone(),
                object_key: multipart_key,
                object_version_id: None,
                provider_upload_id: Some(provider_upload_id),
            },
            QueuedCleanupRow {
                cleanup_kind: "multipart_abort".to_owned(),
                storage_prefix: second_prefix.clone(),
                object_key: second_multipart_key,
                object_version_id: None,
                provider_upload_id: Some(second_provider_upload_id),
            },
            QueuedCleanupRow {
                cleanup_kind: "version_delete".to_owned(),
                storage_prefix: prefix,
                object_key,
                object_version_id: Some("object-version-1".to_owned()),
                provider_upload_id: None,
            },
            QueuedCleanupRow {
                cleanup_kind: "version_delete".to_owned(),
                storage_prefix: second_prefix,
                object_key: second_object_key,
                object_version_id: Some("object-version-1".to_owned()),
                provider_upload_id: None,
            },
        ];
        expected_cleanup.sort_by(|left, right| {
            left.cleanup_kind
                .cmp(&right.cleanup_kind)
                .then_with(|| left.object_key.cmp(&right.object_key))
        });
        assert_eq!(
            queued_cleanup_rows(&data, &refreshed_context_a).await?,
            expected_cleanup,
            "clean must preserve/recreate every provider descriptor across either worker race order"
        );
        let count_a = organization_count(&data, &refreshed_context_a).await?;
        let count_b = organization_count(&data, &context_b).await?;
        assert_eq!(
            count_a, 1,
            "clean retains the selected sandbox's IAM identity projection"
        );
        assert_eq!(count_b, 1, "clean leaves the sibling sandbox intact");
        reconcile_roots(&repository, &refreshed_context_a).await?;
        assert_eq!(
            entry_count(&data, &refreshed_context_a).await?,
            3,
            "the next authenticated request rebuilds both containers and the retained member root"
        );
        assert_eq!(
            testing_storage_state(&data, &refreshed_context_a).await?,
            (0, 2_147_483_648),
            "clean resets consumption and retains the exact 2 GiB ceiling"
        );

        let delete_mutation = mutation(format!("delete-a-{suffix}"), b"delete-a")?;
        let deleted = store
            .delete(
                &control_context,
                environment_a.environment.id,
                &delete_mutation,
            )
            .await?;
        let deleted_replay = store
            .delete(
                &control_context,
                environment_a.environment.id,
                &delete_mutation,
            )
            .await?;
        assert_eq!(deleted.version, deleted_replay.version);
        assert_eq!(deleted.deleted_at, deleted_replay.deleted_at);
        assert_eq!(deleted.status, TestingEnvironmentStatus::Deleted);
        assert!(matches!(
            store
                .resolve_root_key(&SecretString::from(rotated.key.clone()))
                .await,
            Err(AppError::Unauthenticated)
        ));

        let restore_mutation = mutation(format!("restore-a-{suffix}"), b"restore-a")?;
        let restored = store
            .restore(
                &control_context,
                environment_a.environment.id,
                &restore_mutation,
            )
            .await?;
        let restored_replay = store
            .restore(
                &control_context,
                environment_a.environment.id,
                &restore_mutation,
            )
            .await?;
        assert_same_secret_response(&restored, &restored_replay);
        assert_ne!(restored.key, rotated.key);
        assert_eq!(restored.environment.key_generation, 3);
        assert!(matches!(
            store
                .resolve_root_key(&SecretString::from(rotated.key.clone()))
                .await,
            Err(AppError::Unauthenticated)
        ));
        let current_key = store
            .key(&control_context, environment_a.environment.id)
            .await?;
        assert_eq!(current_key.key, restored.key);
        assert_eq!(current_key.key_generation, 3);

        // Maintenance uses the same fence as broker and data-plane work. An
        // idle retirement cannot linearize until an accepted generation has
        // finished, and physical purge first enters a non-restorable state.
        let create_c = create_input(&organization, format!("environment-c-{suffix}"));
        let environment_c = store
            .create(
                &control_context,
                &create_c,
                &mutation(format!("create-c-{suffix}"), b"create-c")?,
            )
            .await?;
        created_environment_ids.push(environment_c.environment.id);
        let context_c = execution(
            &organization,
            &actor_id,
            Some(TestingEnvironmentContext::new(
                environment_c.environment.id,
                environment_c.environment.version,
            )),
        )?;
        reconcile_roots(&repository, &context_c).await?;
        seed_provider_state(&data, &context_c, "purge").await?;
        let access_c = store
            .resolve_root_key(&SecretString::from(environment_c.key.clone()))
            .await?;
        sqlx::query(
            "UPDATE briefcase.testing_environments \
                SET last_activity_at = clock_timestamp() - INTERVAL '31 days' \
              WHERE environment_id = $1",
        )
        .bind(environment_c.environment.id)
        .execute(&production)
        .await?;

        let use_fence_c = store.acquire_use_fence(&access_c).await?;
        let retire_production = production.clone();
        let retire_data = data.clone();
        let retire_objects = Arc::clone(&object_store);
        let mut retire_task = tokio::spawn(async move {
            maintain_testing_environments(
                &retire_production,
                &retire_data,
                retire_objects.as_ref(),
            )
            .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut retire_task)
                .await
                .is_err(),
            "idle retirement must wait for a shared environment-use fence"
        );
        use_fence_c.release().await?;
        let (retired_count, _) = retire_task.await??;
        assert!(retired_count >= 1);
        let retired_status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM briefcase.testing_environments WHERE environment_id = $1",
        )
        .bind(environment_c.environment.id)
        .fetch_one(&production)
        .await?;
        assert_eq!(retired_status, "deleted");

        sqlx::query(
            "UPDATE briefcase.testing_environments \
                SET deleted_at = clock_timestamp() - INTERVAL '31 days', \
                    purge_after = clock_timestamp() - INTERVAL '1 day' \
              WHERE environment_id = $1 AND status = 'deleted'",
        )
        .bind(environment_c.environment.id)
        .execute(&production)
        .await?;
        let purge_production = production.clone();
        let purge_data = data.clone();
        let purge_objects = Arc::clone(&object_store);
        let purge_task = tokio::spawn(async move {
            maintain_testing_environments(&purge_production, &purge_data, purge_objects.as_ref())
                .await
        });

        // The provider pause occurs after the control row is atomically marked
        // purging while its exclusive fence is still held.
        delete_barrier.wait().await;
        let purging_status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM briefcase.testing_environments WHERE environment_id = $1",
        )
        .bind(environment_c.environment.id)
        .fetch_one(&production)
        .await?;
        assert_eq!(purging_status, "purging");
        let public_environments = store.list(&control_context, None).await?;
        assert!(
            public_environments
                .items
                .iter()
                .all(|environment| environment.id != environment_c.environment.id),
            "the internal purging claim must never enter the public list"
        );
        assert!(matches!(
            store
                .get(&control_context, environment_c.environment.id)
                .await,
            Err(AppError::NotFound)
        ));
        let restore_c_store = store.clone();
        let restore_c_context = control_context.clone();
        let restore_c_mutation = mutation(format!("restore-c-{suffix}"), b"restore-c")?;
        let purge_target_id = environment_c.environment.id;
        let mut restore_c_task = tokio::spawn(async move {
            restore_c_store
                .restore(
                    &restore_c_context,
                    purge_target_id,
                    &restore_c_mutation,
                )
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut restore_c_task)
                .await
                .is_err(),
            "restore must wait behind an in-progress physical purge"
        );
        delete_barrier.wait().await;
        let (_, purged_count) = purge_task.await??;
        assert!(purged_count >= 1);
        assert!(restore_c_task.await?.is_err());
        let control_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM briefcase.testing_environments \
              WHERE environment_id = $1)",
        )
        .bind(environment_c.environment.id)
        .fetch_one(&production)
        .await?;
        assert!(!control_exists);
        assert_eq!(organization_count(&data, &context_c).await?, 0);

        // The global-ten-environment check is safe only when this disposable
        // database had no pre-existing active environments.
        if active_before == 0 {
            for index in 0..8 {
                let filler = store
                    .create(
                        &control_context,
                        &create_input(&organization, format!("filler-{index}-{suffix}")),
                        &mutation(
                            format!("create-filler-{index}-{suffix}"),
                            format!("filler-{index}").as_bytes(),
                        )?,
                    )
                    .await?;
                created_environment_ids.push(filler.environment.id);
            }
            let active = sqlx::query_scalar::<_, i64>(
                "SELECT briefcase.active_testing_environment_count()",
            )
            .fetch_one(&production)
            .await?;
            assert_eq!(active, ROOT_LIMIT);
            let Err(overflow) = store
                .create(
                    &control_context,
                    &create_input(&organization, format!("overflow-{suffix}")),
                    &mutation(format!("overflow-{suffix}"), b"overflow")?,
                )
                .await
            else {
                panic!("the eleventh active environment must be rejected");
            };
            assert_conflict(overflow, "testing_environment_limit_reached");
        } else {
            eprintln!(
                "skipping max-10 subcheck: disposable control database began with {active_before} active environments"
            );
        }

        // A final purge may not discard durable provider jobs. It claims the
        // non-restorable state, then waits for the worker to drain the queue.
        assert!(!queued_cleanup_rows(&data, &refreshed_context_a)
            .await?
            .is_empty());
        store
            .delete(
                &control_context,
                environment_a.environment.id,
                &mutation(format!("delete-a-pending-cleanup-{suffix}"), b"delete-a-again")?,
            )
            .await?;
        sqlx::query(
            "UPDATE briefcase.testing_environments \
                SET deleted_at = clock_timestamp() - INTERVAL '31 days', \
                    purge_after = clock_timestamp() - INTERVAL '1 day' \
              WHERE environment_id = $1 AND status = 'deleted'",
        )
        .bind(environment_a.environment.id)
        .execute(&production)
        .await?;
        let provider_calls_before_wait = object_store.calls();
        let (_, pending_purged) =
            maintain_testing_environments(&production, &data, object_store.as_ref()).await?;
        assert_eq!(pending_purged, 0);
        assert_eq!(object_store.calls(), provider_calls_before_wait);
        let pending_status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM briefcase.testing_environments WHERE environment_id = $1",
        )
        .bind(environment_a.environment.id)
        .fetch_one(&production)
        .await?;
        assert_eq!(pending_status, "purging");

        Ok(())
    })
    .catch_unwind()
    .await;

    let cleanup = hard_cleanup(
        &production,
        &data,
        &organization,
        &actor_id,
        &created_environment_ids,
    )
    .await;
    match result {
        Ok(result) => {
            cleanup?;
            result
        }
        Err(panic) => {
            if let Err(error) = cleanup {
                eprintln!("testing-environment cleanup also failed: {error:#}");
            }
            resume_unwind(panic)
        }
    }
}

fn assert_same_secret_response(
    left: &TestingEnvironmentWithKey,
    right: &TestingEnvironmentWithKey,
) {
    assert_eq!(left.environment.id, right.environment.id);
    assert_eq!(left.environment.version, right.environment.version);
    assert_eq!(
        left.environment.key_generation,
        right.environment.key_generation
    );
    assert_eq!(left.key, right.key);
}

async fn organization_count(pool: &PgPool, context: &ExecutionContext) -> anyhow::Result<i64> {
    let tenant = TenantContext::from_execution(context);
    let mut transaction: Transaction<'_, Postgres> =
        begin_tenant_transaction(pool, &tenant).await?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM briefcase.organizations \
          WHERE org_id = briefcase.current_org_id()",
    )
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(count)
}

async fn entry_count(pool: &PgPool, context: &ExecutionContext) -> anyhow::Result<i64> {
    let tenant = TenantContext::from_execution(context);
    let mut transaction = begin_tenant_transaction(pool, &tenant).await?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM briefcase.entries WHERE org_id = briefcase.current_org_id()",
    )
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(count)
}

async fn testing_storage_state(
    pool: &PgPool,
    context: &ExecutionContext,
) -> anyhow::Result<(i64, i64)> {
    let tenant = TenantContext::from_execution(context);
    let mut transaction = begin_tenant_transaction(pool, &tenant).await?;
    let state = sqlx::query_as::<_, (i64, i64)>(
        "SELECT stored_bytes, storage_limit_bytes \
           FROM briefcase.organization_usage \
          WHERE org_id = briefcase.current_org_id()",
    )
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(state)
}
