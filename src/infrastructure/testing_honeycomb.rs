//! Protected Honeycomb participant operations; no test session grants this authority.
use super::*;
use serde::Deserialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq as _;

/// Exact request sent by Honeycomb's shared-environment coordinator.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HoneycombOperation {
    /// Stable replay identity.
    pub operation_id: Uuid,
    /// Shared environment identity.
    pub environment_id: Uuid,
    /// Owning production organization.
    pub org_id: String,
    /// Must match the deployment's configured IAM application identity.
    pub app_id: String,
    /// Monotonic lifecycle revision, independent of IAM configuration revisions.
    pub environment_revision: i64,
    /// Cleaning generation.
    pub generation: i64,
    /// Root-key version.
    pub key_version: i64,
    /// Coordinator action.
    pub action: String,
    /// Root authority, encrypted at rest and never returned in a receipt.
    pub testing_key: String,
    /// Accepted application configuration, without production secrets.
    #[serde(default)]
    pub snapshot: Value,
    /// Coordinator reason.
    #[serde(default)]
    pub reason: String,
    /// Optional application retirement selection.
    #[serde(default)]
    pub retired_apps: Vec<String>,
}

impl HoneycombOperation {
    fn validate(&self, app_id: &str) -> Result<(), AppError> {
        validate_root_key(&self.testing_key)?;
        if self.app_id != app_id
            || self.environment_id.is_nil()
            || self.operation_id.is_nil()
            || self.org_id.is_empty()
            || self.org_id.len() > 128
            || self.environment_revision < 1
            || self.generation < 1
            || self.key_version < 1
            || (self.action == "retire-applications"
                && (self.retired_apps.is_empty() || self.retired_apps.iter().any(String::is_empty)))
            || !matches!(
                self.action.as_str(),
                "prepare"
                    | "import"
                    | "refresh-import"
                    | "rotate-key"
                    | "clean"
                    | "disable"
                    | "restore"
                    | "purge"
                    | "retire-applications"
            )
        {
            return Err(AppError::validation("invalid_honeycomb_operation"));
        }
        Ok(())
    }
    fn retires_participant(&self) -> bool {
        self.action == "retire-applications"
            && self.retired_apps.iter().any(|app| app == &self.app_id)
    }
    fn receipt(&self, state: &str) -> Value {
        json!({"operation_id":self.operation_id,"environment_id":self.environment_id,
            "app_id":self.app_id,"environment_revision":self.environment_revision,
            "generation":self.generation,"key_version":self.key_version,"state":state,
            "retired_apps":self.retired_apps})
    }
}

impl TestingEnvironmentStore {
    /// Configures the dedicated service credential, independently of test secrets.
    #[must_use]
    pub fn with_honeycomb_token(mut self, token: Option<SecretString>) -> Self {
        self.honeycomb_token = token;
        self
    }
    /// Verifies service authority using constant-time digests.
    /// # Errors
    /// Fails closed when the integration is unconfigured or the token is wrong.
    pub fn authenticate_honeycomb(&self, token: &str) -> Result<(), AppError> {
        let expected = self
            .honeycomb_token
            .as_ref()
            .ok_or(AppError::Unauthenticated)?;
        let actual: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let expected: [u8; 32] = Sha256::digest(expected.expose_secret().as_bytes()).into();
        if bool::from(actual.ct_eq(&expected)) {
            Ok(())
        } else {
            Err(AppError::Unauthenticated)
        }
    }

    /// Returns a durable secret-free receipt for lost-response recovery.
    /// # Errors
    /// Fails for an unknown operation or database failure.
    pub async fn honeycomb_receipt(
        &self,
        org: &str,
        id: Uuid,
        operation: Uuid,
    ) -> Result<Value, AppError> {
        let tenant = TenantContext::for_control_service(org, "honeycomb-receipt");
        let mut tx = begin_tenant_transaction(&self.production, &tenant).await?;
        let receipt=sqlx::query_scalar("SELECT receipt FROM briefcase.honeycomb_operations WHERE environment_id=$1 AND operation_id=$2 AND receipt->>'app_id'=$3")
            .bind(id).bind(operation).bind(&self.app_id).fetch_optional(&mut *tx).await?.ok_or(AppError::NotFound)?;
        tx.commit().await?;
        Ok(receipt)
    }

