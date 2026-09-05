//! Encrypted Briefcase testing-environment control-plane persistence.

use std::{fmt, sync::Arc};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead as _, KeyInit as _, Payload},
};
use base64::{Engine as _, engine::general_purpose};
use hmac::{Hmac, Mac as _};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use sqlx::{PgPool, Postgres, Transaction, pool::PoolConnection};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    application::{
        context::{ExecutionContext, TestingEnvironmentContext},
        ports::{ObjectKey, ObjectStore, ObjectStoreError, StorageTarget},
        service::MutationMetadata,
        testing::{
            MAX_ACTIVE_TESTING_ENVIRONMENTS, TestingEnvironment, TestingEnvironmentCleaning,
            TestingEnvironmentCreate, TestingEnvironmentCreator, TestingEnvironmentIamPairing,
            TestingEnvironmentKey, TestingEnvironmentPage, TestingEnvironmentPatch,
            TestingEnvironmentSelf, TestingEnvironmentStatus, TestingEnvironmentWithKey,
        },
    },
    domain::{
        actor::{ActorKind, is_canonical_iam_application_id},
        storage::EncryptionMode,
    },
    error::AppError,
    infrastructure::postgres::{
        TenantContext, acquire_testing_environment_exclusive_lock,
        acquire_testing_environment_shared_session_lock, begin_tenant_transaction,
        begin_testing_environment_cleanup_transaction, release_testing_environment_exclusive_lock,
        release_testing_environment_shared_session_lock,
    },
};

type HmacSha256 = Hmac<Sha256>;

const CREATE_OPERATION: &str = "testing_environment.create";
const REPLACE_IAM_PAIRING_OPERATION: &str = "testing_environment.replace_iam_pairing";
const UPDATE_OPERATION: &str = "testing_environment.update";
const ROTATE_KEY_OPERATION: &str = "testing_environment.rotate_key";
const DELETE_OPERATION: &str = "testing_environment.delete";
const RESTORE_OPERATION: &str = "testing_environment.restore";
const CLEAN_OPERATION: &str = "testing_environment.clean";

macro_rules! environment_columns {
    () => {
        "org_id, environment_id, name, description, created_by_type, created_by_id, \
         iam_environment_id, iam_app_id, key_generation, key_rotated_at, status, \
         last_activity_at, cleaned_at, deleted_at, purge_after, version, created_at, updated_at"
    };
}

#[derive(Clone)]
struct MasterKey([u8; 32]);

impl fmt::Debug for MasterKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MasterKey(<redacted>)")
    }
}

/// Secrets and public routing facts loaded after a root-key match.
#[derive(Clone)]
pub struct TestingEnvironmentAccess {
    /// Public Briefcase testing-environment UUID.
    pub environment_id: Uuid,
    /// Production organization that owns the environment.
    pub owner_org_id: String,
    /// Display name used by the key-authorized self route.
    pub name: String,
    /// Optional purpose description.
    pub description: Option<String>,
    /// Current root-key generation.
    pub key_generation: i64,
    /// Control-plane version authenticated with the root key.
    pub control_version: i64,
    /// Environment creation time.
    pub created_at: OffsetDateTime,
    /// Public IAM testing-environment UUID.
    pub iam_environment_id: Uuid,
    /// Canonical Briefcase Application ID inside the IAM environment.
    pub iam_app_id: String,
    /// IAM environment root key, forwarded on every IAM call.
    pub iam_environment_key: SecretString,
    /// Test-only IAM Application secret.
    pub iam_app_secret: SecretString,
}

impl fmt::Debug for TestingEnvironmentAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TestingEnvironmentAccess")
            .field("environment_id", &self.environment_id)
            .field("owner_org_id", &self.owner_org_id)
            .field("name", &self.name)
            .field("description", &self.description)
            .field("key_generation", &self.key_generation)
            .field("control_version", &self.control_version)
            .field("created_at", &self.created_at)
            .field("iam_environment_id", &self.iam_environment_id)
            .field("iam_app_id", &self.iam_app_id)
            .field("iam_environment_key", &"<redacted>")
            .field("iam_app_secret", &"<redacted>")
            .finish()
    }
}

/// Cloneable environment store spanning production control and shared test data.
#[derive(Clone)]
pub struct TestingEnvironmentStore {
    production: PgPool,
    test: PgPool,
    master: Arc<MasterKey>,
}

impl fmt::Debug for TestingEnvironmentStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TestingEnvironmentStore")
            .field("production", &self.production)
            .field("test", &self.test)
            .field("master", &self.master)
            .finish()
    }
}

#[derive(sqlx::FromRow)]
struct EnvironmentRow {
    org_id: String,
    environment_id: Uuid,
    name: String,
    description: Option<String>,
    created_by_type: String,
    created_by_id: String,
    iam_environment_id: Uuid,
    iam_app_id: String,
    key_generation: i64,
    key_rotated_at: Option<OffsetDateTime>,
    status: String,
    last_activity_at: OffsetDateTime,
    cleaned_at: Option<OffsetDateTime>,
    deleted_at: Option<OffsetDateTime>,
    purge_after: Option<OffsetDateTime>,
    version: i64,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct RootLookupRow {
    org_id: String,
    environment_id: Uuid,
    name: String,
    description: Option<String>,
    key_generation: i64,
    control_version: i64,
    created_at: OffsetDateTime,
    iam_environment_id: Uuid,
    iam_app_id: String,
    iam_environment_key_ciphertext: Vec<u8>,
    iam_environment_key_nonce: Vec<u8>,
    iam_app_secret_ciphertext: Vec<u8>,
    iam_app_secret_nonce: Vec<u8>,
}

#[derive(sqlx::FromRow)]
struct IamCandidateRow {
    org_id: String,
    environment_id: Uuid,
    control_version: i64,
    iam_environment_key_ciphertext: Vec<u8>,
    iam_environment_key_nonce: Vec<u8>,
}

#[derive(sqlx::FromRow)]
struct EnvironmentIdempotencyRow {
    request_hash: Vec<u8>,
    environment_id: Uuid,
    status: String,
    response_ciphertext: Option<Vec<u8>>,
    response_nonce: Option<Vec<u8>>,
    locked_until: OffsetDateTime,
}

struct PreparedEnvironmentCreate {
    environment_id: Uuid,
    root_key: String,
    root_digest: [u8; 32],
    iam_digest: [u8; 32],
    root_ciphertext: Vec<u8>,
    root_nonce: [u8; 12],
    iam_ciphertext: Vec<u8>,
    iam_nonce: [u8; 12],
    app_ciphertext: Vec<u8>,
    app_nonce: [u8; 12],
}

struct PreparedIamPairing {
    iam_digest: [u8; 32],
    iam_ciphertext: Vec<u8>,
    iam_nonce: [u8; 12],
    app_ciphertext: Vec<u8>,
    app_nonce: [u8; 12],
}

enum EnvironmentMutationClaim<T> {
    Acquired(Uuid),
    Replay(T),
}

enum FencedCleanClaim {
    Proceed {
        fence: TestingEnvironmentExclusiveFence,
        control_version: i64,
    },
    Replay(TestingEnvironmentCleaning),
}

#[derive(Clone, Copy)]
struct EnvironmentMutationIdentity<'a> {
    authority_type: &'static str,
    authority_id: &'a str,
    origin_app_id: Option<&'a str>,
    operation: &'static str,
    metadata: &'a MutationMetadata,
}

#[derive(sqlx::FromRow)]
struct StoredVersionObject {
    bucket_name: String,
    storage_region: String,
    storage_prefix: String,
    storage_role_arn: Option<String>,
    storage_encryption_mode: String,
    storage_kms_key_arn: Option<String>,
    object_key: String,
    object_version_id: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PendingMultipartObject {
    bucket_name: String,
    storage_region: String,
    storage_prefix: String,
    storage_role_arn: Option<String>,
    storage_encryption_mode: String,
    storage_kms_key_arn: Option<String>,
    object_key: String,
    provider_upload_id: String,
}

/// Dedicated session holding the exclusive side of one environment's
/// lifecycle fence. The connection is always closed on drop so task
/// cancellation cannot leak a PostgreSQL advisory lock back into the pool.
struct TestingEnvironmentExclusiveFence {
    connection: PoolConnection<Postgres>,
    environment_id: Uuid,
}

/// Shared lifecycle fence held across a credential-sensitive request.
///
/// The backing connection is marked `close_on_drop`, so cancellation closes
/// the PostgreSQL session and cannot leak an advisory lock into the pool.
pub struct TestingEnvironmentUseFence {
    connection: PoolConnection<Postgres>,
    environment_id: Uuid,
}

impl fmt::Debug for TestingEnvironmentUseFence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TestingEnvironmentUseFence")
            .field("environment_id", &self.environment_id)
            .finish_non_exhaustive()
    }
}

impl TestingEnvironmentUseFence {
    async fn acquire(test: &PgPool, environment_id: Uuid) -> Result<Self, AppError> {
        let mut connection = test.acquire().await?;
        connection.close_on_drop();
        acquire_testing_environment_shared_session_lock(&mut connection, environment_id).await?;
        Ok(Self {
            connection,
            environment_id,
        })
    }

    /// Releases the held lifecycle fence before the guard leaves scope.
    ///
    /// # Errors
    ///
    /// Returns a database error if the PostgreSQL session no longer owns the
    /// shared advisory lock.
    pub async fn release(mut self) -> Result<(), AppError> {
        release_testing_environment_shared_session_lock(&mut self.connection, self.environment_id)
            .await?;
        Ok(())
    }
}

impl TestingEnvironmentExclusiveFence {
    async fn acquire(test: &PgPool, environment_id: Uuid) -> Result<Self, AppError> {
        let mut connection = test.acquire().await?;
        connection.close_on_drop();
        acquire_testing_environment_exclusive_lock(&mut connection, environment_id).await?;
        Ok(Self {
            connection,
            environment_id,
        })
    }

