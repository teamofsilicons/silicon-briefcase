//! Canonical IAM wire identities mapped to Briefcase-owned persistence keys.

#[cfg(test)]
use std::sync::Arc;

use serde_json::{Value, json};
use sqlx::PgPool;

use super::{
    Deserialize, IamClient, IamClientError, IamEnvironmentCredential, OffsetDateTime, Serialize,
    Uuid, binding_mismatch, invalid_response, is_canonical_iam_organization_id,
};

#[cfg(test)]
type TestBindings = std::collections::BTreeMap<(Option<Uuid>, String, String), Uuid>;

#[derive(Clone)]
pub(super) enum IdentityKeys {
    Unconfigured,
    Database {
        production: PgPool,
        testing: Option<PgPool>,
    },
    #[cfg(test)]
    Memory(Arc<tokio::sync::Mutex<TestBindings>>),
}

impl IdentityKeys {
    pub(super) const fn unconfigured() -> Self {
        Self::Unconfigured
    }
    #[cfg(test)]
    pub(super) fn for_tests() -> Self {
        Self::Memory(Arc::default())
    }

    pub(super) async fn resolve(
        &self,
        environment: Option<Uuid>,
        kind: &str,
        public_id: &str,
    ) -> Result<Uuid, IamClientError> {
        match self {
            Self::Unconfigured => Err(invalid_response("identity_store_missing")),
            Self::Database {
                production,
                testing,
            } => {
                let pool = if environment.is_some() {
                    testing
                        .as_ref()
                        .ok_or_else(|| binding_mismatch("identity.testing_database"))?
                } else {
                    production
                };
                let mut tx = pool
                    .begin()
                    .await
                    .map_err(|_| invalid_response("identity_store_unavailable"))?;
                sqlx::query("SELECT set_config('briefcase.testing_environment_id',$1,true)")
                    .bind(environment.map_or_else(String::new, |id| id.to_string()))
                    .execute(&mut *tx)
                    .await
                    .map_err(|_| invalid_response("identity_store_unavailable"))?;
                let id =
                    sqlx::query_scalar("SELECT briefcase.resolve_iam_identity_key($1,$2,$3,$4)")
                        .bind(environment.unwrap_or(Uuid::nil()))
                        .bind(kind)
                        .bind(public_id)
                        .bind(Uuid::now_v7())
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|_| invalid_response("identity_backfill_or_store_unavailable"))?;
                tx.commit()
                    .await
                    .map_err(|_| invalid_response("identity_store_unavailable"))?;
                Ok(id)
            }
            #[cfg(test)]
            Self::Memory(values) => Ok(*values
                .lock()
                .await
                .entry((environment, kind.to_owned(), public_id.to_owned()))
                .or_insert_with(Uuid::now_v7)),
        }
    }
}

impl IamClient {
    /// Selects independent production and testing stores for local identity keys.
    #[must_use]
    pub fn with_identity_databases(mut self, production: PgPool, testing: Option<PgPool>) -> Self {
        self.identity_keys = IdentityKeys::Database {
            production,
            testing,
        };
        self
    }

    pub(super) async fn prepare<T: Serialize>(
        &self,
        response: T,
        environment: Option<&IamEnvironmentCredential>,
    ) -> Result<Value, IamClientError> {
        let mut value =
            serde_json::to_value(response).map_err(|_| invalid_response("sdk_model"))?;
        if serde_json::to_vec(&value)
            .map_err(|_| invalid_response("sdk_model"))?
            .len()
            > self.max_response_bytes
        {
            return Err(invalid_response("response_size"));
        }
        if value.get("active").and_then(Value::as_bool) == Some(false) {
            return Ok(value);
        }
        let selected = environment.and_then(|env| env.environment_id);
        if environment.is_some() && selected.is_none() {
            return Err(binding_mismatch("identity.testing_environment"));
        }
        let audience = self.application_identity(environment).0.as_str();
        let (kind, public_id) = validated_identity(&value, audience, selected)?;
        let local = self
            .identity_keys
            .resolve(selected, &kind, &public_id)
            .await?;
        if value.get("actor").is_some() {
            value["actor"]["principal_id"] = json!(local);
        }
        if value.get("active").is_some() {
            value["principal_id"] = json!(local);
            value["public_id"] = json!(public_id);
            value["actor_type"] = json!(kind);
        }
        if let Some(snapshot) = value.get_mut("authorization") {
            self.map_snapshot(snapshot, local, selected).await?;
        }
        if let Some(snapshots) = value
            .get_mut("authorizations")
            .and_then(Value::as_array_mut)
        {
            for snapshot in snapshots {
                self.map_snapshot(snapshot, local, selected).await?;
            }
        }
        if let Some(membership) = value.get("membership_id").and_then(Value::as_str) {
            value["membership_id"] = json!(
                self.identity_keys
                    .resolve(selected, "membership", membership)
                    .await?
            );
        }
        Ok(value)
    }

    async fn map_snapshot(
        &self,
        snapshot: &mut Value,
        local: Uuid,
        environment: Option<Uuid>,
    ) -> Result<(), IamClientError> {
        let member = snapshot["membership_id"]
            .as_str()
            .ok_or_else(|| invalid_response("authorization.membership_id"))?;
        let member = self
            .identity_keys
            .resolve(environment, "membership", member)
            .await?;
        snapshot["membership_id"] = json!(member);
        snapshot["principal_id"] = json!(local);
        Ok(())
    }
}

