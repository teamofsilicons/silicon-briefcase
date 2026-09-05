//! Transactional persistence for verified IAM webhook projections.

use async_trait::async_trait;
use serde_json::Value;
use sqlx::{Postgres, Transaction};

use crate::{
    application::{
        context::TestingEnvironmentContext,
        webhook::{IamWebhookRepository, VerifiedIamWebhook, WebhookApplyOutcome},
    },
    domain::actor::is_canonical_iam_organization_id,
    error::AppError,
};

use super::{PostgresRepository, begin_projection_transaction, roots};

const SOURCE: &str = "silicon-iam";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReceiptClaim {
    Inserted,
    Duplicate,
}

struct MemberProjection<'a> {
    actor_type: &'a str,
    actor_id: &'a str,
    principal_id: uuid::Uuid,
    membership_id: uuid::Uuid,
    authorization_epoch: Option<i64>,
    role: Option<&'a str>,
    status: &'a str,
    version: i64,
}

#[async_trait]
impl IamWebhookRepository for PostgresRepository {
    async fn apply_iam_event(
        &self,
        webhook: &VerifiedIamWebhook,
        testing_environment: Option<TestingEnvironmentContext>,
    ) -> Result<WebhookApplyOutcome, AppError> {
        let event = &webhook.event;
        if !is_known_application_event(&event.event_type) {
            return Ok(WebhookApplyOutcome::Ignored);
        }
        if webhook.is_testing() != testing_environment.is_some() {
            return Err(AppError::Unauthenticated);
        }
        let testing_environment_id = testing_environment.map(TestingEnvironmentContext::id);
        let pool = if testing_environment_id.is_some() {
            self.test_pool().ok_or(AppError::DependencyUnavailable {
                dependency: "test_database",
            })?
        } else {
            self.pool()
        };
        let org_id = resolve_event_organization(pool, webhook, testing_environment_id).await?;
        let version = i64::try_from(event.aggregate_version)
            .map_err(|_| AppError::validation("invalid_iam_aggregate_version"))?;
        if version < 1 {
            return Err(AppError::validation("invalid_iam_aggregate_version"));
        }

        let event_id = event.event_id.to_string();
        let mut transaction =
            begin_projection_transaction(pool, &org_id, &event_id, testing_environment_id).await?;
        if let Some(environment) = testing_environment
            && !self.testing_environment_is_current(environment).await?
        {
            transaction.rollback().await?;
            return Err(AppError::conflict("testing_environment_changed"));
        }

        roots::lock_organization_reconciliation(&mut transaction).await?;
        ensure_organization(&mut transaction, &org_id, event.organization_id).await?;
        if testing_environment_id.is_some() {
            super::quota::ensure_testing_environment_storage_limit(&mut transaction).await?;
        }
        if claim_receipt(&mut transaction, webhook, &org_id).await? == ReceiptClaim::Duplicate {
            transaction.commit().await?;
            return Ok(WebhookApplyOutcome::Duplicate);
        }

        let outcome = if event.schema_version == 1 {
            apply_projection(&mut transaction, webhook, version).await?
        } else {
            WebhookApplyOutcome::Ignored
        };
        if event.schema_version == 1 {
            roots::reconcile_system_roots(&mut transaction, None).await?;
        }
        finish_receipt(&mut transaction, &event_id, outcome).await?;
        transaction.commit().await?;
        Ok(outcome)
    }
}