    async fn release(mut self) -> Result<(), AppError> {
        release_testing_environment_exclusive_lock(&mut self.connection, self.environment_id)
            .await?;
        Ok(())
    }

    async fn clean(
        &mut self,
        owner_org_id: &str,
        control_version: i64,
        request_id: &str,
    ) -> Result<u64, AppError> {
        let selected = TestingEnvironmentContext::new(self.environment_id, control_version);
        let tenant =
            TenantContext::for_testing_environment_service(owner_org_id, selected, request_id);
        let mut transaction =
            begin_testing_environment_cleanup_transaction(&mut self.connection, &tenant).await?;
        // A worker may already have preflighted an otherwise cancellable job.
        // Invalidate that lease in this transaction before source metadata is
        // erased, so its stale cancellation cannot remove the sole descriptor.
        sqlx::query_scalar::<_, i64>(
            "SELECT briefcase.prepare_current_testing_environment_clean()",
        )
        .fetch_one(&mut *transaction)
        .await?;
        let erased_rows =
            sqlx::query_scalar::<_, i64>("SELECT briefcase.erase_current_testing_environment()")
                .fetch_one(&mut *transaction)
                .await?;
        transaction.commit().await?;
        u64::try_from(erased_rows).map_err(|_| crypto_error())
    }

    async fn purge(
        &mut self,
        objects: &(impl ObjectStore + ?Sized),
        owner_org_id: &str,
        control_version: i64,
        request_id: &str,
    ) -> Result<u64, AppError> {
        let selected = TestingEnvironmentContext::new(self.environment_id, control_version);
        let tenant =
            TenantContext::for_testing_environment_service(owner_org_id, selected, request_id);
        let mut transaction =
            begin_testing_environment_cleanup_transaction(&mut self.connection, &tenant).await?;
        let versions = sqlx::query_as::<_, StoredVersionObject>(
            "SELECT version.bucket_name, version.storage_region, version.storage_prefix, \
                    configuration.role_arn AS storage_role_arn, \
                    version.storage_encryption_mode, version.storage_kms_key_arn, \
                    version.object_key, version.object_version_id \
               FROM briefcase.entry_versions AS version \
               LEFT JOIN briefcase.organization_storage_configs AS configuration \
                 ON configuration.org_id = version.org_id \
                AND configuration.storage_config_id = version.storage_config_id \
              WHERE version.org_id = briefcase.current_org_id()",
        )
        .fetch_all(&mut *transaction)
        .await?;
        let multiparts = sqlx::query_as::<_, PendingMultipartObject>(
            "SELECT upload.bucket_name, upload.storage_region, upload.storage_prefix, \
                    configuration.role_arn AS storage_role_arn, \
                    upload.storage_encryption_mode, upload.storage_kms_key_arn, \
                    upload.object_key, upload.provider_upload_id \
               FROM briefcase.multipart_uploads AS upload \
               LEFT JOIN briefcase.organization_storage_configs AS configuration \
                 ON configuration.org_id = upload.org_id \
                AND configuration.storage_config_id = upload.storage_config_id \
              WHERE upload.org_id = briefcase.current_org_id() \
                AND upload.status <> 'completed'",
        )
        .fetch_all(&mut *transaction)
        .await?;

        for upload in &multiparts {
            let target = storage_target(
                owner_org_id,
                &upload.bucket_name,
                &upload.storage_region,
                &upload.storage_prefix,
                upload.storage_role_arn.as_deref(),
                &upload.storage_encryption_mode,
                upload.storage_kms_key_arn.as_deref(),
            )?;
            let key = ObjectKey::new(upload.object_key.clone()).map_err(|_| crypto_error())?;
            accept_provider_cleanup(
                &objects
                    .abort_multipart(&target, &key, &upload.provider_upload_id)
                    .await,
            )?;
        }
        for version in &versions {
            let target = storage_target(
                owner_org_id,
                &version.bucket_name,
                &version.storage_region,
                &version.storage_prefix,
                version.storage_role_arn.as_deref(),
                &version.storage_encryption_mode,
                version.storage_kms_key_arn.as_deref(),
            )?;
            let key = ObjectKey::new(version.object_key.clone()).map_err(|_| crypto_error())?;
            accept_provider_cleanup(
                &objects
                    .delete(&target, &key, version.object_version_id.as_deref())
                    .await,
            )?;
        }

        let erased_rows =
            sqlx::query_scalar::<_, i64>("SELECT briefcase.purge_current_testing_environment()")
                .fetch_one(&mut *transaction)
                .await?;
        transaction.commit().await?;
        u64::try_from(erased_rows).map_err(|_| crypto_error())
    }

    async fn has_provider_cleanup(
        &mut self,
        owner_org_id: &str,
        control_version: i64,
        request_id: &str,
    ) -> Result<bool, AppError> {
        let selected = TestingEnvironmentContext::new(self.environment_id, control_version);
        let tenant =
            TenantContext::for_testing_environment_service(owner_org_id, selected, request_id);
        let mut transaction =
            begin_testing_environment_cleanup_transaction(&mut self.connection, &tenant).await?;
        let pending = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM briefcase.object_cleanup_jobs \
              WHERE org_id = briefcase.current_org_id())",
        )
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(pending)
    }
}

impl TestingEnvironmentStore {
    /// Builds a store from a base64-encoded 256-bit encryption key.
    ///
    /// # Errors
    ///
    /// Returns a redacted error if the master key is not exactly 32 bytes.
    pub fn new(
        production: PgPool,
        test: PgPool,
        encoded_master_key: &SecretString,
    ) -> Result<Self, AppError> {
        let bytes = general_purpose::STANDARD
            .decode(encoded_master_key.expose_secret())
            .or_else(|_| {
                general_purpose::URL_SAFE_NO_PAD.decode(encoded_master_key.expose_secret())
            })
            .map_err(|_| AppError::Internal {
                category: "testing_environment_master_key",
            })?;
        let master: [u8; 32] = bytes.try_into().map_err(|_| AppError::Internal {
            category: "testing_environment_master_key",
        })?;
        Ok(Self {
            production,
            test,
            master: Arc::new(MasterKey(master)),
        })
    }

    /// Production pool holding lifecycle records.
    #[must_use]
    pub const fn production_pool(&self) -> &PgPool {
        &self.production
    }

    /// Shared sandbox pool holding filesystem state.
    #[must_use]
    pub const fn test_pool(&self) -> &PgPool {
        &self.test
    }

    /// Resolves a Briefcase root key without recording activity.
    ///
    /// # Errors
    ///
    /// Fails when the key is malformed, unknown, inactive, or its encrypted
    /// environment credentials cannot be loaded safely.
    pub async fn resolve_root_key(
        &self,
        root_key: &SecretString,
    ) -> Result<TestingEnvironmentAccess, AppError> {
        validate_root_key(root_key.expose_secret())?;
        let digest = self.digest(b"briefcase-root", root_key.expose_secret().as_bytes())?;
        let row = sqlx::query_as::<_, RootLookupRow>(
            "SELECT * FROM briefcase.testing_environment_by_root_digest($1)",
        )
        .bind(digest.as_slice())
        .fetch_optional(&self.production)
        .await?
        .ok_or(AppError::Unauthenticated)?;
        self.access_from_row(row)
    }

    /// Records successful use of an already-authenticated environment key.
    ///
    /// Root lookup itself is intentionally read-only: callers invoke this only
    /// after IAM actor authentication succeeds, or after a root-authorized
    /// lifecycle endpoint has accepted the key.
    ///
    /// # Errors
    ///
    /// Returns an authentication or database error if the environment stopped
    /// being active before its accepted use could be recorded.
    pub async fn touch(&self, access: &TestingEnvironmentAccess) -> Result<(), AppError> {
        let touched =
            sqlx::query_scalar::<_, bool>("SELECT briefcase.touch_testing_environment($1, $2)")
                .bind(access.environment_id)
                .bind(access.control_version)
                .fetch_one(&self.production)
                .await?;
        if !touched {
            return Err(AppError::Unauthenticated);
        }
        Ok(())
    }

    /// Holds the shared lifecycle fence for a credential-sensitive request and
    /// revalidates the exact generation after acquiring it.
    ///
    /// # Errors
    ///
    /// Returns an authentication error when a lifecycle mutation changed or
    /// retired the environment after its root key was resolved.
    pub async fn acquire_use_fence(
        &self,
        access: &TestingEnvironmentAccess,
    ) -> Result<TestingEnvironmentUseFence, AppError> {
        let fence = TestingEnvironmentUseFence::acquire(&self.test, access.environment_id).await?;
        let current = sqlx::query_scalar::<_, bool>(
            "SELECT briefcase.testing_environment_version_matches($1, $2)",
        )
        .bind(access.environment_id)
        .bind(access.control_version)
        .fetch_one(&self.production)
        .await?;
        if !current {
            fence.release().await?;
            return Err(AppError::Unauthenticated);
        }
        Ok(fence)
    }

    /// Finds the sandbox whose encrypted IAM key matches a verified test webhook.
    ///
    /// The supplied predicate receives one decrypted candidate at a time and
    /// should perform the verifier's constant-time comparison. No candidate is
    /// logged or persisted with the webhook event.
    ///
    /// # Errors
    ///
    /// Fails when candidate credentials cannot be loaded, decrypted, or
    /// atomically marked active.
    pub async fn resolve_iam_webhook<F>(
        &self,
        mut matches: F,
    ) -> Result<Option<(TestingEnvironmentContext, String)>, AppError>
    where
        F: FnMut(&SecretString) -> bool,
    {
        let candidates = sqlx::query_as::<_, IamCandidateRow>(
            "SELECT * FROM briefcase.active_testing_environment_iam_candidates()",
        )
        .fetch_all(&self.production)
        .await?;
        for candidate in candidates {
            let aad = secret_aad(candidate.environment_id, "iam-environment-key");
            let key = self.decrypt(
                &candidate.iam_environment_key_ciphertext,
                &candidate.iam_environment_key_nonce,
                &aad,
            )?;
            if matches(&key) {
                let control_version = sqlx::query_scalar::<_, Option<i64>>(
                    "SELECT briefcase.touch_testing_environment_generation($1, $2)",
                )
                .bind(candidate.environment_id)
                .bind(candidate.control_version)
                .fetch_one(&self.production)
                .await?;
                let Some(control_version) = control_version else {
                    return Ok(None);
                };
                return Ok(Some((
                    TestingEnvironmentContext::new(candidate.environment_id, control_version),
                    candidate.org_id,
                )));
            }
        }
        Ok(None)
    }