fn validated_identity(
    value: &Value,
    audience: &str,
    selected: Option<Uuid>,
) -> Result<(String, String), IamClientError> {
    let snapshots: Vec<Value> = if let Some(snapshot) = value.get("authorization") {
        if value.get("authorizations").is_some() {
            return Err(binding_mismatch("authorization.multiple_shapes"));
        }
        vec![snapshot.clone()]
    } else {
        value
            .get("authorizations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let mut identity = None;
    for snapshot in &snapshots {
        let current = snapshot_identity(snapshot, audience, selected)?;
        if identity
            .as_ref()
            .is_some_and(|identity| identity != &current)
        {
            return Err(binding_mismatch("authorization.identity"));
        }
        identity = Some(current);
    }
    if let Some(actor) = value.get("actor") {
        let current = canonical_actor(
            actor.get("type").and_then(Value::as_str),
            actor.get("public_id").and_then(Value::as_str),
        )?;
        if identity
            .as_ref()
            .is_some_and(|identity| identity != &current)
        {
            return Err(binding_mismatch("authorization.actor"));
        }
        identity = Some(current);
    }
    let (kind, public_id) = identity
        .or_else(|| {
            canonical_actor(
                value.get("actor_type").and_then(Value::as_str),
                value.get("public_id").and_then(Value::as_str),
            )
            .ok()
        })
        .ok_or_else(|| invalid_response("identity.canonical_id"))?;
    if value
        .get("public_id")
        .and_then(Value::as_str)
        .is_some_and(|id| id != public_id)
        || value
            .get("actor_type")
            .and_then(Value::as_str)
            .is_some_and(|value| value != kind)
    {
        return Err(binding_mismatch("identity.introspection"));
    }
    if value.get("active").is_some()
        && (value.get("client_id").and_then(Value::as_str) != Some(audience)
            || value
                .get("audience")
                .and_then(Value::as_str)
                .is_some_and(|value| value != audience)
            || value
                .get("expires_at")
                .and_then(Value::as_i64)
                .is_none_or(|expiry| expiry <= OffsetDateTime::now_utc().unix_timestamp()))
    {
        return Err(IamClientError::Rejected);
    }
    if let Some(membership) = value.get("membership_id").and_then(Value::as_str) {
        let org = value
            .get("org_id")
            .and_then(Value::as_str)
            .ok_or_else(|| binding_mismatch("identity.organization"))?;
        if membership != format!("{public_id}[{org}]") {
            return Err(binding_mismatch("identity.membership"));
        }
    }
    Ok((kind, public_id))
}

fn canonical_actor(
    kind: Option<&str>,
    id: Option<&str>,
) -> Result<(String, String), IamClientError> {
    let kind = kind.ok_or_else(|| invalid_response("identity.actor_type"))?;
    let id = id.ok_or_else(|| invalid_response("identity.public_id"))?;
    if !valid_public_identity(kind, id) {
        return Err(invalid_response("identity.public_id"));
    }
    Ok((kind.to_owned(), id.to_owned()))
}

pub(crate) fn valid_public_identity(kind: &str, id: &str) -> bool {
    let label = |part: &str| {
        (3..=50).contains(&part.len())
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
    };
    match kind {
        "carbon" => id
            .strip_prefix("c:")
            .is_some_and(|handle| handle.len() <= 30 && label(handle)),
        "silicon" => id.strip_prefix("si:").is_some_and(label),
        _ => false,
    }
}

fn snapshot_identity(
    snapshot: &Value,
    audience: &str,
    environment: Option<Uuid>,
) -> Result<(String, String), IamClientError> {
    let identity = canonical_actor(
        snapshot["actor_type"].as_str(),
        snapshot["public_id"].as_str(),
    )?;
    let org = snapshot["org_id"]
        .as_str()
        .filter(|org| is_canonical_iam_organization_id(org))
        .ok_or_else(|| invalid_response("authorization.org_id"))?;
    let plane = serde_json::from_value::<Option<Uuid>>(snapshot["testing_environment_id"].clone())
        .map_err(|_| invalid_response("authorization.environment"))?;
    if snapshot["audience"].as_str() != Some(audience)
        || plane != environment
        || snapshot["membership_id"].as_str() != Some(format!("{}[{org}]", identity.1).as_str())
    {
        return Err(binding_mismatch("authorization.canonical_identity"));
    }
    Ok(identity)
}

#[derive(Debug, Deserialize)]
pub(super) struct LocalAuthorization {
    pub principal_id: Uuid,
    pub membership_id: Uuid,
    pub actor_type: Option<silicon_iam_client::models::ApplicationAuthorizationActorType>,
    pub public_id: Option<String>,
    pub organization_id: Uuid,
    pub org_id: String,
    pub membership_version: i64,
    pub authorization_epoch: i64,
    pub audience: String,
    pub testing_environment_id: Option<Uuid>,
    pub scopes: Vec<String>,
    pub org_role: Option<String>,
    pub tags: Option<Vec<silicon_iam_client::models::AuthorizationTag>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LocalIntrospection {
    pub active: bool,
    pub public_id: Option<String>,
    pub principal_id: Option<Uuid>,
    pub actor_type: Option<silicon_iam_client::models::TokenIntrospectionActorType>,
    pub client_id: Option<String>,
    pub audience: Option<String>,
    pub expires_at: Option<i64>,
    pub org_id: Option<String>,
    pub authorization: Option<LocalAuthorization>,
    pub authorizations: Option<Vec<LocalAuthorization>>,
}