    /// Applies a retry-safe lifecycle request under the same fence as file traffic.
    /// # Errors
    /// Rejects stale revisions, altered replays, invalid transitions, or database failures.
    pub async fn honeycomb_operation(&self, op: &HoneycombOperation) -> Result<Value, AppError> {
        // Reject another participant before touching even its failure receipt.
        op.validate(&self.app_id)?;
        let result = self.execute_honeycomb_operation(op).await;
        if result.is_err() {
            let tenant = TenantContext::for_control_service(&op.org_id, "honeycomb-failure");
            let mut tx = begin_tenant_transaction(&self.production, &tenant).await?;
            let hash = self.digest(
                b"honeycomb-operation",
                &serde_json::to_vec(op).map_err(|_| crypto_error())?,
            )?;
            sqlx::query("UPDATE briefcase.honeycomb_operations SET receipt=$4 WHERE environment_id=$1 AND operation_id=$2 AND request_hash=$3 AND receipt->>'state'<>'completed'")
                .bind(op.environment_id).bind(op.operation_id).bind(hash.as_slice()).bind(op.receipt("failed")).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        result
    }

    #[allow(
        clippy::too_many_lines,
        reason = "claim, cleanup and receipt remain together to make cross-database ordering explicit"
    )]
    async fn execute_honeycomb_operation(
        &self,
        op: &HoneycombOperation,
    ) -> Result<Value, AppError> {
        let hash = self.digest(
            b"honeycomb-operation",
            &serde_json::to_vec(op).map_err(|_| crypto_error())?,
        )?;
        let mut fence =
            TestingEnvironmentExclusiveFence::acquire(&self.test, op.environment_id).await?;
        let tenant = TenantContext::for_control_service(&op.org_id, "honeycomb-lifecycle");
        let mut tx = begin_tenant_transaction(&self.production, &tenant).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(742864113)")
            .execute(&mut *tx)
            .await?;
        let replay=sqlx::query_as::<_,(Vec<u8>,Value,bool)>("SELECT request_hash,receipt,cleanup_queued FROM briefcase.honeycomb_operations WHERE environment_id=$1 AND operation_id=$2")
            .bind(op.environment_id).bind(op.operation_id).fetch_optional(&mut *tx).await?;
        if let Some((prior, receipt, _)) = &replay {
            if prior.as_slice() != hash {
                return Err(AppError::conflict("honeycomb_idempotency_conflict"));
            }
            if receipt["state"] == "completed" {
                tx.commit().await?;
                fence.release().await?;
                return Ok(receipt.clone());
            }
            sqlx::query("UPDATE briefcase.honeycomb_operations SET receipt=$3 WHERE environment_id=$1 AND operation_id=$2")
                .bind(op.environment_id).bind(op.operation_id).bind(op.receipt("pending"))
                .execute(&mut *tx).await?;
        }
        let prior=sqlx::query_as::<_,(i64,i64,i64,String,Uuid,Vec<u8>,Vec<u8>)>("SELECT environment_revision,generation,key_version,state,operation_id,testing_key_ciphertext,testing_key_nonce FROM briefcase.honeycomb_environments WHERE environment_id=$1 FOR UPDATE")
            .bind(op.environment_id).fetch_optional(&mut *tx).await?;
        if let Some((revision, generation, key_version, state, operation, key_cipher, key_nonce)) =
            &prior
        {
            let participant: Option<String> = sqlx::query_scalar("SELECT receipt->>'app_id' FROM briefcase.honeycomb_operations WHERE environment_id=$1 AND operation_id=$2")
                .bind(op.environment_id).bind(operation).fetch_optional(&mut *tx).await?;
            if participant.as_deref() != Some(self.app_id.as_str()) {
                return Err(AppError::conflict("honeycomb_participant_identity_changed"));
            }
            let reimporting =
                state == "retired" && matches!(op.action.as_str(), "prepare" | "import");
            if (*revision >= op.environment_revision && *operation != op.operation_id)
                || op.generation < *generation
                || (op.generation != *generation && op.action != "clean" && !reimporting)
                || (op.key_version != *key_version && op.action != "rotate-key" && !reimporting)
                || op.key_version < *key_version
                || state == "purged"
                || (state == "retired"
                    && !matches!(op.action.as_str(), "prepare" | "import" | "purge"))
                || (state == "cleaning" && *operation != op.operation_id)
                || (state == "disabled"
                    && !matches!(op.action.as_str(), "restore" | "purge" | "disable"))
                || (op.action == "clean"
                    && *operation != op.operation_id
                    && op.generation <= *generation)
                || (op.action == "rotate-key"
                    && *operation != op.operation_id
                    && op.key_version <= *key_version)
            {
                return Err(AppError::conflict("stale_honeycomb_operation"));
            }
            let current_key = self.decrypt(
                key_cipher,
                key_nonce,
                &secret_aad(op.environment_id, "honeycomb-key"),
            )?;
            let unchanged_key = current_key.expose_secret() == op.testing_key;
            if (*key_version == op.key_version && !unchanged_key)
                || (op.action == "rotate-key" && *operation != op.operation_id && unchanged_key)
            {
                return Err(AppError::conflict("honeycomb_key_version_mismatch"));
            }
        } else if !matches!(op.action.as_str(), "prepare" | "import") {
            return Err(AppError::conflict("honeycomb_prepare_required"));
        }
        if replay.is_none() {
            if prior.as_ref().is_none_or(|p| p.3 != "active")
                && matches!(op.action.as_str(), "prepare" | "import" | "restore")
            {
                let count: i64 =
                    sqlx::query_scalar("SELECT briefcase.honeycomb_active_environment_count($1)")
                        .bind(op.environment_id)
                        .fetch_one(&mut *tx)
                        .await?;
                if count >= MAX_ACTIVE_TESTING_ENVIRONMENTS {
                    return Err(AppError::conflict("testing_environment_limit_reached"));
                }
            }
            let state = match op.action.as_str() {
                "retire-applications" if op.retires_participant() => "cleaning",
                "retire-applications" => prior.as_ref().map_or("active", |p| p.3.as_str()),
                "clean" | "purge" => "cleaning",
                "disable" => "disabled",
                _ => "active",
            };
            let (cipher, nonce) = self.encrypt(
                &SecretString::from(op.testing_key.clone()),
                &secret_aad(op.environment_id, "honeycomb-key"),
            )?;
            sqlx::query("INSERT INTO briefcase.honeycomb_environments(environment_id,org_id,environment_revision,generation,key_version,state,operation_id,testing_key_ciphertext,testing_key_nonce) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(environment_id) DO UPDATE SET environment_revision=$3,generation=$4,key_version=$5,state=$6,operation_id=$7,testing_key_ciphertext=$8,testing_key_nonce=$9")
                .bind(op.environment_id).bind(&op.org_id).bind(op.environment_revision).bind(op.generation).bind(op.key_version).bind(state).bind(op.operation_id).bind(cipher).bind(nonce.as_slice()).execute(&mut *tx).await?;
            if op.action == "clean" || op.retires_participant() {
                // Pre-clean activity must never be relabeled as use of the new generation.
                sqlx::query("UPDATE briefcase.honeycomb_environments SET last_activity_at=NULL,activity_reported_at=NULL WHERE environment_id=$1")
                    .bind(op.environment_id).execute(&mut *tx).await?;
            }
            if op.action == "clean" {
                sqlx::query("UPDATE briefcase.honeycomb_environments SET require_iam_clean=true,iam_cleaned_before=(SELECT iam_cleaned_at FROM briefcase.testing_environments WHERE environment_id=$1) WHERE environment_id=$1")
                    .bind(op.environment_id).execute(&mut *tx).await?;
            }
            // Invalidate already resolved requests and webhook/jobs from an older lifecycle.
            sqlx::query("UPDATE briefcase.testing_environments SET version=version+1,honeycomb_sync_pending=honeycomb_sync_pending OR $2 WHERE environment_id=$1")
                .bind(op.environment_id).bind(matches!(op.action.as_str(), "clean"|"rotate-key")).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO briefcase.honeycomb_operations(environment_id,operation_id,org_id,request_hash,receipt) VALUES($1,$2,$3,$4,$5)")
                .bind(op.environment_id).bind(op.operation_id).bind(&op.org_id).bind(hash.as_slice()).bind(op.receipt("pending")).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        let clean = matches!(op.action.as_str(), "clean" | "purge") || op.retires_participant();
        if clean {
            // Reset is idempotent while access is blocked. A crash between databases can
            // repeat it; deterministic cleanup IDs retain every outstanding descriptor.
            if !replay.as_ref().is_some_and(|r| r.2) {
                fence.reset_iam(&op.org_id, op.environment_revision).await?;
                let mut tx = begin_tenant_transaction(&self.production, &tenant).await?;
                sqlx::query("UPDATE briefcase.honeycomb_operations SET cleanup_queued=true WHERE environment_id=$1 AND operation_id=$2")
                    .bind(op.environment_id).bind(op.operation_id).execute(&mut *tx).await?;
                tx.commit().await?;
            }
            let context = TenantContext::for_testing_environment_service(
                &op.org_id,
                TestingEnvironmentContext::new(op.environment_id, op.environment_revision),
                "honeycomb-cleanup",
            );
            let mut data =
                begin_testing_environment_cleanup_transaction(&mut fence.connection, &context)
                    .await?;
            let pending: bool = sqlx::query_scalar("SELECT briefcase.honeycomb_cleanup_pending()")
                .fetch_one(&mut *data)
                .await?;
            if pending {
                data.commit().await?;
                fence.release().await?;
                return Ok(op.receipt("pending"));
            }
            if op.action == "purge" || op.retires_participant() {
                sqlx::query("SELECT briefcase.honeycomb_purge_data()")
                    .execute(&mut *data)
                    .await?;
            }
            data.commit().await?;
        }
        let receipt = op.receipt("completed");
        let mut tx = begin_tenant_transaction(&self.production, &tenant).await?;
        sqlx::query("UPDATE briefcase.honeycomb_operations SET receipt=$3 WHERE environment_id=$1 AND operation_id=$2")
            .bind(op.environment_id).bind(op.operation_id).bind(&receipt).execute(&mut *tx).await?;
        let state = match op.action.as_str() {
            "disable" => "disabled",
            "purge" => "purged",
            "retire-applications" if op.retires_participant() => "retired",
            "retire-applications" => prior.as_ref().map_or("active", |p| p.3.as_str()),
            _ => "active",
        };
        sqlx::query("UPDATE briefcase.honeycomb_environments SET state=$2,testing_key_ciphertext=CASE WHEN $2='purged' THEN ''::bytea ELSE testing_key_ciphertext END,testing_key_nonce=CASE WHEN $2='purged' THEN ''::bytea ELSE testing_key_nonce END WHERE environment_id=$1")
            .bind(op.environment_id).bind(state).execute(&mut *tx).await?;
        if op.action == "purge" || op.retires_participant() {
            sqlx::query("DELETE FROM briefcase.testing_environments WHERE environment_id=$1")
                .bind(op.environment_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        fence.release().await?;
        Ok(receipt)
    }

    pub(super) async fn ensure_honeycomb_access(&self, id: Uuid) -> Result<(), AppError> {
        let allowed: bool = sqlx::query_scalar("SELECT briefcase.honeycomb_environment_access($1)")
            .bind(id)
            .fetch_one(&self.production)
            .await?;
        if allowed {
            Ok(())
        } else {
            Err(AppError::Unauthenticated)
        }
    }
    pub(super) async fn validate_honeycomb_discovery(
        &self,
        meta: &silicon_iam_client::models::TestingEnvironmentMetadata,
    ) -> Result<(), AppError> {
        let current: bool =
            sqlx::query_scalar("SELECT briefcase.honeycomb_discovery_matches($1,$2,$3,$4)")
                .bind(meta.environment_id)
                .bind(&meta.org_id)
                .bind(meta.key_generation)
                .bind(meta.cleaned_at)
                .fetch_one(&self.production)
                .await?;
        if current {
            Ok(())
        } else {
            Err(AppError::conflict("stale_iam_testing_context"))
        }
    }
}

impl TestingEnvironmentStore {
    /// Delivers persisted activity using the current Honeycomb root and generation.
    /// Failures retain the outbox item for a later retry, including after restart.
    /// # Errors
    /// Reports database, encryption, or Honeycomb transport failures without secrets.
    pub async fn report_honeycomb_activity(&self, origin: &url::Url) -> Result<(), AppError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|_| AppError::DependencyUnavailable {
                dependency: "honeycomb",
            })?;
        let rows = sqlx::query_as::<_, (Uuid, String, i64, i64, Vec<u8>, Vec<u8>, OffsetDateTime)>(
            "SELECT * FROM briefcase.honeycomb_activity_outbox()",
        )
        .fetch_all(&self.production)
        .await?;
        let mut failed = false;
        for (id, org, generation, key_version, cipher, nonce, at) in rows {
            let result: Result<(), AppError> = async {
            let tenant = TenantContext::for_control_service(&org, "honeycomb-activity-identity");
            let mut tx = begin_tenant_transaction(&self.production, &tenant).await?;
            let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM briefcase.honeycomb_environments e JOIN briefcase.honeycomb_operations o ON o.environment_id=e.environment_id AND o.operation_id=e.operation_id WHERE e.environment_id=$1 AND o.receipt->>'app_id'=$2)")
                .bind(id).bind(&self.app_id).fetch_one(&mut *tx).await?;
            tx.commit().await?;
            if !current {
                return Ok(());
            }
            let key = self.decrypt(&cipher, &nonce, &secret_aad(id, "honeycomb-key"))?;
            let mut endpoint = origin.join("api/v1/").map_err(|_| crypto_error())?;
            endpoint.path_segments_mut().map_err(|()| crypto_error())?
                .pop_if_empty().extend(["environments", &id.to_string(), "apps", &self.app_id, "activity"]);
            let response = client
                .post(endpoint)
                .header("X-Testing-Environment-Key", key.expose_secret())
                .header(
                    "Idempotency-Key",
                    format!(
                        "{}:{id}:{generation}:{key_version}:{}",
                        self.app_id, at.unix_timestamp_nanos()
                    ),
                )
                .json(&json!({"generation":generation,"key_version":key_version}))
                .send()
                .await
                .map_err(|_| AppError::DependencyUnavailable {
                    dependency: "honeycomb",
                })?;
            if !response.status().is_success() {
                return Err(AppError::DependencyUnavailable {
                    dependency: "honeycomb",
                });
            }
            let tenant = TenantContext::for_control_service(&org, "honeycomb-activity-receipt");
            let mut tx = begin_tenant_transaction(&self.production, &tenant).await?;
            sqlx::query("UPDATE briefcase.honeycomb_environments SET activity_reported_at=GREATEST(activity_reported_at,$2) WHERE environment_id=$1 AND generation=$3 AND key_version=$4 AND state='active'")
                .bind(id).bind(at).bind(generation).bind(key_version).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(())
            }.await;
            failed |= result.is_err();
        }
        if failed {
            Err(AppError::DependencyUnavailable {
                dependency: "honeycomb",
            })
        } else {
            Ok(())
        }
    }
}