async fn resolve_event_organization(
    pool: &sqlx::PgPool,
    webhook: &VerifiedIamWebhook,
    testing_environment_id: Option<uuid::Uuid>,
) -> Result<String, AppError> {
    let event = &webhook.event;
    let resolved = if let Some(internal_id) = event.organization_id {
        sqlx::query_scalar::<_, Option<String>>("SELECT briefcase.resolve_iam_organization_id($1)")
            .bind(internal_id)
            .fetch_one(pool)
            .await?
    } else {
        None
    };
    if let Some(public_id) = &event.org_id {
        let storage_id = storage_organization_id(public_id.as_str(), testing_environment_id);
        if resolved
            .as_deref()
            .is_some_and(|known| known != storage_id.as_str())
        {
            return Err(AppError::validation("iam_organization_binding_mismatch"));
        }
        return Ok(storage_id);
    }
    let resolved =
        resolved.ok_or_else(|| AppError::validation("missing_iam_organization_snapshot"))?;
    let public_id = match testing_environment_id {
        Some(environment_id) => resolved
            .strip_prefix(&format!("{environment_id}:"))
            .ok_or_else(|| AppError::validation("iam_organization_binding_mismatch"))?,
        None => resolved.as_str(),
    };
    if !is_canonical_iam_organization_id(public_id) {
        return Err(AppError::validation("invalid_iam_organization_snapshot"));
    }
    Ok(resolved)
}

fn storage_organization_id(public_id: &str, testing_environment_id: Option<uuid::Uuid>) -> String {
    testing_environment_id.map_or_else(
        || public_id.to_owned(),
        |environment_id| format!("{environment_id}:{public_id}"),
    )
}

