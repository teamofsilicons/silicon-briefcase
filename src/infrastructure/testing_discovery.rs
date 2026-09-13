//! Idempotent storage materialization after live IAM application validation.
use super::*;

impl TestingEnvironmentStore {
    /// Supplies encrypted-at-rest application credentials for signed webhook routing.
    /// # Errors
    /// Fails if the bounded active registry or a stored credential cannot be read.
    pub async fn webhook_candidates(&self) -> Result<Vec<SecretString>, AppError> {
        let rows = sqlx::query_as::<_, (Uuid, Vec<u8>, Vec<u8>)>(
            "SELECT * FROM briefcase.discovered_testing_webhook_candidates()",
        )
        .fetch_all(&self.production)
        .await?;
        rows.into_iter()
            .map(|(id, cipher, nonce)| {
                self.decrypt(&cipher, &nonce, &secret_aad(id, "iam-app-secret"))
            })
            .collect()
    }

    async fn discovery_is_current(
        &self,
        org: &str,
        id: Uuid,
        revision: i64,
        digest: &[u8],
    ) -> Result<bool, AppError> {
        let tenant = TenantContext::for_control_service(org, "iam-test-discovery");
        let mut tx = begin_tenant_transaction(&self.production, &tenant).await?;
        let current = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM briefcase.testing_environments WHERE environment_id=$1 AND status='active' AND iam_control_version=$2 AND root_key_digest=$3 AND NOT iam_sync_pending)")
            .bind(id).bind(revision).bind(digest).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(current)
    }

    /// Initializes and synchronizes local storage for a verified IAM test world.
    /// No production user session or shared IAM environment root key is required.
    ///
    /// # Errors
    /// Fails closed on stale lifecycle metadata, storage limits, or cleanup failure.
    pub async fn discover(
        &self,
        current: &silicon_iam_client::models::ApplicationTestingContext,
        secret: &SecretString,
    ) -> Result<TestingEnvironmentAccess, AppError> {
        let meta = discovery_metadata(current)?;
        let id = meta.environment_id;
        let input = TestingEnvironmentCreate {
            name: meta.name.clone(),
            description: meta.description.clone(),
            iam_environment_id: id,
            iam_environment_key: SecretString::from(String::new()),
            iam_app_id: current.application.app_id.clone(),
            iam_app_secret: secret.clone(),
        };
        let mut prepared = self.prepare_create(&input)?;
        // No IAM root is retained. Keep the legacy unique column stable per world.
        prepared.iam_digest = self.digest(b"iam-discovered-environment", id.as_bytes())?;
        // Unchanged requests must not wait for long downloads holding a shared
        // lifecycle fence. The caller still acquires its ordinary version fence.
        if self
            .discovery_is_current(&meta.org_id, id, meta.version, &prepared.root_digest)
            .await?
        {
            return self.resolve_root_key(secret).await;
        }
        let mut fence = TestingEnvironmentExclusiveFence::acquire(&self.test, id).await?;
        let tenant = TenantContext::for_control_service(&meta.org_id, "iam-test-discovery");
        let mut tx = begin_tenant_transaction(&self.production, &tenant).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(742864113)")
            .execute(&mut *tx)
            .await?;
        let prior = sqlx::query_as::<_,(String,i64,Option<i64>,Option<OffsetDateTime>,Option<Vec<u8>>,bool)>(
            "SELECT status,version,iam_control_version,iam_cleaned_at,root_key_digest,iam_sync_pending FROM briefcase.testing_environments WHERE environment_id=$1 FOR UPDATE")
            .bind(id).fetch_optional(&mut *tx).await?;
        if let Some((status, _, revision, _, _, _)) = &prior {
            if status != "active" {
                return Err(AppError::Unauthenticated);
            }
            if revision.is_some_and(|v| v > meta.version) {
                return Err(AppError::conflict("stale_iam_testing_context"));
            }
        } else {
            let count =
                sqlx::query_scalar::<_, i64>("SELECT briefcase.active_testing_environment_count()")
                    .fetch_one(&mut *tx)
                    .await?;
            if count >= MAX_ACTIVE_TESTING_ENVIRONMENTS {
                return Err(AppError::conflict("testing_environment_limit_reached"));
            }
            sqlx::query(
                "INSERT INTO briefcase.organizations(org_id) VALUES($1) ON CONFLICT DO NOTHING",
            )
            .bind(&meta.org_id)
            .execute(&mut *tx)
            .await?;
        }
        let needs_clean = prior
            .as_ref()
            .is_some_and(|(_, _, _, cleaned, _, pending)| *pending || *cleaned != meta.cleaned_at);
        let unchanged = prior
            .as_ref()
            .is_some_and(|(_, _, revision, cleaned, digest, pending)| {
                *revision == Some(meta.version)
                    && *cleaned == meta.cleaned_at
                    && !pending
                    && digest.as_deref() == Some(prepared.root_digest.as_slice())
            });
        if unchanged {
            tx.commit().await?;
            fence.release().await?;
            return self.resolve_root_key(secret).await;
        }
        if needs_clean {
            let next = sqlx::query_scalar::<_,i64>("UPDATE briefcase.testing_environments SET version=version+1,iam_sync_pending=true WHERE environment_id=$1 RETURNING version")
                .bind(id).fetch_one(&mut *tx).await?;
            tx.commit().await?;
            // The exclusive fence and durable pending flag keep old requests out.
            // Provider cleanup is queued atomically with erasing file metadata.
            fence.reset_iam(&meta.org_id, next).await?;
            tx = begin_tenant_transaction(&self.production, &tenant).await?;
        }
        sqlx::query(
            "INSERT INTO briefcase.testing_environments(org_id,environment_id,name,description,created_by_type,created_by_id,iam_environment_id,iam_app_id,iam_environment_key_digest,iam_environment_key_ciphertext,iam_environment_key_nonce,iam_app_secret_ciphertext,iam_app_secret_nonce,root_key_digest,root_key_ciphertext,root_key_nonce,iam_control_version,iam_cleaned_at,created_at) \
             VALUES($1,$2,$3,$4,$5,$6,$2,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18) \
             ON CONFLICT(environment_id) DO UPDATE SET name=EXCLUDED.name,description=EXCLUDED.description, \
             iam_environment_key_digest=EXCLUDED.iam_environment_key_digest,iam_environment_key_ciphertext=EXCLUDED.iam_environment_key_ciphertext,iam_environment_key_nonce=EXCLUDED.iam_environment_key_nonce, \
             iam_app_secret_ciphertext=EXCLUDED.iam_app_secret_ciphertext,iam_app_secret_nonce=EXCLUDED.iam_app_secret_nonce, \
             root_key_digest=EXCLUDED.root_key_digest,root_key_ciphertext=EXCLUDED.root_key_ciphertext,root_key_nonce=EXCLUDED.root_key_nonce, \
             iam_control_version=EXCLUDED.iam_control_version,iam_cleaned_at=EXCLUDED.iam_cleaned_at,iam_sync_pending=false,version=briefcase.testing_environments.version+1,updated_at=clock_timestamp()")
            .bind(&meta.org_id).bind(id).bind(&meta.name).bind(&meta.description)
            .bind(&meta.creator_type).bind(&meta.creator_id).bind(&input.iam_app_id)
            .bind(prepared.iam_digest.as_slice()).bind(prepared.iam_ciphertext).bind(prepared.iam_nonce.as_slice())
            .bind(prepared.app_ciphertext).bind(prepared.app_nonce.as_slice())
            .bind(prepared.root_digest.as_slice()).bind(prepared.root_ciphertext).bind(prepared.root_nonce.as_slice())
            .bind(meta.version).bind(meta.cleaned_at).bind(meta.created_at)
            .execute(&mut *tx).await.map_err(map_environment_sql)?;
        tx.commit().await?;
        fence.release().await?;
        self.resolve_root_key(secret).await
    }
}

impl TestingEnvironmentExclusiveFence {
    async fn reset_iam(&mut self, owner_org_id: &str, version: i64) -> Result<(), AppError> {
        let tenant = TenantContext::for_testing_environment_service(
            owner_org_id,
            TestingEnvironmentContext::new(self.environment_id, version),
            "iam-environment-reset",
        );
        let mut tx =
            begin_testing_environment_cleanup_transaction(&mut self.connection, &tenant).await?;
        sqlx::query("SELECT briefcase.reset_current_iam_testing_environment()")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

fn discovery_metadata(
    current: &silicon_iam_client::models::ApplicationTestingContext,
) -> Result<&silicon_iam_client::models::TestingEnvironmentMetadata, AppError> {
    let meta = current
        .environment
        .as_ref()
        .ok_or(AppError::Unauthenticated)?;
    if meta.environment_id != current.environment_id
        || meta.version < 1
        || !matches!(meta.creator_type.as_str(), "carbon" | "silicon")
        || meta.creator_id.is_empty()
    {
        return Err(AppError::Unauthenticated);
    }
    Ok(meta)
}
