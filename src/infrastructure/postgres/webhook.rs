//! Transactional persistence for verified IAM webhook projections.

use async_trait::async_trait;
use serde_json::Value;
use sqlx::{Postgres, Transaction};

use crate::{
    application::webhook::{IamWebhookRepository, VerifiedIamWebhook, WebhookApplyOutcome},
    error::AppError,
};

use super::{PostgresRepository, begin_projection_transaction, roots};

const SOURCE: &str = "silicon-iam";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReceiptClaim {
    Inserted,
    Duplicate,
}

#[async_trait]
impl IamWebhookRepository for PostgresRepository {
    async fn apply_iam_event(
        &self,
        webhook: &VerifiedIamWebhook,
    ) -> Result<WebhookApplyOutcome, AppError> {
        let event = &webhook.event;
        let version = i64::try_from(event.aggregate_version)
            .map_err(|_| AppError::validation("invalid_iam_aggregate_version"))?;
        if version < 1 {
            return Err(AppError::validation("invalid_iam_aggregate_version"));
        }

        let event_id = event.event_id.to_string();
        let mut transaction =
            begin_projection_transaction(self.pool(), event.org_id.as_str(), &event_id).await?;

        roots::lock_organization_reconciliation(&mut transaction).await?;
        ensure_organization(&mut transaction, event.org_id.as_str()).await?;
        if claim_receipt(&mut transaction, webhook).await? == ReceiptClaim::Duplicate {
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

async fn ensure_organization(
    transaction: &mut Transaction<'_, Postgres>,
    org_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO briefcase.organizations (org_id, iam_version, lifecycle_status) \
         VALUES ($1, 0, 'active') \
         ON CONFLICT (org_id) DO NOTHING",
    )
    .bind(org_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn claim_receipt(
    transaction: &mut Transaction<'_, Postgres>,
    webhook: &VerifiedIamWebhook,
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
    .bind(event.org_id.as_str())
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

async fn apply_projection(
    transaction: &mut Transaction<'_, Postgres>,
    webhook: &VerifiedIamWebhook,
    version: i64,
) -> Result<WebhookApplyOutcome, AppError> {
    let event = &webhook.event;
    match event.event_type.as_str() {
        "organization.created.v1" | "organization.updated.v1" | "organization.removed.v1" => {
            apply_organization(transaction, &event.event_type, &event.data, version).await
        }
        "organization.membership.created.v1"
        | "organization.membership.updated.v1"
        | "organization.membership.removed.v1" => {
            apply_member(transaction, &event.event_type, &event.data, version).await
        }
        "organization.tag.created.v1"
        | "organization.tag.updated.v1"
        | "organization.tag.removed.v1" => {
            apply_tag(transaction, &event.event_type, &event.data, version).await
        }
        "organization.membership.tags.updated.v1" | "organization.membership.tags.changed.v1" => {
            apply_member_tags(transaction, &event.data, version).await
        }
        _ => Ok(WebhookApplyOutcome::Ignored),
    }
}

async fn apply_organization(
    transaction: &mut Transaction<'_, Postgres>,
    event_type: &str,
    data: &Value,
    version: i64,
) -> Result<WebhookApplyOutcome, AppError> {
    let status = if event_type.ends_with("removed.v1") {
        "removed"
    } else {
        optional_string(
            projection(data, "organization"),
            &["lifecycle_status", "status"],
        )
        .unwrap_or("active")
    };
    validate_lifecycle(status, &["active", "suspended", "removed"])?;
    let result = sqlx::query(
        "UPDATE briefcase.organizations \
            SET lifecycle_status = $1, iam_version = $2 \
          WHERE org_id = briefcase.current_org_id() AND iam_version < $2",
    )
    .bind(status)
    .bind(version)
    .execute(&mut **transaction)
    .await?;
    Ok(applied_or_stale(result.rows_affected()))
}

async fn apply_member(
    transaction: &mut Transaction<'_, Postgres>,
    event_type: &str,
    data: &Value,
    version: i64,
) -> Result<WebhookApplyOutcome, AppError> {
    let member = projection(data, "membership");
    let (actor_type, actor_id) = actor_fields(member)?;
    validate_lifecycle(actor_type, &["carbon", "silicon"])?;
    let role = optional_string(member, &["org_role", "role"]);
    if let Some(role) = role {
        validate_lifecycle(role, &["owner", "admin", "member"])?;
    }
    let status = if event_type.ends_with("removed.v1") {
        Some("removed")
    } else {
        optional_string(member, &["membership_status", "status"])
    };
    if let Some(status) = status {
        validate_lifecycle(status, &["active", "suspended", "removed"])?;
    }

    let updated = upsert_member(transaction, actor_type, actor_id, role, status, version).await?;
    if updated == 0 {
        return Ok(WebhookApplyOutcome::Stale);
    }

    if status == Some("removed") {
        delete_member_tags(transaction, actor_type, actor_id).await?;
    } else if let Some(tags) = member.get("tags").and_then(Value::as_array) {
        replace_member_tags(transaction, actor_type, actor_id, tags, version).await?;
    }
    Ok(WebhookApplyOutcome::Applied)
}

async fn upsert_member(
    transaction: &mut Transaction<'_, Postgres>,
    actor_type: &str,
    actor_id: &str,
    role: Option<&str>,
    status: Option<&str>,
    version: i64,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO briefcase.organization_members ( \
                org_id, actor_type, actor_id, org_role, membership_status, iam_version \
         ) VALUES (briefcase.current_org_id(), $1, $2, COALESCE($3, 'member'), \
                   COALESCE($4, 'active'), $5) \
         ON CONFLICT (org_id, actor_type, actor_id) DO UPDATE \
             SET org_role = COALESCE($3, organization_members.org_role), \
                 membership_status = COALESCE($4, organization_members.membership_status), \
                 iam_version = EXCLUDED.iam_version \
           WHERE organization_members.iam_version < EXCLUDED.iam_version",
    )
    .bind(actor_type)
    .bind(actor_id)
    .bind(role)
    .bind(status)
    .bind(version)
    .execute(&mut **transaction)
    .await?;
    Ok(result.rows_affected())
}

async fn apply_tag(
    transaction: &mut Transaction<'_, Postgres>,
    event_type: &str,
    data: &Value,
    version: i64,
) -> Result<WebhookApplyOutcome, AppError> {
    let tag = projection(data, "tag");
    let tag_id = required_string(tag, &["tag_id", "id"], "invalid_iam_tag_payload")?;
    let name = optional_string(tag, &["name", "tag"]);
    let status = if event_type.ends_with("removed.v1") {
        "removed"
    } else {
        optional_string(tag, &["lifecycle_status", "status"]).unwrap_or("active")
    };
    validate_lifecycle(status, &["active", "removed"])?;

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

async fn apply_member_tags(
    transaction: &mut Transaction<'_, Postgres>,
    data: &Value,
    version: i64,
) -> Result<WebhookApplyOutcome, AppError> {
    let member = projection(data, "membership");
    let (actor_type, actor_id) = actor_fields(member)?;
    validate_lifecycle(actor_type, &["carbon", "silicon"])?;
    let tags = member
        .get("tags")
        .or_else(|| data.get("tags"))
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::validation("invalid_iam_membership_tags_payload"))?;

    let updated = upsert_member(
        transaction,
        actor_type,
        actor_id,
        optional_string(member, &["org_role", "role"]),
        optional_string(member, &["membership_status", "status"]),
        version,
    )
    .await?;
    if updated == 0 {
        return Ok(WebhookApplyOutcome::Stale);
    }
    replace_member_tags(transaction, actor_type, actor_id, tags, version).await?;
    Ok(WebhookApplyOutcome::Applied)
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

fn projection<'a>(data: &'a Value, key: &str) -> &'a Value {
    data.get(key).unwrap_or(data)
}

fn actor_fields(value: &Value) -> Result<(&str, &str), AppError> {
    let actor = value.get("actor").unwrap_or(value);
    let actor_type = required_string(
        actor,
        &["type", "actor_type", "principal_kind"],
        "invalid_iam_membership_payload",
    )?;
    let actor_id = required_string(
        actor,
        &["public_id", "actor_id", "id", "principal_id"],
        "invalid_iam_membership_payload",
    )?;
    Ok((actor_type, actor_id))
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
    use serde_json::json;

    use super::{actor_fields, receipt_is_exact_duplicate};

    #[test]
    fn receipt_deduplication_requires_the_exact_signed_payload() {
        let expected = [7_u8; 32];
        let altered = [8_u8; 32];

        assert!(receipt_is_exact_duplicate(Some(&expected), &expected));
        assert!(!receipt_is_exact_duplicate(Some(&altered), &expected));
        assert!(!receipt_is_exact_duplicate(None, &expected));
    }

    #[test]
    fn membership_projection_prefers_the_public_actor_handle() {
        let payload = json!({
            "actor": {
                "type": "carbon",
                "public_id": "carbon-public-handle",
                "id": "018f0000-0000-7000-8000-000000000000",
                "principal_id": "018f0000-0000-7000-8000-000000000001"
            }
        });
        let fields = actor_fields(&payload);

        match fields {
            Ok((actor_type, actor_id)) => {
                assert_eq!(actor_type, "carbon");
                assert_eq!(actor_id, "carbon-public-handle");
            }
            Err(error) => panic!("actor projection must decode: {error}"),
        }
    }
}