async fn ensure_organization(
    transaction: &mut Transaction<'_, Postgres>,
    org_id: &str,
    iam_organization_id: Option<uuid::Uuid>,
) -> Result<(), AppError> {
    let existing = sqlx::query_scalar::<_, Option<uuid::Uuid>>(
        "SELECT iam_organization_id FROM briefcase.organizations \
          WHERE org_id = briefcase.current_org_id()",
    )
    .fetch_optional(&mut **transaction)
    .await?
    .flatten();
    if existing.is_some() && iam_organization_id.is_some() && existing != iam_organization_id {
        return Err(AppError::validation("iam_organization_binding_mismatch"));
    }
    sqlx::query(
        "INSERT INTO briefcase.organizations ( \
             org_id, iam_organization_id, iam_version, lifecycle_status \
         ) VALUES ($1, $2, 0, 'active') \
         ON CONFLICT (org_id) DO UPDATE \
             SET iam_organization_id = COALESCE( \
                 briefcase.organizations.iam_organization_id, EXCLUDED.iam_organization_id \
             )",
    )
    .bind(org_id)
    .bind(iam_organization_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn claim_receipt(
    transaction: &mut Transaction<'_, Postgres>,
    webhook: &VerifiedIamWebhook,
    org_id: &str,
) -> Result<ReceiptClaim, AppError> {
    let event = &webhook.event;
    let event_id = event.event_id.to_string();
    let inserted = sqlx::query_scalar::<_, i32>(
        "INSERT INTO briefcase.webhook_receipts ( \
                source, event_id, org_id, event_type, aggregate_type, aggregate_id, \
                aggregate_version, signature_timestamp, payload_sha256 \
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
         ON CONFLICT (source, event_id) DO NOTHING \
         RETURNING 1",
    )
    .bind(SOURCE)
    .bind(&event_id)
    .bind(org_id)
    .bind(&event.event_type)
    .bind(&event.aggregate_type)
    .bind(event.aggregate_id.to_string())
    .bind(i64::try_from(event.aggregate_version).map_err(decode_error)?)
    .bind(webhook.signature_timestamp)
    .bind(webhook.payload_sha256.as_slice())
    .fetch_optional(&mut **transaction)
    .await?;
    if inserted.is_some() {
        return Ok(ReceiptClaim::Inserted);
    }

    let existing_payload = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT payload_sha256 FROM briefcase.webhook_receipts \
          WHERE source = $1 AND event_id = $2 \
            AND org_id = briefcase.current_org_id()",
    )
    .bind(SOURCE)
    .bind(&event_id)
    .fetch_optional(&mut **transaction)
    .await?;
    if receipt_is_exact_duplicate(existing_payload.as_deref(), &webhook.payload_sha256) {
        Ok(ReceiptClaim::Duplicate)
    } else {
        // This single opaque outcome covers both altered bytes and a globally
        // colliding event ID hidden by tenant RLS.
        Err(AppError::conflict("iam_webhook_event_id_conflict"))
    }
}

fn receipt_is_exact_duplicate(existing_payload: Option<&[u8]>, signed_payload: &[u8]) -> bool {
    existing_payload.is_some_and(|existing| existing == signed_payload)
}

fn is_known_application_event(event_type: &str) -> bool {
    matches!(
        event_type,
        "carbon.updated.v1"
            | "session.logout.v1"
            | "organization.membership.created.v1"
            | "organization.membership.reactivated.v1"
            | "organization.membership.removed.v1"
            | "organization.silicon.created.v1"
            | "organization.silicon.removed.v1"
            | "organization.membership.updated.v1"
            | "organization.membership.authorization_updated.v1"
            | "organization.ownership_transferred.v1"
            | "organization.admin.promoted.v1"
            | "organization.admin.demoted.v1"
            | "organization.silicon.updated.v1"
            | "organization.tag_updated.v1"
            | "organization.tag_archived.v1"
            | "organization.trust.default_updated.v1"
            | "organization.trust.rule_created.v1"
            | "organization.trust.rule_updated.v1"
            | "organization.trust.rule_archived.v1"
            | "organization.updated.v1"
            | "organization.silicon.credential_rotated.v1"
    )
}

async fn apply_projection(
    transaction: &mut Transaction<'_, Postgres>,
    webhook: &VerifiedIamWebhook,
    _version: i64,
) -> Result<WebhookApplyOutcome, AppError> {
    let current = webhook
        .event
        .data
        .get("current")
        .unwrap_or(&webhook.event.data);
    let mut outcome = WebhookApplyOutcome::Ignored;

    if let Some(organization) = current.get("organization") {
        outcome = merge_outcome(
            outcome,
            apply_organization(transaction, organization).await?,
        );
    }
    if let Some(resource) = current.get("resource")
        && resource.get("type").and_then(Value::as_str) == Some("organization_tag")
    {
        outcome = merge_outcome(outcome, apply_tag(transaction, resource).await?);
    }
    if let Some(members) = current.get("members").and_then(Value::as_array) {
        for member in members {
            outcome = merge_outcome(outcome, apply_member(transaction, member).await?);
        }
    }
    Ok(outcome)
}

async fn apply_organization(
    transaction: &mut Transaction<'_, Postgres>,
    organization: &Value,
) -> Result<WebhookApplyOutcome, AppError> {
    let version = required_positive_version(organization, "invalid_iam_organization_payload")?;
    let status = if organization.get("authorization").and_then(Value::as_str) == Some("removed") {
        "removed"
    } else {
        optional_string(organization, &["status"]).unwrap_or("active")
    };
    let status = if status == "deleted" {
        "removed"
    } else {
        status
    };
    validate_lifecycle(status, &["active", "suspended", "removed"])?;
    let result = sqlx::query(
        "INSERT INTO briefcase.organizations (org_id, iam_version, lifecycle_status) \
         VALUES (briefcase.current_org_id(), $2, $1) \
         ON CONFLICT (org_id) DO UPDATE \
             SET lifecycle_status = EXCLUDED.lifecycle_status, \
                 iam_version = EXCLUDED.iam_version \
           WHERE briefcase.organizations.iam_version < EXCLUDED.iam_version",
    )
    .bind(status)
    .bind(version)
    .execute(&mut **transaction)
    .await?;
    Ok(applied_or_stale(result.rows_affected()))
}

async fn apply_member(
    transaction: &mut Transaction<'_, Postgres>,
    member: &Value,
) -> Result<WebhookApplyOutcome, AppError> {
    let resource = member.get("resource").unwrap_or(member);
    let principal_id = required_uuid(resource, "principal_id", "invalid_iam_membership_payload")?;
    let membership_id = required_uuid(resource, "id", "invalid_iam_membership_payload")?;
    let version = required_positive_version(resource, "invalid_iam_membership_payload")?;
    let actor_type = required_string(
        resource,
        &["principal_type", "actor_type"],
        "invalid_iam_membership_payload",
    )?;
    validate_lifecycle(actor_type, &["carbon", "silicon"])?;
    let status = if member.get("authorization").and_then(Value::as_str) == Some("removed") {
        "removed"
    } else {
        optional_string(resource, &["status"]).unwrap_or("active")
    };
    validate_lifecycle(status, &["active", "removed"])?;
    let authorization_epoch = member
        .pointer("/membership/authorization_epoch")
        .and_then(Value::as_i64)
        .filter(|epoch| *epoch > 0);
    if status == "active" && authorization_epoch.is_none() {
        return Err(AppError::validation("invalid_iam_membership_payload"));
    }
    let tags = member.pointer("/membership/tags").and_then(Value::as_array);
    if status == "active" && tags.is_none() {
        return Err(AppError::validation("invalid_iam_membership_payload"));
    }

    let actor_id = member
        .pointer("/principal/public_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let existing_actor = if actor_id.is_none() {
        sqlx::query_as::<_, (String, String)>(
            "SELECT actor_type, actor_id FROM briefcase.organization_members \
              WHERE org_id = briefcase.current_org_id() \
                AND principal_id = $1 AND membership_id = $2",
        )
        .bind(principal_id)
        .bind(membership_id)
        .fetch_optional(&mut **transaction)
        .await?
    } else {
        None
    };
    let actor_id = actor_id.or_else(|| {
        existing_actor
            .as_ref()
            .map(|(_, actor_id)| actor_id.as_str())
    });
    // A before-only authorization tombstone can legitimately omit the public
    // profile. If Briefcase never held that member, there is no local authority
    // to revoke and the tombstone is already satisfied.
    if status == "removed" && actor_id.is_none() {
        return Ok(WebhookApplyOutcome::Applied);
    }
    let actor_id =
        actor_id.ok_or_else(|| AppError::validation("invalid_iam_membership_payload"))?;
    if existing_actor
        .as_ref()
        .is_some_and(|(existing_type, _)| existing_type != actor_type)
    {
        return Err(AppError::validation("invalid_iam_membership_payload"));
    }

    let role = member.pointer("/roles/org_role").and_then(Value::as_str);
    if let Some(role) = role {
        validate_lifecycle(role, &["owner", "admin", "member"])?;
    }
    let projection = MemberProjection {
        actor_type,
        actor_id,
        principal_id,
        membership_id,
        authorization_epoch,
        role,
        status,
        version,
    };
    let updated = upsert_member(transaction, &projection).await?;
    if updated == 0 {
        return Ok(WebhookApplyOutcome::Stale);
    }

    if status == "removed" {
        delete_member_tags(transaction, actor_type, actor_id).await?;
    } else if let Some(tags) = tags {
        replace_member_tags(transaction, actor_type, actor_id, tags, version).await?;
    }
    Ok(WebhookApplyOutcome::Applied)
}

async fn upsert_member(
    transaction: &mut Transaction<'_, Postgres>,
    member: &MemberProjection<'_>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO briefcase.organization_members ( \
                org_id, actor_type, actor_id, principal_id, membership_id, \
                authorization_epoch, org_role, membership_status, iam_version \
         ) VALUES (briefcase.current_org_id(), $1, $2, $3, $4, $5, \
                   COALESCE($6, 'member'), $7, $8) \
         ON CONFLICT (org_id, actor_type, actor_id) DO UPDATE \
             SET principal_id = EXCLUDED.principal_id, \
                 membership_id = EXCLUDED.membership_id, \
                 authorization_epoch = COALESCE($5, organization_members.authorization_epoch), \
                 org_role = COALESCE($6, 'member'), \
                 membership_status = EXCLUDED.membership_status, \
                 iam_version = EXCLUDED.iam_version \
           WHERE organization_members.iam_version < EXCLUDED.iam_version",
    )
    .bind(member.actor_type)
    .bind(member.actor_id)
    .bind(member.principal_id)
    .bind(member.membership_id)
    .bind(member.authorization_epoch)
    .bind(member.role)
    .bind(member.status)
    .bind(member.version)
    .execute(&mut **transaction)
    .await?;
    Ok(result.rows_affected())
}

