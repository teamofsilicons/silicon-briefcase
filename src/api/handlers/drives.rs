//! The drives a caller can open, what others shared with it, and who may
//! share with a Silicon.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    application::{context::ExecutionContext, service::MetadataServiceError},
    domain::{account::AccountUuid, actor::ActorKind},
    error::AppError,
    infrastructure::postgres::accounts::AccountRecord,
};

use super::{
    super::{
        auth::Action, cursor, drives, dto::PageQuery, extract, mapping::metadata_error,
        state::AppState, validation,
    },
    accounts::private_json,
};

/// Lists the drives the caller may open with the owner's rights: its own,
/// and (for a Carbon) the drive of every Silicon it looks after.
pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let context = extract::authenticate(&state, &headers, Action::ListDrives, "drives").await?;
    let account = context
        .authorization()
        .account()
        .cloned()
        .ok_or(AppError::Internal {
            category: "account_context",
        })?;
    let mut owners = vec![(account.uuid.as_str().to_owned(), "own")];
    if account.kind == ActorKind::Carbon {
        state.accounts.learn_unknown_custodians().await;
        for silicon in state.accounts.custodial_silicons(&account.uuid).await? {
            owners.push((silicon.uuid, "custodial"));
        }
    }
    let mut items = Vec::with_capacity(owners.len());
    for (owner, relation) in owners {
        let entered = match drives::enter(&state, context.clone(), &owner).await {
            Ok(entered) => entered,
            Err(AppError::NotFound) => continue,
            Err(error) => return Err(error),
        };
        if !entered.authorization().has_owner_rights() {
            // The cached custodian changed since the list was read.
            continue;
        }
        let root = match extract::scoped(&entered, state.metadata.drive_root(&entered)).await {
            Ok(root) => root,
            Err(MetadataServiceError::NotFound) => continue,
            Err(error) => return Err(metadata_error(error)),
        };
        let record = state
            .accounts
            .record(
                &AccountUuid::new(owner.clone()).map_err(|_| AppError::Internal {
                    category: "account_uuid",
                })?,
            )
            .await?;
        let external_id = state
            .accounts
            .store()
            .storage_external_id(&entered)
            .await
            .map_err(|_| AppError::Internal {
                category: "storage_configuration",
            })?
            .unwrap_or_else(|| crate::infrastructure::s3::organization_storage_external_id(&owner));
        items.push(json!({
            "owner": account_json(&record),
            "relation": relation,
            "root_id": root.entry.id.as_uuid(),
            "storage_external_id": external_id,
            "permanent_url": state
                .mapper_for([owner.as_str()])
                .await?
                .permanent_url(&owner, &root.entry.path)?,
        }));
    }
    Ok(private_json(json!({ "items": items })))
}

#[derive(Deserialize, Serialize)]
struct SharedCursor {
    shared_at: OffsetDateTime,
    entry_id: Uuid,
}

#[derive(Deserialize)]
pub(crate) struct SharedQuery {
    cursor: Option<String>,
    limit: Option<u16>,
    from: Option<String>,
    contacts: Option<String>,
    min_trust: Option<u8>,
    q: Option<String>,
}
impl SharedQuery {
    fn validate(&self) -> Result<(), AppError> {
        extract::validation(validation::page(&PageQuery {
            cursor: self.cursor.clone(),
            limit: self.limit,
        }))?;
        if self
            .contacts
            .as_deref()
            .is_some_and(|v| !matches!(v, "saved" | "unsaved"))
            || self.min_trust == Some(0)
            || (self.contacts.as_deref() == Some("unsaved") && self.min_trust.is_some())
            || self.q.as_ref().is_some_and(|q| q.len() > 200)
        {
            return Err(AppError::validation("invalid_shared_filter"));
        }
        Ok(())
    }
}
#[derive(sqlx::FromRow)]
struct SharedPersonEntry {
    drive: String,
    entry_id: Uuid,
    shared_at: OffsetDateTime,
    sharers: Vec<String>,
}