    /// Recovers a completed create before its external IAM credentials are
    /// contacted again.
    ///
    /// The lookup is scoped by the production tenant plus the exact actor,
    /// originating Application, operation, idempotency key, and request
    /// fingerprint. An in-progress or absent claim returns `None`; a key reused
    /// for a different body is rejected.
    ///
    /// # Errors
    ///
    /// Fails for a test-plane context, an idempotency binding mismatch, corrupt
    /// encrypted response state, or a database error.
    pub async fn replay_create(
        &self,
        execution: &ExecutionContext,
        mutation: &MutationMetadata,
    ) -> Result<Option<TestingEnvironmentWithKey>, AppError> {
        require_environment_admin(execution)?;
        let mut transaction = self.management_transaction(execution).await?;
        let authorization = execution.authorization();
        let actor = authorization.actor();
        let identity = EnvironmentMutationIdentity {
            authority_type: actor_type(actor.kind()),
            authority_id: actor.id().as_str(),
            origin_app_id: authorization
                .originating_application()
                .map(crate::domain::actor::ApplicationId::as_str),
            operation: CREATE_OPERATION,
            metadata: mutation,
        };
        let replay = self
            .completed_mutation(&mut transaction, &identity, None)
            .await?;
        transaction.commit().await?;
        Ok(replay)
    }

    /// Creates an empty Briefcase environment and returns its one root key.
    ///
    /// # Errors
    ///
    /// Fails for invalid input, authorization or persistence errors, duplicate
    /// IAM bindings, or when the deployment-wide active limit is reached.
    pub async fn create(
        &self,
        execution: &ExecutionContext,
        input: &TestingEnvironmentCreate,
        mutation: &MutationMetadata,
    ) -> Result<TestingEnvironmentWithKey, AppError> {
        validate_create(input)?;
        require_environment_admin(execution)?;
        let prepared = self.prepare_create(input)?;
        let environment_id = prepared.environment_id;

        let tenant = TenantContext::from_execution(execution);
        let mut transaction = begin_tenant_transaction(&self.production, &tenant).await?;
        let auth = execution.authorization();
        let actor = auth.actor();
        let actor_type = actor_type(actor.kind());
        let mutation_identity = EnvironmentMutationIdentity {
            authority_type: actor_type,
            authority_id: actor.id().as_str(),
            origin_app_id: auth
                .originating_application()
                .map(crate::domain::actor::ApplicationId::as_str),
            operation: CREATE_OPERATION,
            metadata: mutation,
        };
        match self
            .claim_mutation::<TestingEnvironmentWithKey>(
                &mut transaction,
                &mutation_identity,
                environment_id,
            )
            .await?
        {
            EnvironmentMutationClaim::Replay(response) => {
                transaction.commit().await?;
                return Ok(response);
            }
            EnvironmentMutationClaim::Acquired(claimed_id) if claimed_id != environment_id => {
                return Err(idempotency_error());
            }
            EnvironmentMutationClaim::Acquired(_) => {}
        }
        sqlx::query("SELECT pg_advisory_xact_lock(742864113)")
            .execute(&mut *transaction)
            .await?;
        let active =
            sqlx::query_scalar::<_, i64>("SELECT briefcase.active_testing_environment_count()")
                .fetch_one(&mut *transaction)
                .await?;
        if active >= MAX_ACTIVE_TESTING_ENVIRONMENTS {
            return Err(AppError::conflict("testing_environment_limit_reached"));
        }
        let row = sqlx::query_as::<_, EnvironmentRow>(concat!(
            "INSERT INTO briefcase.testing_environments (",
            "org_id, environment_id, name, description, created_by_type, created_by_id, ",
            "iam_environment_id, iam_app_id, iam_environment_key_digest, ",
            "iam_environment_key_ciphertext, iam_environment_key_nonce, ",
            "iam_app_secret_ciphertext, iam_app_secret_nonce, root_key_digest, ",
            "root_key_ciphertext, root_key_nonce) ",
            "VALUES (briefcase.current_org_id(), $1, $2, $3, $4, $5, $6, $7, $8, ",
            "$9, $10, $11, $12, $13, $14, $15) RETURNING ",
            environment_columns!()
        ))
        .bind(environment_id)
        .bind(input.name.trim())
        .bind(input.description.as_deref())
        .bind(actor_type)
        .bind(actor.id().as_str())
        .bind(input.iam_environment_id)
        .bind(&input.iam_app_id)
        .bind(prepared.iam_digest.as_slice())
        .bind(prepared.iam_ciphertext)
        .bind(prepared.iam_nonce.as_slice())
        .bind(prepared.app_ciphertext)
        .bind(prepared.app_nonce.as_slice())
        .bind(prepared.root_digest.as_slice())
        .bind(prepared.root_ciphertext)
        .bind(prepared.root_nonce.as_slice())
        .fetch_one(&mut *transaction)
        .await
        .map_err(map_environment_sql)?;
        let response = TestingEnvironmentWithKey {
            environment: environment(row)?,
            key: prepared.root_key,
        };
        self.complete_mutation(
            &mut transaction,
            &mutation_identity,
            environment_id,
            &response,
        )
        .await?;
        transaction.commit().await?;
        Ok(response)
    }

    fn prepare_create(
        &self,
        input: &TestingEnvironmentCreate,
    ) -> Result<PreparedEnvironmentCreate, AppError> {
        let environment_id = Uuid::now_v7();
        let root_key = Uuid::new_v4().simple().to_string();
        let root_digest = self.digest(b"briefcase-root", root_key.as_bytes())?;
        let iam_digest = self.digest(
            b"iam-environment",
            input.iam_environment_key.expose_secret().as_bytes(),
        )?;
        let (root_ciphertext, root_nonce) = self.encrypt(
            &SecretString::from(root_key.clone()),
            &secret_aad(environment_id, "briefcase-root"),
        )?;
        let (iam_ciphertext, iam_nonce) = self.encrypt(
            &input.iam_environment_key,
            &secret_aad(environment_id, "iam-environment-key"),
        )?;
        let (app_ciphertext, app_nonce) = self.encrypt(
            &input.iam_app_secret,
            &secret_aad(environment_id, "iam-app-secret"),
        )?;
        Ok(PreparedEnvironmentCreate {
            environment_id,
            root_key,
            root_digest,
            iam_digest,
            root_ciphertext,
            root_nonce,
            iam_ciphertext,
            iam_nonce,
            app_ciphertext,
            app_nonce,
        })
    }

    /// Lists environments owned by the caller's production organization.
    ///
    /// # Errors
    ///
    /// Fails when the tenant-scoped control query cannot be completed.
    pub async fn list(
        &self,
        execution: &ExecutionContext,
        status: Option<TestingEnvironmentStatus>,
    ) -> Result<TestingEnvironmentPage, AppError> {
        let mut transaction = self.management_transaction(execution).await?;
        let rows = sqlx::query_as::<_, EnvironmentRow>(concat!(
            "SELECT ",
            environment_columns!(),
            " FROM briefcase.testing_environments ",
            "WHERE org_id = briefcase.current_org_id() ",
            "AND status IN ('active', 'deleted') ",
            "AND ($1::text IS NULL OR status = $1) ",
            "ORDER BY created_at DESC, environment_id DESC"
        ))
        .bind(status.map(status_name))
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(TestingEnvironmentPage {
            items: rows
                .into_iter()
                .map(environment)
                .collect::<Result<_, _>>()?,
        })
    }

    /// Reads one environment in the caller's production organization.
    ///
    /// # Errors
    ///
    /// Fails when the environment is absent or its tenant-scoped row cannot be
    /// loaded.
    pub async fn get(
        &self,
        execution: &ExecutionContext,
        environment_id: Uuid,
    ) -> Result<TestingEnvironment, AppError> {
        let mut transaction = self.management_transaction(execution).await?;
        let row = load_environment(&mut transaction, environment_id).await?;
        transaction.commit().await?;
        environment(row)
    }

    /// Recovers a completed IAM re-pairing before the replacement credential
    /// is contacted again, while also enforcing creator/admin authority.
    ///
    /// # Errors
    ///
    /// Fails for insufficient authority, a missing environment, an idempotency
    /// binding mismatch, corrupt encrypted response state, or a database error.
    pub async fn replay_iam_pairing(
        &self,
        execution: &ExecutionContext,
        environment_id: Uuid,
        mutation: &MutationMetadata,
    ) -> Result<Option<TestingEnvironment>, AppError> {
        require_environment_admin(execution)?;
        let mut transaction = self.management_transaction(execution).await?;
        ensure_creator_or_admin(&mut transaction, execution, environment_id).await?;
        let authorization = execution.authorization();
        let actor = authorization.actor();
        let identity = EnvironmentMutationIdentity {
            authority_type: actor_type(actor.kind()),
            authority_id: actor.id().as_str(),
            origin_app_id: authorization
                .originating_application()
                .map(crate::domain::actor::ApplicationId::as_str),
            operation: REPLACE_IAM_PAIRING_OPERATION,
            metadata: mutation,
        };
        let replay = self
            .completed_mutation(&mut transaction, &identity, Some(environment_id))
            .await?;
        transaction.commit().await?;
        Ok(replay)
    }