async fn apply_tag(
    transaction: &mut Transaction<'_, Postgres>,
    tag: &Value,
) -> Result<WebhookApplyOutcome, AppError> {
    let tag_id = required_string(tag, &["id"], "invalid_iam_tag_payload")?;
    let version = required_positive_version(tag, "invalid_iam_tag_payload")?;
    let name = optional_string(tag, &["name", "tag"]);
    let status = if tag.get("authorization").and_then(Value::as_str) == Some("removed") {
        "removed"
    } else {
        optional_string(tag, &["status"]).unwrap_or("active")
    };
    let status = if status == "archived" {
        "removed"
    } else {
        status
    };
    validate_lifecycle(status, &["active", "removed"])?;
    if status == "active" && name.is_none() {
        return Err(AppError::validation("invalid_iam_tag_payload"));
    }

    let result = sqlx::query(
        "INSERT INTO briefcase.organization_tags ( \
                org_id, tag_id, name, lifecycle_status, iam_version \
         ) VALUES (briefcase.current_org_id(), $1, COALESCE($2, $1), $3, $4) \
         ON CONFLICT (org_id, tag_id) DO UPDATE \
             SET name = COALESCE($2, organization_tags.name), \
                 lifecycle_status = EXCLUDED.lifecycle_status, \
                 iam_version = EXCLUDED.iam_version \
           WHERE organization_tags.iam_version < EXCLUDED.iam_version",
    )
    .bind(tag_id)
    .bind(name)
    .bind(status)
    .bind(version)
    .execute(&mut **transaction)
    .await?;
    Ok(applied_or_stale(result.rows_affected()))
}