async fn shared_rows(
    state: &AppState,
    context: &ExecutionContext,
    token: &str,
    query: &SharedQuery,
    before: Option<(OffsetDateTime, Uuid)>,
    limit: u16,
) -> Result<Vec<SharedPersonEntry>, AppError> {
    let caller = context.authorization().actor();
    let from = match query.from.as_deref() {
        Some(id) => Some(state.accounts.resolve(id).await?.uuid),
        None => None,
    };
    let contacts = if query.contacts.is_some() || query.min_trust.is_some() {
        Some(
            super::people::load_contacts(state, super::people::caller(context)?, token)
                .await?
                .into_iter()
                .filter(|c| {
                    c["trust"].as_u64().unwrap_or(0) >= u64::from(query.min_trust.unwrap_or(1))
                })
                .filter_map(|c| c["uuid"].as_str().map(str::to_owned))
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };
    sqlx::query_as::<_, SharedPersonEntry>(
        "SELECT drive,entry_id,shared_at,sharers FROM briefcase.shared_with_people($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"
    ).bind(match caller.kind() { ActorKind::Carbon => "carbon", ActorKind::Silicon => "silicon" })
        .bind(caller.id().as_str()).bind(&context.organizations().groups)
        .bind(before.map(|b| b.0)).bind(before.map(|b| b.1)).bind(i32::from(limit)+1)
        .bind(from).bind(contacts).bind(query.contacts.as_deref() == Some("unsaved"))
        .bind(query.q.as_deref().filter(|q| !q.trim().is_empty())).fetch_all(&state.database).await
        .map_err(|_| AppError::Internal { category: "shared_lookup" })
}

/// Lists what other accounts shared with the caller, newest share first,
/// each with the account whose drive holds it (clients group by owner).
pub(crate) async fn shared(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<SharedQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    let query = extract::query(query)?;
    query.validate()?;
    let before = query
        .cursor
        .as_deref()
        .map(decode_cursor)
        .transpose()?
        .map(|cursor| (cursor.shared_at, cursor.entry_id));
    let limit = query.limit.unwrap_or(100);
    let (context, token) =
        extract::authenticate_with_token(&state, &headers, Action::ListShared, "shared").await?;
    let rows = shared_rows(
        &state,
        &context,
        secrecy::ExposeSecret::expose_secret(&token),
        &query,
        before,
        limit,
    )
    .await?;
    let has_more = rows.len() > usize::from(limit);
    let mapper = state
        .mapper_for(rows.iter().map(|row| row.drive.clone()))
        .await?;
    let mut items = Vec::with_capacity(usize::from(limit));
    let mut last = None;
    // A page usually spans few drives: resolve each drive (the caller's
    // relation to its owner, and the owner's record) once per request.
    let mut resolved: HashMap<String, Option<(ExecutionContext, AccountRecord)>> = HashMap::new();
    for row in rows.into_iter().take(usize::from(limit)) {
        last = Some((row.shared_at, row.entry_id));
        if !resolved.contains_key(&row.drive) {
            let drive = match drives::enter(&state, context.clone(), &row.drive).await {
                Ok(entered) => {
                    let owner = state
                        .accounts
                        .record(&AccountUuid::new(row.drive.clone()).map_err(|_| {
                            AppError::Internal {
                                category: "account_uuid",
                            }
                        })?)
                        .await?;
                    Some((entered, owner))
                }
                Err(AppError::NotFound) => None,
                Err(error) => return Err(error),
            };
            resolved.insert(row.drive.clone(), drive);
        }
        let Some(Some((entered, owner))) = resolved.get(&row.drive) else {
            continue;
        };
        let entry_id = extract::entry_id(row.entry_id)?;
        let item =
            match extract::scoped(entered, state.metadata.visible_entry(entered, entry_id)).await {
                Ok(item) => item,
                // The share ended or the entry moved to the bin since the lookup.
                Err(MetadataServiceError::NotFound) => continue,
                Err(error) => return Err(metadata_error(error)),
            };
        let mut shared_by = Vec::new();
        for uuid in row.sharers {
            if let Some(account) =
                state
                    .accounts
                    .store()
                    .find(&uuid)
                    .await
                    .map_err(|_| AppError::Internal {
                        category: "account_cache",
                    })?
            {
                shared_by.push(account_json(&account));
            }
        }
        items.push(json!({
            "owner": account_json(owner),
            "shared_at": row.shared_at,
            "shared_by": shared_by,
            "entry": mapper.entry_item(&row.drive, item)?,
        }));
    }
    let next_cursor = if has_more {
        last.map(|(shared_at, entry_id)| {
            encode_cursor(&SharedCursor {
                shared_at,
                entry_id,
            })
        })
        .transpose()?
    } else {
        None
    };
    Ok(private_json(
        json!({ "items": items, "next_cursor": next_cursor }),
    ))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IncomingShares {
    allow_from_anyone: bool,
}

pub(crate) async fn incoming_shares(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<String>,
) -> Result<Response, AppError> {
    let context = extract::authenticate(
        &state,
        &headers,
        Action::ReadShareAllowances,
        "incoming-shares",
    )
    .await?;
    let (_, record) = managed_account(&state, &context, &account, false).await?;
    Ok(private_json(json!(IncomingShares {
        allow_from_anyone: record.accept_shares_from_anyone
    })))
}

pub(crate) async fn set_incoming_shares(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<String>,
    Json(preference): Json<IncomingShares>,
) -> Result<Response, AppError> {
    let context = extract::authenticate(
        &state,
        &headers,
        Action::ManageShareAllowances,
        "incoming-shares",
    )
    .await?;
    let (recipient, _) = managed_account(&state, &context, &account, false).await?;
    let saved = state
        .accounts
        .store()
        .set_incoming_shares(
            recipient.as_str(),
            caller_uuid(&context)?.as_str(),
            preference.allow_from_anyone,
        )
        .await
        .map_err(|_| AppError::Internal {
            category: "incoming_shares",
        })?
        .ok_or_else(|| {
            AppError::refused(
                "share_settings_forbidden",
                "Only the account or its current custodian may change this preference.",
            )
        })?;
    Ok(private_json(json!(IncomingShares {
        allow_from_anyone: saved
    })))
}

#[derive(Serialize)]
struct Allowance {
    account: Value,
    #[serde(with = "time::serde::rfc3339")]
    allowed_at: OffsetDateTime,
}

/// Lists explicit allowances recorded through the legacy Silicon API.
pub(crate) async fn allowances(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(silicon): Path<String>,
) -> Result<Response, AppError> {
    let context = extract::authenticate(
        &state,
        &headers,
        Action::ReadShareAllowances,
        "share-allowances",
    )
    .await?;
    let silicon = managed_account(&state, &context, &silicon, true).await?;
    let items = state
        .accounts
        .allowed_sharers(&silicon.0)
        .await?
        .into_iter()
        .map(|(account, allowed_at)| Allowance {
            account: account_json(&account),
            allowed_at,
        })
        .collect::<Vec<_>>();
    Ok(private_json(json!({
        "silicon": account_json(&silicon.1),
        "items": items,
    })))
}

/// Records an idempotent legacy allowance for a Silicon that accepts shares from anyone.
pub(crate) async fn allow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((silicon, account)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let context = extract::authenticate(
        &state,
        &headers,
        Action::ManageShareAllowances,
        "share-allowances",
    )
    .await?;
    let silicon = managed_account(&state, &context, &silicon, true).await?;
    if !silicon.1.accept_shares_from_anyone {
        return Err(AppError::refused(
            "incoming_shares_restricted",
            "This Silicon restricts incoming shares. Save the sender in its Silicon Contacts address book.",
        ));
    }
    let allowed = state.accounts.resolve(&account).await?;
    let allowed_uuid = AccountUuid::new(allowed.uuid).map_err(|_| AppError::Internal {
        category: "account_uuid",
    })?;
    if allowed_uuid == silicon.0 {
        return Err(AppError::unprocessable(
            "allowance_self",
            "A Silicon always reaches its own files; there is nothing to allow.",
        ));
    }
    let by = caller_uuid(&context)?;
    let created = state
        .accounts
        .allow_sharer(&silicon.0, &allowed_uuid, &by)
        .await?;
    Ok(if created {
        StatusCode::CREATED
    } else {
        StatusCode::NO_CONTENT
    })
}

/// Removes a legacy allowance record for the Silicon.
pub(crate) async fn disallow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((silicon, account)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let context = extract::authenticate(
        &state,
        &headers,
        Action::ManageShareAllowances,
        "share-allowances",
    )
    .await?;
    let silicon = managed_account(&state, &context, &silicon, true).await?;
    let allowed = state.accounts.resolve(&account).await?;
    let allowed_uuid = AccountUuid::new(allowed.uuid).map_err(|_| AppError::Internal {
        category: "account_uuid",
    })?;
    if state
        .accounts
        .disallow_sharer(&silicon.0, &allowed_uuid)
        .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

/// Resolves a settings target and verifies the caller is its owner or custodian.
/// The legacy allowance endpoints require a Silicon target.
async fn managed_account(
    state: &AppState,
    context: &ExecutionContext,
    named: &str,
    silicon_only: bool,
) -> Result<(AccountUuid, AccountRecord), AppError> {
    let record = state.accounts.resolve(named).await?;
    if silicon_only && record.kind != ActorKind::Silicon {
        return Err(AppError::unprocessable(
            "not_a_silicon",
            format!("{named} is a Carbon. Use its incoming share settings."),
        ));
    }
    let owner = AccountUuid::new(record.uuid.clone()).map_err(|_| AppError::Internal {
        category: "account_uuid",
    })?;
    let caller = caller_uuid(context)?;
    if !state.accounts.manages_account(&caller, &owner).await? {
        return Err(AppError::refused(
            if record.kind == ActorKind::Silicon {
                "not_silicons_custodian"
            } else {
                "share_settings_forbidden"
            },
            format!("Only {named} or its custodian can manage its incoming share settings."),
        ));
    }
    Ok((owner, record))
}

fn caller_uuid(context: &ExecutionContext) -> Result<AccountUuid, AppError> {
    context
        .authorization()
        .account()
        .map(|account| account.uuid.clone())
        .ok_or(AppError::Internal {
            category: "account_context",
        })
}

pub(super) fn account_json(record: &AccountRecord) -> Value {
    json!({
        "uuid": record.uuid,
        "id": record.public_id,
        "kind": match record.kind {
            ActorKind::Carbon => "carbon",
            ActorKind::Silicon => "silicon",
        },
        "display_name": record.display_name,
        "pfp_url": record.pfp_url,
    })
}

fn encode_cursor(cursor: &SharedCursor) -> Result<String, AppError> {
    serde_json::to_vec(cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| AppError::Internal {
            category: "shared_cursor",
        })
}

fn decode_cursor(value: &str) -> Result<SharedCursor, AppError> {
    cursor::validate_opaque(value).map_err(|_| AppError::bad_request("invalid_cursor"))?;
    URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| AppError::bad_request("invalid_cursor"))
}