    /// Atomically replaces every IAM test-plane credential while retaining the
    /// Briefcase root key and isolated data.
    ///
    /// # Errors
    ///
    /// Fails for invalid input, insufficient authority, a missing environment,
    /// duplicate IAM binding, idempotency conflict, or persistence error.
    pub async fn replace_iam_pairing(
        &self,
        execution: &ExecutionContext,
        environment_id: Uuid,
        pairing: &TestingEnvironmentIamPairing,
        mutation: &MutationMetadata,
    ) -> Result<TestingEnvironment, AppError> {
        validate_iam_pairing(pairing)?;
        require_environment_admin(execution)?;
        let prepared = self.prepare_iam_pairing(environment_id, pairing)?;
        let fence = TestingEnvironmentExclusiveFence::acquire(&self.test, environment_id).await?;
        let mut transaction = self.management_transaction(execution).await?;
        ensure_creator_or_admin(&mut transaction, execution, environment_id).await?;
        let authorization = execution.authorization();
        let actor = authorization.actor();
        let identity = EnvironmentMutationIdentity {
            authority_type: actor_type(actor.kind()),
            authority_id: actor.id().as_str(),
            origin_app_id: authorization
                .originating_application()
                .map(crate::domain::actor::ApplicationId::as_str),
            operation: REPLACE_IAM_PAIRING_OPERATION,
            metadata: mutation,
        };
        match self
            .claim_mutation::<TestingEnvironment>(&mut transaction, &identity, environment_id)
            .await?
        {
            EnvironmentMutationClaim::Replay(response) => {
                transaction.commit().await?;
                fence.release().await?;
                return Ok(response);
            }
            EnvironmentMutationClaim::Acquired(claimed_id) if claimed_id != environment_id => {
                return Err(idempotency_error());
            }
            EnvironmentMutationClaim::Acquired(_) => {}
        }
        let row = sqlx::query_as::<_, EnvironmentRow>(concat!(
            "UPDATE briefcase.testing_environments SET ",
            "iam_environment_id = $2, iam_app_id = $3, ",
            "iam_environment_key_digest = $4, iam_environment_key_ciphertext = $5, ",
            "iam_environment_key_nonce = $6, iam_app_secret_ciphertext = $7, ",
            "iam_app_secret_nonce = $8, version = version + 1 ",
            "WHERE org_id = briefcase.current_org_id() AND environment_id = $1 ",
            "AND status IN ('active', 'deleted') ",
            "RETURNING ",
            environment_columns!()
        ))
        .bind(environment_id)
        .bind(pairing.iam_environment_id)
        .bind(&pairing.iam_app_id)
        .bind(prepared.iam_digest.as_slice())
        .bind(prepared.iam_ciphertext)
        .bind(prepared.iam_nonce.as_slice())
        .bind(prepared.app_ciphertext)
        .bind(prepared.app_nonce.as_slice())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_environment_sql)?
        .ok_or(AppError::NotFound)?;
        let response = environment(row)?;
        self.complete_mutation(&mut transaction, &identity, environment_id, &response)
            .await?;
        transaction.commit().await?;
        fence.release().await?;
        Ok(response)
    }

    fn prepare_iam_pairing(
        &self,
        environment_id: Uuid,
        pairing: &TestingEnvironmentIamPairing,
    ) -> Result<PreparedIamPairing, AppError> {
        let iam_digest = self.digest(
            b"iam-environment",
            pairing.iam_environment_key.expose_secret().as_bytes(),
        )?;
        let (iam_ciphertext, iam_nonce) = self.encrypt(
            &pairing.iam_environment_key,
            &secret_aad(environment_id, "iam-environment-key"),
        )?;
        let (app_ciphertext, app_nonce) = self.encrypt(
            &pairing.iam_app_secret,
            &secret_aad(environment_id, "iam-app-secret"),
        )?;
        Ok(PreparedIamPairing {
            iam_digest,
            iam_ciphertext,
            iam_nonce,
            app_ciphertext,
            app_nonce,
        })
    }

    /// Updates the name or description as the creator/admin/owner.
    ///
    /// # Errors
    ///
    /// Fails for invalid input, insufficient authority, a missing environment,
    /// or a stale optimistic-concurrency version.
    pub async fn update(
        &self,
        execution: &ExecutionContext,
        environment_id: Uuid,
        expected_version: i64,
        patch: &TestingEnvironmentPatch,
        mutation: &MutationMetadata,
    ) -> Result<TestingEnvironment, AppError> {
        validate_patch(patch)?;
        require_environment_admin(execution)?;
        let fence = TestingEnvironmentExclusiveFence::acquire(&self.test, environment_id).await?;
        let mut transaction = self.management_transaction(execution).await?;
        ensure_creator_or_admin(&mut transaction, execution, environment_id).await?;
        let auth = execution.authorization();
        let actor = auth.actor();
        let authority_type = actor_type(actor.kind());
        let mutation_identity = EnvironmentMutationIdentity {
            authority_type,
            authority_id: actor.id().as_str(),
            origin_app_id: auth
                .originating_application()
                .map(crate::domain::actor::ApplicationId::as_str),
            operation: UPDATE_OPERATION,
            metadata: mutation,
        };
        match self
            .claim_mutation::<TestingEnvironment>(
                &mut transaction,
                &mutation_identity,
                environment_id,
            )
            .await?
        {
            EnvironmentMutationClaim::Replay(response) => {
                transaction.commit().await?;
                fence.release().await?;
                return Ok(response);
            }
            EnvironmentMutationClaim::Acquired(claimed_id) if claimed_id != environment_id => {
                return Err(idempotency_error());
            }
            EnvironmentMutationClaim::Acquired(_) => {}
        }
        let description_present = patch.description.is_some();
        let row = sqlx::query_as::<_, EnvironmentRow>(concat!(
            "UPDATE briefcase.testing_environments SET ",
            "name = COALESCE($2, name), ",
            "description = CASE WHEN $3 THEN $4 ELSE description END, version = version + 1 ",
            "WHERE org_id = briefcase.current_org_id() AND environment_id = $1 ",
            "AND status = 'active' AND version = $5 RETURNING ",
            environment_columns!()
        ))
        .bind(environment_id)
        .bind(patch.name.as_deref().map(str::trim))
        .bind(description_present)
        .bind(
            patch
                .description
                .as_ref()
                .and_then(|value| value.as_deref()),
        )
        .bind(expected_version)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_environment_sql)?
        .ok_or_else(|| AppError::conflict("testing_environment_version_conflict"))?;
        let response = environment(row)?;
        self.complete_mutation(
            &mut transaction,
            &mutation_identity,
            environment_id,
            &response,
        )
        .await?;
        transaction.commit().await?;
        fence.release().await?;
        Ok(response)
    }

    /// Retrieves the current root key as the creator/admin/owner.
    ///
    /// # Errors
    ///
    /// Fails for insufficient authority, a missing or deleted environment, or
    /// if its encrypted root key cannot be recovered.
    pub async fn key(
        &self,
        execution: &ExecutionContext,
        environment_id: Uuid,
    ) -> Result<TestingEnvironmentKey, AppError> {
        let fence = TestingEnvironmentUseFence::acquire(&self.test, environment_id).await?;
        let mut transaction = self.management_transaction(execution).await?;
        ensure_creator_or_admin(&mut transaction, execution, environment_id).await?;
        let row = sqlx::query_as::<_, (i64, Option<OffsetDateTime>, Vec<u8>, Vec<u8>)>(
            "SELECT key_generation, key_rotated_at, root_key_ciphertext, root_key_nonce \
               FROM briefcase.testing_environments \
              WHERE org_id = briefcase.current_org_id() AND environment_id = $1 \
                AND status = 'active'",
        )
        .bind(environment_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound)?;
        transaction.commit().await?;
        let key = self.decrypt(
            &row.2,
            &row.3,
            &secret_aad(environment_id, "briefcase-root"),
        )?;
        let response = TestingEnvironmentKey {
            environment_id,
            key_generation: row.0,
            key_rotated_at: row.1,
            key: key.expose_secret().to_owned(),
        };
        fence.release().await?;
        Ok(response)
    }

    /// Rotates the Briefcase root key and invalidates the prior value atomically.
    ///
    /// # Errors
    ///
    /// Fails for insufficient authority, a missing or deleted environment, or
    /// when secure key generation, encryption, or persistence fails.
    pub async fn rotate_key(
        &self,
        execution: &ExecutionContext,
        environment_id: Uuid,
        mutation: &MutationMetadata,
    ) -> Result<TestingEnvironmentWithKey, AppError> {
        let key = Uuid::new_v4().simple().to_string();
        let digest = self.digest(b"briefcase-root", key.as_bytes())?;
        let (ciphertext, nonce) = self.encrypt(
            &SecretString::from(key.clone()),
            &secret_aad(environment_id, "briefcase-root"),
        )?;
        let fence = TestingEnvironmentExclusiveFence::acquire(&self.test, environment_id).await?;
        let mut transaction = self.management_transaction(execution).await?;
        ensure_creator_or_admin(&mut transaction, execution, environment_id).await?;
        let auth = execution.authorization();
        let actor = auth.actor();
        let authority_type = actor_type(actor.kind());
        let mutation_identity = EnvironmentMutationIdentity {
            authority_type,
            authority_id: actor.id().as_str(),
            origin_app_id: auth
                .originating_application()
                .map(crate::domain::actor::ApplicationId::as_str),
            operation: ROTATE_KEY_OPERATION,
            metadata: mutation,
        };
        match self
            .claim_mutation::<TestingEnvironmentWithKey>(
                &mut transaction,
                &mutation_identity,
                environment_id,
            )
            .await?
        {
            EnvironmentMutationClaim::Replay(response) => {
                transaction.commit().await?;
                fence.release().await?;
                return Ok(response);
            }
            EnvironmentMutationClaim::Acquired(claimed_id) if claimed_id != environment_id => {
                return Err(idempotency_error());
            }
            EnvironmentMutationClaim::Acquired(_) => {}
        }
        let row = sqlx::query_as::<_, EnvironmentRow>(concat!(
            "UPDATE briefcase.testing_environments SET root_key_digest = $2, ",
            "root_key_ciphertext = $3, root_key_nonce = $4, ",
            "key_generation = key_generation + 1, key_rotated_at = clock_timestamp(), ",
            "version = version + 1 WHERE org_id = briefcase.current_org_id() ",
            "AND environment_id = $1 AND status = 'active' RETURNING ",
            environment_columns!()
        ))
        .bind(environment_id)
        .bind(digest.as_slice())
        .bind(ciphertext)
        .bind(nonce.as_slice())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound)?;
        let response = TestingEnvironmentWithKey {
            environment: environment(row)?,
            key,
        };
        self.complete_mutation(
            &mut transaction,
            &mutation_identity,
            environment_id,
            &response,
        )
        .await?;
        transaction.commit().await?;
        fence.release().await?;
        Ok(response)
    }

    /// Soft-deletes an environment, destroying its Briefcase root key.
    ///
    /// # Errors
    ///
    /// Fails for insufficient authority, a missing or already deleted
    /// environment, or a persistence error.
    pub async fn delete(
        &self,
        execution: &ExecutionContext,
        environment_id: Uuid,
        mutation: &MutationMetadata,
    ) -> Result<TestingEnvironment, AppError> {
        let fence = TestingEnvironmentExclusiveFence::acquire(&self.test, environment_id).await?;
        let mut transaction = self.management_transaction(execution).await?;
        ensure_creator_or_admin(&mut transaction, execution, environment_id).await?;
        let auth = execution.authorization();
        let actor = auth.actor();
        let authority_type = actor_type(actor.kind());
        let mutation_identity = EnvironmentMutationIdentity {
            authority_type,
            authority_id: actor.id().as_str(),
            origin_app_id: auth
                .originating_application()
                .map(crate::domain::actor::ApplicationId::as_str),
            operation: DELETE_OPERATION,
            metadata: mutation,
        };
        match self
            .claim_mutation::<TestingEnvironment>(
                &mut transaction,
                &mutation_identity,
                environment_id,
            )
            .await?
        {
            EnvironmentMutationClaim::Replay(response) => {
                transaction.commit().await?;
                fence.release().await?;
                return Ok(response);
            }
            EnvironmentMutationClaim::Acquired(claimed_id) if claimed_id != environment_id => {
                return Err(idempotency_error());
            }
            EnvironmentMutationClaim::Acquired(_) => {}
        }
        let row = sqlx::query_as::<_, EnvironmentRow>(concat!(
            "UPDATE briefcase.testing_environments SET status = 'deleted', ",
            "root_key_digest = NULL, root_key_ciphertext = NULL, root_key_nonce = NULL, ",
            "deleted_at = clock_timestamp(), purge_after = clock_timestamp() + INTERVAL '30 days', ",
            "version = version + 1 WHERE org_id = briefcase.current_org_id() ",
            "AND environment_id = $1 AND status = 'active' RETURNING ", environment_columns!()
        ))
        .bind(environment_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound)?;
        let response = environment(row)?;
        self.complete_mutation(
            &mut transaction,
            &mutation_identity,
            environment_id,
            &response,
        )
        .await?;
        transaction.commit().await?;
        fence.release().await?;
        Ok(response)
    }

    /// Restores a soft-deleted environment and necessarily issues a fresh key.
    ///
    /// # Errors
    ///
    /// Fails for insufficient authority, an expired or missing environment,
    /// persistence errors, or when the active limit is reached.
    pub async fn restore(
        &self,
        execution: &ExecutionContext,
        environment_id: Uuid,
        mutation: &MutationMetadata,
    ) -> Result<TestingEnvironmentWithKey, AppError> {
        let key = Uuid::new_v4().simple().to_string();
        let digest = self.digest(b"briefcase-root", key.as_bytes())?;
        let (ciphertext, nonce) = self.encrypt(
            &SecretString::from(key.clone()),
            &secret_aad(environment_id, "briefcase-root"),
        )?;
        let fence = TestingEnvironmentExclusiveFence::acquire(&self.test, environment_id).await?;
        let mut transaction = self.management_transaction(execution).await?;
        ensure_creator_or_admin(&mut transaction, execution, environment_id).await?;
        let auth = execution.authorization();
        let actor = auth.actor();
        let authority_type = actor_type(actor.kind());
        let mutation_identity = EnvironmentMutationIdentity {
            authority_type,
            authority_id: actor.id().as_str(),
            origin_app_id: auth
                .originating_application()
                .map(crate::domain::actor::ApplicationId::as_str),
            operation: RESTORE_OPERATION,
            metadata: mutation,
        };
        match self
            .claim_mutation::<TestingEnvironmentWithKey>(
                &mut transaction,
                &mutation_identity,
                environment_id,
            )
            .await?
        {
            EnvironmentMutationClaim::Replay(response) => {
                transaction.commit().await?;
                fence.release().await?;
                return Ok(response);
            }
            EnvironmentMutationClaim::Acquired(claimed_id) if claimed_id != environment_id => {
                return Err(idempotency_error());
            }
            EnvironmentMutationClaim::Acquired(_) => {}
        }
        sqlx::query("SELECT pg_advisory_xact_lock(742864113)")
            .execute(&mut *transaction)
            .await?;
        let active =
            sqlx::query_scalar::<_, i64>("SELECT briefcase.active_testing_environment_count()")
                .fetch_one(&mut *transaction)
                .await?;
        if active >= MAX_ACTIVE_TESTING_ENVIRONMENTS {
            return Err(AppError::conflict("testing_environment_limit_reached"));
        }
        let row = sqlx::query_as::<_, EnvironmentRow>(concat!(
            "UPDATE briefcase.testing_environments SET status = 'active', ",
            "root_key_digest = $2, root_key_ciphertext = $3, root_key_nonce = $4, ",
            "key_generation = key_generation + 1, key_rotated_at = clock_timestamp(), ",
            "deleted_at = NULL, purge_after = NULL, last_activity_at = clock_timestamp(), ",
            "version = version + 1 WHERE org_id = briefcase.current_org_id() ",
            "AND environment_id = $1 AND status = 'deleted' ",
            "AND purge_after > clock_timestamp() RETURNING ",
            environment_columns!()
        ))
        .bind(environment_id)
        .bind(digest.as_slice())
        .bind(ciphertext)
        .bind(nonce.as_slice())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound)?;
        let response = TestingEnvironmentWithKey {
            environment: environment(row)?,
            key,
        };
        self.complete_mutation(
            &mut transaction,
            &mutation_identity,
            environment_id,
            &response,
        )
        .await?;
        transaction.commit().await?;
        fence.release().await?;
        Ok(response)
    }

    /// Describes the environment selected by its root key.
    #[must_use]
    pub fn current(&self, access: &TestingEnvironmentAccess) -> TestingEnvironmentSelf {
        TestingEnvironmentSelf {
            id: access.environment_id,
            name: access.name.clone(),
            description: access.description.clone(),
            key_generation: access.key_generation,
            created_at: access.created_at,
        }
    }

    /// Atomically erases sandbox metadata after durably queuing provider work.
    ///
    /// # Errors
    ///
    /// Fails closed if provider descriptors cannot be queued, database state
    /// cannot be erased, or lifecycle bookkeeping cannot be completed.
    pub async fn clean(
        &self,
        access: &TestingEnvironmentAccess,
        request_id: &str,
        mutation: &MutationMetadata,
    ) -> Result<TestingEnvironmentCleaning, AppError> {
        let control_context = TenantContext::for_control_service(
            &access.owner_org_id,
            format!("testing-environment-clean-claim:{}", access.environment_id),
        );
        let authority_id = access.environment_id.to_string();
        let mutation_identity = EnvironmentMutationIdentity {
            authority_type: "root",
            authority_id: &authority_id,
            origin_app_id: None,
            operation: CLEAN_OPERATION,
            metadata: mutation,
        };
        if let Some(response) = self
            .claim_clean_mutation(&control_context, &mutation_identity, access.environment_id)
            .await?
        {
            return Ok(response);
        }

        let (mut cleanup_fence, next_control_version) = match self
            .acquire_clean_fence(&control_context, &mutation_identity, access)
            .await?
        {
            FencedCleanClaim::Replay(response) => return Ok(response),
            FencedCleanClaim::Proceed {
                fence,
                control_version,
            } => (fence, control_version),
        };

        let erased_rows = cleanup_fence
            .clean(&access.owner_org_id, next_control_version, request_id)
            .await;
        let erased_rows = match erased_rows {
            Ok(erased_rows) => erased_rows,
            Err(error) => {
                self.release_clean_mutation(
                    &control_context,
                    &mutation_identity,
                    access.environment_id,
                )
                .await?;
                cleanup_fence.release().await?;
                return Err(error);
            }
        };

        let response = self
            .complete_clean_mutation(
                &control_context,
                &mutation_identity,
                access.environment_id,
                erased_rows,
            )
            .await?;
        cleanup_fence.release().await?;
        Ok(response)
    }

    async fn claim_clean_mutation(
        &self,
        context: &TenantContext,
        identity: &EnvironmentMutationIdentity<'_>,
        environment_id: Uuid,
    ) -> Result<Option<TestingEnvironmentCleaning>, AppError> {
        let mut control_transaction = begin_tenant_transaction(&self.production, context).await?;
        let claim = self
            .claim_mutation::<TestingEnvironmentCleaning>(
                &mut control_transaction,
                identity,
                environment_id,
            )
            .await?;
        control_transaction.commit().await?;
        match claim {
            EnvironmentMutationClaim::Replay(response) => Ok(Some(response)),
            EnvironmentMutationClaim::Acquired(claimed_id) if claimed_id == environment_id => {
                Ok(None)
            }
            EnvironmentMutationClaim::Acquired(_) => Err(idempotency_error()),
        }
    }

    async fn acquire_clean_fence(
        &self,
        context: &TenantContext,
        identity: &EnvironmentMutationIdentity<'_>,
        access: &TestingEnvironmentAccess,
    ) -> Result<FencedCleanClaim, AppError> {
        // Unlike the five-minute idempotency lease, this distributed fence is
        // held through the atomic queue-and-erase transaction. A retry that
        // steals an expired lease waits here, then re-reads the response.
        let fence =
            TestingEnvironmentExclusiveFence::acquire(&self.test, access.environment_id).await?;
        let mut transaction = begin_tenant_transaction(&self.production, context).await?;
        match self
            .resume_clean_mutation::<TestingEnvironmentCleaning>(
                &mut transaction,
                identity,
                access.environment_id,
            )
            .await?
        {
            EnvironmentMutationClaim::Replay(response) => {
                transaction.commit().await?;
                fence.release().await?;
                return Ok(FencedCleanClaim::Replay(response));
            }
            EnvironmentMutationClaim::Acquired(claimed_id)
                if claimed_id != access.environment_id =>
            {
                return Err(idempotency_error());
            }
            EnvironmentMutationClaim::Acquired(_) => {}
        }
        let control_version = sqlx::query_scalar::<_, i64>(
            "UPDATE briefcase.testing_environments SET version = version + 1 \
              WHERE org_id = briefcase.current_org_id() AND environment_id = $1 \
                AND status = 'active' AND version = $2 \
              RETURNING version",
        )
        .bind(access.environment_id)
        .bind(access.control_version)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(control_version) = control_version else {
            self.release_mutation(&mut transaction, identity, access.environment_id)
                .await?;
            transaction.commit().await?;
            fence.release().await?;
            return Err(AppError::conflict("testing_environment_changed"));
        };
        transaction.commit().await?;
        Ok(FencedCleanClaim::Proceed {
            fence,
            control_version,
        })
    }

    async fn release_clean_mutation(
        &self,
        context: &TenantContext,
        identity: &EnvironmentMutationIdentity<'_>,
        environment_id: Uuid,
    ) -> Result<(), AppError> {
        let mut transaction = begin_tenant_transaction(&self.production, context).await?;
        self.release_mutation(&mut transaction, identity, environment_id)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn complete_clean_mutation(
        &self,
        context: &TenantContext,
        identity: &EnvironmentMutationIdentity<'_>,
        environment_id: Uuid,
        erased_rows: u64,
    ) -> Result<TestingEnvironmentCleaning, AppError> {
        let cleaned_at = OffsetDateTime::now_utc();
        let mut transaction = begin_tenant_transaction(&self.production, context).await?;
        sqlx::query(
            "UPDATE briefcase.testing_environments SET cleaned_at = $2, \
                    last_activity_at = $2 \
              WHERE org_id = briefcase.current_org_id() AND environment_id = $1",
        )
        .bind(environment_id)
        .bind(cleaned_at)
        .execute(&mut *transaction)
        .await?;
        let response = TestingEnvironmentCleaning {
            environment_id,
            erased_rows,
            cleaned_at,
        };
        self.complete_mutation(&mut transaction, identity, environment_id, &response)
            .await?;
        transaction.commit().await?;
        Ok(response)
    }

    async fn management_transaction<'a>(
        &'a self,
        execution: &ExecutionContext,
    ) -> Result<Transaction<'a, Postgres>, AppError> {
        let tenant = TenantContext::from_execution(execution);
        Ok(begin_tenant_transaction(&self.production, &tenant).await?)
    }

    async fn completed_mutation<T>(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        identity: &EnvironmentMutationIdentity<'_>,
        expected_environment_id: Option<Uuid>,
    ) -> Result<Option<T>, AppError>
    where
        T: DeserializeOwned,
    {
        let key = identity
            .metadata
            .idempotency_key
            .as_ref()
            .ok_or_else(|| AppError::bad_request("missing_idempotency_key"))?;
        let row = sqlx::query_as::<_, EnvironmentIdempotencyRow>(
            "SELECT request_hash, environment_id, status, response_ciphertext, \
                    response_nonce, locked_until \
               FROM briefcase.testing_environment_idempotency \
              WHERE org_id = briefcase.current_org_id() AND authority_type = $1 \
                AND authority_id = $2 AND origin_app_id = $3 AND operation = $4 \
                AND idempotency_key = $5",
        )
        .bind(identity.authority_type)
        .bind(identity.authority_id)
        .bind(identity.origin_app_id.unwrap_or_default())
        .bind(identity.operation)
        .bind(key.as_str())
        .fetch_optional(&mut **transaction)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        if row.request_hash.as_slice() != identity.metadata.request_fingerprint.as_slice()
            || expected_environment_id
                .is_some_and(|environment_id| environment_id != row.environment_id)
        {
            return Err(AppError::conflict("idempotency_key_reused"));
        }
        if row.status == "in_progress" {
            return Ok(None);
        }
        if row.status != "completed" {
            return Err(idempotency_error());
        }
        let ciphertext = row
            .response_ciphertext
            .as_deref()
            .ok_or_else(idempotency_error)?;
        let nonce = row
            .response_nonce
            .as_deref()
            .ok_or_else(idempotency_error)?;
        let plaintext = self.decrypt(
            ciphertext,
            nonce,
            &idempotency_aad(row.environment_id, identity.operation, key.as_str()),
        )?;
        let response =
            serde_json::from_str(plaintext.expose_secret()).map_err(|_| idempotency_error())?;
        Ok(Some(response))
    }

    async fn claim_mutation<T>(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        identity: &EnvironmentMutationIdentity<'_>,
        environment_id: Uuid,
    ) -> Result<EnvironmentMutationClaim<T>, AppError>
    where
        T: DeserializeOwned,
    {
        let key = identity
            .metadata
            .idempotency_key
            .as_ref()
            .ok_or_else(|| AppError::bad_request("missing_idempotency_key"))?;
        let origin_app_id = identity.origin_app_id.unwrap_or_default();
        let inserted = sqlx::query(
            "INSERT INTO briefcase.testing_environment_idempotency (\
                    org_id, authority_type, authority_id, origin_app_id, operation, \
                    idempotency_key, request_hash, environment_id, locked_until, expires_at) \
             VALUES (briefcase.current_org_id(), $1, $2, $3, $4, $5, $6, $7, \
                     clock_timestamp() + INTERVAL '5 minutes', \
                     clock_timestamp() + INTERVAL '24 hours') \
             ON CONFLICT DO NOTHING",
        )
        .bind(identity.authority_type)
        .bind(identity.authority_id)
        .bind(origin_app_id)
        .bind(identity.operation)
        .bind(key.as_str())
        .bind(identity.metadata.request_fingerprint.as_slice())
        .bind(environment_id)
        .execute(&mut **transaction)
        .await?;
        if inserted.rows_affected() == 1 {
            return Ok(EnvironmentMutationClaim::Acquired(environment_id));
        }

        let row = sqlx::query_as::<_, EnvironmentIdempotencyRow>(
            "SELECT request_hash, environment_id, status, response_ciphertext, \
                    response_nonce, locked_until \
               FROM briefcase.testing_environment_idempotency \
              WHERE org_id = briefcase.current_org_id() AND authority_type = $1 \
                AND authority_id = $2 AND origin_app_id = $3 AND operation = $4 \
                AND idempotency_key = $5 FOR UPDATE",
        )
        .bind(identity.authority_type)
        .bind(identity.authority_id)
        .bind(origin_app_id)
        .bind(identity.operation)
        .bind(key.as_str())
        .fetch_one(&mut **transaction)
        .await?;
        if row.request_hash.as_slice() != identity.metadata.request_fingerprint.as_slice() {
            return Err(AppError::conflict("idempotency_key_reused"));
        }
        if row.status == "completed" {
            let ciphertext = row
                .response_ciphertext
                .as_deref()
                .ok_or_else(idempotency_error)?;
            let nonce = row
                .response_nonce
                .as_deref()
                .ok_or_else(idempotency_error)?;
            let plaintext = self.decrypt(
                ciphertext,
                nonce,
                &idempotency_aad(row.environment_id, identity.operation, key.as_str()),
            )?;
            let response =
                serde_json::from_str(plaintext.expose_secret()).map_err(|_| idempotency_error())?;
            return Ok(EnvironmentMutationClaim::Replay(response));
        }
        if row.status != "in_progress" {
            return Err(idempotency_error());
        }
        if row.locked_until > OffsetDateTime::now_utc() {
            return Err(AppError::conflict("idempotency_in_progress"));
        }
        sqlx::query(
            "UPDATE briefcase.testing_environment_idempotency \
                SET locked_until = clock_timestamp() + INTERVAL '5 minutes', \
                    expires_at = clock_timestamp() + INTERVAL '24 hours' \
              WHERE org_id = briefcase.current_org_id() AND authority_type = $1 \
                AND authority_id = $2 AND origin_app_id = $3 AND operation = $4 \
                AND idempotency_key = $5",
        )
        .bind(identity.authority_type)
        .bind(identity.authority_id)
        .bind(origin_app_id)
        .bind(identity.operation)
        .bind(key.as_str())
        .execute(&mut **transaction)
        .await?;
        Ok(EnvironmentMutationClaim::Acquired(row.environment_id))
    }

    /// Rechecks a clean claim after winning the environment-wide data fence.
    /// The ordinary lease may have expired while an earlier cleaner waited;
    /// the fence, not the lease, establishes which caller may proceed. A
    /// waiter therefore replays the completed logical-erasure response.
    async fn resume_clean_mutation<T>(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        identity: &EnvironmentMutationIdentity<'_>,
        environment_id: Uuid,
    ) -> Result<EnvironmentMutationClaim<T>, AppError>
    where
        T: DeserializeOwned,
    {
        let key = identity
            .metadata
            .idempotency_key
            .as_ref()
            .ok_or_else(|| AppError::bad_request("missing_idempotency_key"))?;
        let row = sqlx::query_as::<_, EnvironmentIdempotencyRow>(
            "SELECT request_hash, environment_id, status, response_ciphertext, \
                    response_nonce, locked_until \
               FROM briefcase.testing_environment_idempotency \
              WHERE org_id = briefcase.current_org_id() AND authority_type = $1 \
                AND authority_id = $2 AND origin_app_id = $3 AND operation = $4 \
                AND idempotency_key = $5 FOR UPDATE",
        )
        .bind(identity.authority_type)
        .bind(identity.authority_id)
        .bind(identity.origin_app_id.unwrap_or_default())
        .bind(identity.operation)
        .bind(key.as_str())
        .fetch_one(&mut **transaction)
        .await?;
        if row.request_hash.as_slice() != identity.metadata.request_fingerprint.as_slice()
            || row.environment_id != environment_id
        {
            return Err(AppError::conflict("idempotency_key_reused"));
        }
        if row.status == "completed" {
            let ciphertext = row
                .response_ciphertext
                .as_deref()
                .ok_or_else(idempotency_error)?;
            let nonce = row
                .response_nonce
                .as_deref()
                .ok_or_else(idempotency_error)?;
            let plaintext = self.decrypt(
                ciphertext,
                nonce,
                &idempotency_aad(environment_id, identity.operation, key.as_str()),
            )?;
            let response =
                serde_json::from_str(plaintext.expose_secret()).map_err(|_| idempotency_error())?;
            return Ok(EnvironmentMutationClaim::Replay(response));
        }
        if row.status != "in_progress" {
            return Err(idempotency_error());
        }
        Ok(EnvironmentMutationClaim::Acquired(environment_id))
    }

    async fn complete_mutation<T>(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        identity: &EnvironmentMutationIdentity<'_>,
        environment_id: Uuid,
        response: &T,
    ) -> Result<(), AppError>
    where
        T: Serialize + ?Sized,
    {
        let key = identity
            .metadata
            .idempotency_key
            .as_ref()
            .ok_or_else(|| AppError::bad_request("missing_idempotency_key"))?;
        let serialized = serde_json::to_string(response).map_err(|_| idempotency_error())?;
        let (ciphertext, nonce) = self.encrypt(
            &SecretString::from(serialized),
            &idempotency_aad(environment_id, identity.operation, key.as_str()),
        )?;
        let completed = sqlx::query(
            "UPDATE briefcase.testing_environment_idempotency \
                SET status = 'completed', response_ciphertext = $6, response_nonce = $7, \
                    locked_until = clock_timestamp(), expires_at = clock_timestamp() + INTERVAL '24 hours' \
              WHERE org_id = briefcase.current_org_id() AND authority_type = $1 \
                AND authority_id = $2 AND origin_app_id = $3 AND operation = $4 \
                AND idempotency_key = $5 AND request_hash = $8 \
                AND environment_id = $9 AND status = 'in_progress'",
        )
        .bind(identity.authority_type)
        .bind(identity.authority_id)
        .bind(identity.origin_app_id.unwrap_or_default())
        .bind(identity.operation)
        .bind(key.as_str())
        .bind(ciphertext)
        .bind(nonce.as_slice())
        .bind(identity.metadata.request_fingerprint.as_slice())
        .bind(environment_id)
        .execute(&mut **transaction)
        .await?;
        if completed.rows_affected() != 1 {
            return Err(idempotency_error());
        }
        Ok(())
    }

    async fn release_mutation(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        identity: &EnvironmentMutationIdentity<'_>,
        environment_id: Uuid,
    ) -> Result<(), AppError> {
        let key = identity
            .metadata
            .idempotency_key
            .as_ref()
            .ok_or_else(|| AppError::bad_request("missing_idempotency_key"))?;
        sqlx::query(
            "DELETE FROM briefcase.testing_environment_idempotency \
              WHERE org_id = briefcase.current_org_id() AND authority_type = $1 \
                AND authority_id = $2 AND origin_app_id = $3 AND operation = $4 \
                AND idempotency_key = $5 AND request_hash = $6 \
                AND environment_id = $7 AND status = 'in_progress'",
        )
        .bind(identity.authority_type)
        .bind(identity.authority_id)
        .bind(identity.origin_app_id.unwrap_or_default())
        .bind(identity.operation)
        .bind(key.as_str())
        .bind(identity.metadata.request_fingerprint.as_slice())
        .bind(environment_id)
        .execute(&mut **transaction)
        .await?;
        Ok(())
    }

    fn access_from_row(&self, row: RootLookupRow) -> Result<TestingEnvironmentAccess, AppError> {
        let iam_environment_key = self.decrypt(
            &row.iam_environment_key_ciphertext,
            &row.iam_environment_key_nonce,
            &secret_aad(row.environment_id, "iam-environment-key"),
        )?;
        let iam_app_secret = self.decrypt(
            &row.iam_app_secret_ciphertext,
            &row.iam_app_secret_nonce,
            &secret_aad(row.environment_id, "iam-app-secret"),
        )?;
        Ok(TestingEnvironmentAccess {
            environment_id: row.environment_id,
            owner_org_id: row.org_id,
            name: row.name,
            description: row.description,
            key_generation: row.key_generation,
            control_version: row.control_version,
            created_at: row.created_at,
            iam_environment_id: row.iam_environment_id,
            iam_app_id: row.iam_app_id,
            iam_environment_key,
            iam_app_secret,
        })
    }

    fn digest(&self, label: &[u8], value: &[u8]) -> Result<[u8; 32], AppError> {
        let mut mac = <HmacSha256 as hmac::Mac>::new_from_slice(&self.master.0).map_err(|_| {
            AppError::Internal {
                category: "testing_environment_crypto",
            }
        })?;
        mac.update(label);
        mac.update(&[0]);
        mac.update(value);
        Ok(mac.finalize().into_bytes().into())
    }

    fn encryption_key(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(self.master.0);
        digest.update(b"silicon-briefcase/testing-environment/aes-gcm/v1");
        digest.finalize().into()
    }

    fn encrypt(&self, value: &SecretString, aad: &[u8]) -> Result<(Vec<u8>, [u8; 12]), AppError> {
        let key = self.encryption_key();
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| crypto_error())?;
        let random = *Uuid::new_v4().as_bytes();
        let mut nonce = [0_u8; 12];
        nonce.copy_from_slice(&random[..12]);
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: value.expose_secret().as_bytes(),
                    aad,
                },
            )
            .map_err(|_| crypto_error())?;
        Ok((ciphertext, nonce))
    }

    fn decrypt(
        &self,
        ciphertext: &[u8],
        nonce: &[u8],
        aad: &[u8],
    ) -> Result<SecretString, AppError> {
        if nonce.len() != 12 {
            return Err(crypto_error());
        }
        let key = self.encryption_key();
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| crypto_error())?;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| crypto_error())?;
        let value = String::from_utf8(plaintext).map_err(|_| crypto_error())?;
        Ok(SecretString::from(value))
    }
}