async fn replace_member_tags(
    transaction: &mut Transaction<'_, Postgres>,
    actor_type: &str,
    actor_id: &str,
    tags: &[Value],
    version: i64,
) -> Result<(), AppError> {
    delete_member_tags(transaction, actor_type, actor_id).await?;
    for tag in tags {
        let (tag_id, name) = tag_fields(tag)?;
        sqlx::query(
            "INSERT INTO briefcase.organization_tags ( \
                    org_id, tag_id, name, lifecycle_status, iam_version \
             ) VALUES (briefcase.current_org_id(), $1, $2, 'active', 0) \
             ON CONFLICT (org_id, tag_id) DO NOTHING",
        )
        .bind(tag_id)
        .bind(name)
        .execute(&mut **transaction)
        .await?;
        sqlx::query(
            "INSERT INTO briefcase.organization_member_tags ( \
                    org_id, actor_type, actor_id, tag_id, iam_version \
             ) VALUES (briefcase.current_org_id(), $1, $2, $3, $4)",
        )
        .bind(actor_type)
        .bind(actor_id)
        .bind(tag_id)
        .bind(version)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn delete_member_tags(
    transaction: &mut Transaction<'_, Postgres>,
    actor_type: &str,
    actor_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM briefcase.organization_member_tags \
          WHERE org_id = briefcase.current_org_id() \
            AND actor_type = $1 AND actor_id = $2",
    )
    .bind(actor_type)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn finish_receipt(
    transaction: &mut Transaction<'_, Postgres>,
    event_id: &str,
    outcome: WebhookApplyOutcome,
) -> Result<(), sqlx::Error> {
    let status = match outcome {
        WebhookApplyOutcome::Applied => "processed",
        WebhookApplyOutcome::Stale | WebhookApplyOutcome::Ignored => "ignored",
        WebhookApplyOutcome::Duplicate => return Ok(()),
    };
    sqlx::query(
        "UPDATE briefcase.webhook_receipts \
            SET status = $2, processed_at = clock_timestamp() \
          WHERE source = $1 AND event_id = $3 \
            AND org_id = briefcase.current_org_id()",
    )
    .bind(SOURCE)
    .bind(status)
    .bind(event_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn merge_outcome(current: WebhookApplyOutcome, next: WebhookApplyOutcome) -> WebhookApplyOutcome {
    if matches!(current, WebhookApplyOutcome::Applied)
        || matches!(next, WebhookApplyOutcome::Applied)
    {
        WebhookApplyOutcome::Applied
    } else if matches!(current, WebhookApplyOutcome::Stale)
        || matches!(next, WebhookApplyOutcome::Stale)
    {
        WebhookApplyOutcome::Stale
    } else {
        WebhookApplyOutcome::Ignored
    }
}

fn required_uuid(value: &Value, key: &str, code: &'static str) -> Result<uuid::Uuid, AppError> {
    required_string(value, &[key], code)?
        .parse()
        .map_err(|_| AppError::validation(code))
}

fn required_positive_version(value: &Value, code: &'static str) -> Result<i64, AppError> {
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|version| i64::try_from(version).ok())
        .filter(|version| *version > 0)
        .ok_or_else(|| AppError::validation(code))?;
    Ok(version)
}

fn tag_fields(value: &Value) -> Result<(&str, &str), AppError> {
    if let Some(tag) = value.as_str() {
        return Ok((tag, tag));
    }
    let tag_id = required_string(value, &["tag_id", "id", "name"], "invalid_iam_tag_payload")?;
    let name = optional_string(value, &["name", "tag"]).unwrap_or(tag_id);
    Ok((tag_id, name))
}

fn required_string<'a>(
    value: &'a Value,
    keys: &[&str],
    code: &'static str,
) -> Result<&'a str, AppError> {
    optional_string(value, keys)
        .filter(|candidate| !candidate.is_empty())
        .ok_or_else(|| AppError::validation(code))
}

fn optional_string<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

fn validate_lifecycle(value: &str, allowed: &[&str]) -> Result<(), AppError> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(AppError::validation("invalid_iam_projection_value"))
    }
}

fn applied_or_stale(rows_affected: u64) -> WebhookApplyOutcome {
    if rows_affected == 0 {
        WebhookApplyOutcome::Stale
    } else {
        WebhookApplyOutcome::Applied
    }
}

fn decode_error(error: std::num::TryFromIntError) -> sqlx::Error {
    sqlx::Error::Decode(Box::new(error))
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU32, time::Duration};

    use secrecy::SecretString;
    use serde_json::json;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use crate::{
        application::webhook::{
            IamWebhookEvent, IamWebhookRepository as _, VerifiedIamWebhook, WebhookApplyOutcome,
        },
        config::DatabaseSettings,
        domain::actor::OrganizationId,
        infrastructure::postgres::{PostgresRepository, connect, migrate},
    };

    use super::{
        is_known_application_event, receipt_is_exact_duplicate, required_positive_version,
        storage_organization_id,
    };

    #[test]
    fn test_environment_organization_ids_are_namespaced() {
        let environment_id = Uuid::parse_str("01990a9d-86f1-7000-8000-000000000099")
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        assert_eq!(storage_organization_id("tos", None), "tos");
        assert_eq!(
            storage_organization_id("tos", Some(environment_id)),
            "01990a9d-86f1-7000-8000-000000000099:tos"
        );
    }

    #[test]
    fn receipt_deduplication_requires_the_exact_signed_payload() {
        let expected = [7_u8; 32];
        let altered = [8_u8; 32];

        assert!(receipt_is_exact_duplicate(Some(&expected), &expected));
        assert!(!receipt_is_exact_duplicate(Some(&altered), &expected));
        assert!(!receipt_is_exact_duplicate(None, &expected));
    }

    #[test]
    fn membership_projection_requires_a_positive_resource_version() {
        let payload = json!({
            "version": 4
        });
        assert_eq!(required_positive_version(&payload, "invalid").ok(), Some(4));
        assert!(required_positive_version(&json!({"version": 0}), "invalid").is_err());
    }

    #[test]
    fn new_event_types_are_not_projected_by_shape_alone() {
        assert!(is_known_application_event(
            "organization.membership.authorization_updated.v1"
        ));
        assert!(!is_known_application_event("organization.future_event.v1"));
    }

    #[tokio::test]
    async fn membership_snapshot_bootstraps_organization_and_member() -> anyhow::Result<()> {
        let Ok(database_url) = std::env::var("BRIEFCASE_TEST_CONTROL_DATABASE_URL") else {
            eprintln!("skipping: BRIEFCASE_TEST_CONTROL_DATABASE_URL is not set");
            return Ok(());
        };
        let pool = connect(
            &DatabaseSettings {
                url: SecretString::from(database_url),
                max_connections: NonZeroU32::new(2).unwrap_or(NonZeroU32::MIN),
                min_connections: 0,
                acquire_timeout: Duration::from_secs(10),
                statement_timeout: Duration::from_secs(30),
            },
            "briefcase-webhook-bootstrap-test",
        )
        .await?;
        migrate(&pool).await?;

        let suffix = Uuid::new_v4().simple().to_string();
        let org_id = format!("wh-{suffix}");
        let actor_id = format!("carbon:{suffix}");
        let organization_id = Uuid::now_v7();
        let membership_id = Uuid::now_v7();
        let principal_id = Uuid::now_v7();
        let event = IamWebhookEvent {
            event_id: Uuid::now_v7(),
            spec_version: "1.0".to_owned(),
            schema_version: 1,
            aggregate_version: 1,
            aggregate_id: membership_id,
            aggregate_type: "membership".to_owned(),
            event_type: "organization.membership.updated.v1".to_owned(),
            organization_id: Some(organization_id),
            org_id: Some(OrganizationId::new(org_id.clone())?),
            occurred_at: OffsetDateTime::now_utc(),
            data: json!({
                "current": {
                    "organization": {
                        "org_id": org_id,
                        "version": 3,
                        "status": "active"
                    },
                    "members": [{
                        "resource": {
                            "id": membership_id,
                            "principal_id": principal_id,
                            "principal_type": "carbon",
                            "status": "active",
                            "version": 2
                        },
                        "principal": {"public_id": actor_id},
                        "membership": {"authorization_epoch": 1, "tags": []},
                        "roles": {"org_role": "owner"}
                    }]
                }
            }),
        };
        let webhook = VerifiedIamWebhook::new(event, OffsetDateTime::now_utc(), [7_u8; 32], None);
        let repository = PostgresRepository::new(pool.clone());
        let result = repository.apply_iam_event(&webhook, None).await;
        let projected = sqlx::query_as::<_, (i64, i64)>(
            "SELECT organization.iam_version, count(member.actor_id) \
               FROM briefcase.organizations AS organization \
               LEFT JOIN briefcase.organization_members AS member \
                 ON member.org_id = organization.org_id \
              WHERE organization.org_id = $1 \
              GROUP BY organization.iam_version",
        )
        .bind(&org_id)
        .fetch_optional(&pool)
        .await?;
        sqlx::query("DELETE FROM briefcase.organizations WHERE org_id = $1")
            .bind(&org_id)
            .execute(&pool)
            .await?;

        assert_eq!(result?, WebhookApplyOutcome::Applied);
        assert_eq!(projected, Some((3, 1)));
        Ok(())
    }
}