async fn load_environment(
    transaction: &mut Transaction<'_, Postgres>,
    environment_id: Uuid,
) -> Result<EnvironmentRow, AppError> {
    sqlx::query_as::<_, EnvironmentRow>(concat!(
        "SELECT ",
        environment_columns!(),
        " FROM briefcase.testing_environments ",
        "WHERE org_id = briefcase.current_org_id() AND environment_id = $1 ",
        "AND status IN ('active', 'deleted')"
    ))
    .bind(environment_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound)
}

async fn ensure_creator_or_admin(
    transaction: &mut Transaction<'_, Postgres>,
    execution: &ExecutionContext,
    environment_id: Uuid,
) -> Result<(), AppError> {
    let authorization = execution.authorization();
    if authorization.role().has_administrative_access() {
        return Ok(());
    }
    let actor = authorization.actor();
    let creator = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM briefcase.testing_environments \
          WHERE org_id = briefcase.current_org_id() AND environment_id = $1 \
            AND created_by_type = $2 AND created_by_id = $3)",
    )
    .bind(environment_id)
    .bind(actor_type(actor.kind()))
    .bind(actor.id().as_str())
    .fetch_one(&mut **transaction)
    .await?;
    if creator {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn require_environment_admin(execution: &ExecutionContext) -> Result<(), AppError> {
    if execution.testing_environment().is_some() {
        Err(AppError::bad_request("production_control_plane_required"))
    } else {
        Ok(())
    }
}

fn environment(row: EnvironmentRow) -> Result<TestingEnvironment, AppError> {
    let status = match row.status.as_str() {
        "active" => TestingEnvironmentStatus::Active,
        "deleted" | "purging" => TestingEnvironmentStatus::Deleted,
        _ => return Err(crypto_error()),
    };
    Ok(TestingEnvironment {
        id: row.environment_id,
        org_id: row.org_id,
        name: row.name,
        description: row.description,
        status,
        iam_environment_id: row.iam_environment_id,
        iam_app_id: row.iam_app_id,
        created_by: TestingEnvironmentCreator {
            actor_type: row.created_by_type,
            id: row.created_by_id,
        },
        key_generation: row.key_generation,
        key_rotated_at: row.key_rotated_at,
        last_activity_at: row.last_activity_at,
        cleaned_at: row.cleaned_at,
        deleted_at: row.deleted_at,
        purge_after: row.purge_after,
        version: row.version,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn validate_create(input: &TestingEnvironmentCreate) -> Result<(), AppError> {
    let name = input.name.trim();
    if name.is_empty() || name.len() > 100 || name != input.name {
        return Err(AppError::validation("invalid_testing_environment_name"));
    }
    if input
        .description
        .as_ref()
        .is_some_and(|value| value.len() > 1000)
    {
        return Err(AppError::validation(
            "invalid_testing_environment_description",
        ));
    }
    validate_iam_credential_fields(
        input.iam_environment_id,
        &input.iam_environment_key,
        &input.iam_app_id,
        &input.iam_app_secret,
    )
}

fn validate_iam_pairing(input: &TestingEnvironmentIamPairing) -> Result<(), AppError> {
    validate_iam_credential_fields(
        input.iam_environment_id,
        &input.iam_environment_key,
        &input.iam_app_id,
        &input.iam_app_secret,
    )
}

fn validate_iam_credential_fields(
    environment_id: Uuid,
    environment_key: &SecretString,
    app_id: &str,
    app_secret: &SecretString,
) -> Result<(), AppError> {
    if environment_id.is_nil() {
        return Err(AppError::validation("invalid_iam_environment_id"));
    }
    validate_root_key(environment_key.expose_secret())?;
    if !is_canonical_iam_application_id(app_id) {
        return Err(AppError::validation("invalid_iam_application_id"));
    }
    let app_secret = app_secret.expose_secret();
    if app_secret.len() != 47
        || !app_secret.starts_with("ask_")
        || !app_secret[4..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(AppError::validation("invalid_iam_application_secret"));
    }
    Ok(())
}

fn validate_patch(patch: &TestingEnvironmentPatch) -> Result<(), AppError> {
    if patch
        .name
        .as_ref()
        .is_some_and(|name| name.trim().is_empty() || name.len() > 100 || name.trim() != name)
    {
        return Err(AppError::validation("invalid_testing_environment_name"));
    }
    if patch
        .description
        .as_ref()
        .and_then(|value| value.as_ref())
        .is_some_and(|value| value.len() > 1000)
    {
        return Err(AppError::validation(
            "invalid_testing_environment_description",
        ));
    }
    Ok(())
}

fn validate_root_key(value: &str) -> Result<(), AppError> {
    if value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(AppError::bad_request("invalid_testing_environment_key"))
    }
}

const fn actor_type(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Carbon => "carbon",
        ActorKind::Silicon => "silicon",
    }
}

const fn status_name(status: TestingEnvironmentStatus) -> &'static str {
    match status {
        TestingEnvironmentStatus::Active => "active",
        TestingEnvironmentStatus::Deleted => "deleted",
    }
}

fn secret_aad(environment_id: Uuid, field: &str) -> Vec<u8> {
    format!("silicon-briefcase:{environment_id}:{field}:v1").into_bytes()
}

fn idempotency_aad(environment_id: Uuid, operation: &str, key: &str) -> Vec<u8> {
    format!("silicon-briefcase:{environment_id}:idempotency:{operation}:{key}:v1").into_bytes()
}

fn storage_target(
    owner_org_id: &str,
    bucket: &str,
    region: &str,
    prefix: &str,
    role_arn: Option<&str>,
    encryption: &str,
    kms_key_arn: Option<&str>,
) -> Result<StorageTarget, AppError> {
    let encryption = match encryption {
        "sse_s3" => EncryptionMode::SseS3,
        "sse_kms" => EncryptionMode::SseKms,
        _ => return Err(crypto_error()),
    };
    Ok(StorageTarget {
        bucket: bucket.to_owned(),
        region: region.to_owned(),
        prefix: prefix.to_owned(),
        role_arn: role_arn.map(str::to_owned),
        external_id: role_arn
            .map(|_| crate::infrastructure::s3::organization_storage_external_id(owner_org_id)),
        encryption,
        kms_key_arn: kms_key_arn.map(str::to_owned),
    })
}

fn crypto_error() -> AppError {
    AppError::Internal {
        category: "testing_environment_crypto",
    }
}

fn accept_provider_cleanup(result: &Result<(), ObjectStoreError>) -> Result<(), AppError> {
    match result {
        Ok(()) | Err(ObjectStoreError::NotFound) => Ok(()),
        Err(_) => Err(AppError::DependencyUnavailable {
            dependency: "object_storage",
        }),
    }
}

fn idempotency_error() -> AppError {
    AppError::Internal {
        category: "testing_environment_idempotency",
    }
}

fn map_environment_sql(error: sqlx::Error) -> AppError {
    if let sqlx::Error::Database(database) = &error {
        if database.is_unique_violation() {
            return AppError::conflict("testing_environment_already_exists");
        }
        if database.is_foreign_key_violation() {
            return AppError::Forbidden;
        }
    }
    AppError::from(error)
}

/// Retires idle environments and destroys control/data rows past retention.
///
/// # Errors
///
/// Fails if lifecycle rows cannot be updated, or if any expired environment's
/// queued/provider state and database rows cannot be safely retired.
pub async fn maintain_testing_environments(
    production: &PgPool,
    test: &PgPool,
    objects: &(impl ObjectStore + ?Sized),
) -> Result<(u64, u64), AppError> {
    let idle = sqlx::query_scalar::<_, Uuid>(
        "SELECT environment_id FROM briefcase.testing_environments \
          WHERE status = 'active' \
            AND last_activity_at <= clock_timestamp() - INTERVAL '30 days' \
          ORDER BY last_activity_at, environment_id",
    )
    .fetch_all(production)
    .await?;
    let mut retired = 0_u64;
    for environment_id in idle {
        retired += retire_idle_environment(production, test, environment_id).await?;
    }

    let expired = sqlx::query_scalar::<_, Uuid>(
        "SELECT environment_id FROM briefcase.testing_environments \
          WHERE (status = 'deleted' AND purge_after <= clock_timestamp()) \
             OR status = 'purging' \
          ORDER BY purge_after, environment_id",
    )
    .fetch_all(production)
    .await?;
    let mut purged = 0_u64;
    for environment_id in expired {
        purged += purge_testing_environment(production, test, objects, environment_id).await?;
    }
    Ok((retired, purged))
}

async fn retire_idle_environment(
    production: &PgPool,
    test: &PgPool,
    environment_id: Uuid,
) -> Result<u64, AppError> {
    let fence = TestingEnvironmentExclusiveFence::acquire(test, environment_id).await?;
    let retired = sqlx::query(
        "UPDATE briefcase.testing_environments SET status = 'deleted', \
                root_key_digest = NULL, root_key_ciphertext = NULL, root_key_nonce = NULL, \
                deleted_at = clock_timestamp(), purge_after = clock_timestamp() + INTERVAL '30 days', \
                version = version + 1 \
          WHERE environment_id = $1 AND status = 'active' \
            AND last_activity_at <= clock_timestamp() - INTERVAL '30 days'",
    )
    .bind(environment_id)
    .execute(production)
    .await?
    .rows_affected();
    fence.release().await?;
    Ok(retired)
}

async fn purge_testing_environment(
    production: &PgPool,
    test: &PgPool,
    objects: &(impl ObjectStore + ?Sized),
    environment_id: Uuid,
) -> Result<u64, AppError> {
    let mut fence = TestingEnvironmentExclusiveFence::acquire(test, environment_id).await?;
    let claimed = sqlx::query_as::<_, (String, i64)>(
        "UPDATE briefcase.testing_environments \
            SET status = 'purging', \
                version = CASE WHEN status = 'deleted' THEN version + 1 ELSE version END \
          WHERE environment_id = $1 \
            AND ((status = 'deleted' AND purge_after <= clock_timestamp()) \
                 OR status = 'purging') \
          RETURNING org_id, version",
    )
    .bind(environment_id)
    .fetch_optional(production)
    .await?;
    let Some((owner_org_id, control_version)) = claimed else {
        fence.release().await?;
        return Ok(0);
    };
    let request_id = format!("testing-environment-purge:{environment_id}");
    if fence
        .has_provider_cleanup(&owner_org_id, control_version, &request_id)
        .await?
    {
        fence.release().await?;
        return Ok(0);
    }
    fence
        .purge(objects, &owner_org_id, control_version, &request_id)
        .await?;
    let purged = sqlx::query(
        "DELETE FROM briefcase.testing_environments \
          WHERE environment_id = $1 AND status = 'purging'",
    )
    .bind(environment_id)
    .execute(production)
    .await?
    .rows_affected();
    fence.release().await?;
    Ok(purged)
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose};
    use secrecy::{ExposeSecret as _, SecretString};
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use super::{TestingEnvironmentStore, secret_aad, validate_root_key};

    fn store() -> TestingEnvironmentStore {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://localhost/briefcase")
            .unwrap_or_else(|error| panic!("lazy pool must build: {error}"));
        let key = SecretString::from(general_purpose::STANDARD.encode([7_u8; 32]));
        TestingEnvironmentStore::new(pool.clone(), pool, &key)
            .unwrap_or_else(|error| panic!("store must build: {error}"))
    }

    #[test]
    fn root_key_contract_is_exact() {
        assert!(validate_root_key("0123456789abcdefghijklmnopqrstuv").is_ok());
        assert!(validate_root_key("short").is_err());
        assert!(validate_root_key("0123456789abcdefghijklmnopqr-_!").is_err());
    }

    #[tokio::test]
    async fn encrypted_secrets_are_bound_to_environment_and_field() {
        let store = store();
        let secret = SecretString::from("0123456789abcdefghijklmnopqrstuv".to_owned());
        let id = Uuid::now_v7();
        let aad = secret_aad(id, "briefcase-root");
        let (ciphertext, nonce) = store
            .encrypt(&secret, &aad)
            .unwrap_or_else(|error| panic!("encryption must succeed: {error}"));
        assert_ne!(ciphertext, secret.expose_secret().as_bytes());
        let restored = store
            .decrypt(&ciphertext, &nonce, &aad)
            .unwrap_or_else(|error| panic!("decryption must succeed: {error}"));
        assert_eq!(restored.expose_secret(), secret.expose_secret());
        assert!(
            store
                .decrypt(&ciphertext, &nonce, &secret_aad(id, "iam-app-secret"))
                .is_err()
        );
    }

    #[test]
    fn generated_key_shape_is_alphanumeric() {
        let key = Uuid::new_v4().simple().to_string();
        assert_eq!(key.len(), 32);
        assert!(key.bytes().all(|byte| byte.is_ascii_alphanumeric()));
    }
}
