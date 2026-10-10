//! The drives a caller can open, what others shared with it, and who may
//! share with a Silicon.

use std::collections::HashMap;

use axum::{
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
        items.push(json!({
            "owner": account_json(&record),
            "relation": relation,
            "root_id": root.entry.id.as_uuid(),
            "permanent_url": state.mapper.permanent_url(&owner, &root.entry.path)?,
        }));
    }
    Ok(private_json(json!({ "items": items })))
}

#[derive(Deserialize, Serialize)]
struct SharedCursor {
    shared_at: OffsetDateTime,
    entry_id: Uuid,
}

/// Lists what other accounts shared with the caller, newest share first,
/// each with the account whose drive holds it (clients group by owner).
pub(crate) async fn shared(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    let query = extract::query(query)?;
    extract::validation(validation::page(&query))?;
    let before = query
        .cursor
        .as_deref()
        .map(decode_cursor)
        .transpose()?
        .map(|cursor| (cursor.shared_at, cursor.entry_id));
    let limit = query.limit.unwrap_or(100);
    let context = extract::authenticate(&state, &headers, Action::ListShared, "shared").await?;
    let caller = context.authorization().actor().clone();
    let rows = state
        .content_adapter
        .metadata_repository()
        .shared_with(&caller, before, i32::from(limit) + 1)
        .await
        .map_err(|_| AppError::Internal {
            category: "shared_lookup",
        })?;
    let has_more = rows.len() > usize::from(limit);
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
        items.push(json!({
            "owner": account_json(owner),
            "shared_at": row.shared_at,
            "entry": state.mapper.entry_item(&row.drive, item)?,
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

/// Lists who may share with a Silicon besides its circle. The Silicon itself
/// and its custodian may read it.
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
    let silicon = managed_silicon(&state, &context, &silicon).await?;
    let items = state
        .accounts
        .allowed_sharers(&silicon.0)
        .await?
        .into_iter()
        .map(|(account, allowed_at)| {
            json!({ "account": account_json(&account), "allowed_at": allowed_at })
        })
        .collect::<Vec<Value>>();
    Ok(private_json(json!({
        "silicon": account_json(&silicon.1),
        "items": items,
    })))
}

/// Lets an account share with a Silicon. Only the Silicon or its custodian
/// may allow; allowing again changes nothing.
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
    let silicon = managed_silicon(&state, &context, &silicon).await?;
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

/// Stops letting an account share with a Silicon. Shares it already made
/// stay; the Silicon (or its custodian) can remove those one by one.
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
    let silicon = managed_silicon(&state, &context, &silicon).await?;
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

/// Resolves the Silicon an allow-list request names and checks the caller is
/// that Silicon or its custodian.
async fn managed_silicon(
    state: &AppState,
    context: &ExecutionContext,
    named: &str,
) -> Result<(AccountUuid, AccountRecord), AppError> {
    let record = state.accounts.resolve(named).await?;
    if record.kind != ActorKind::Silicon {
        return Err(AppError::unprocessable(
            "not_a_silicon",
            format!(
                "{named} is a Carbon. Anyone signed in to Briefcase can share with a Carbon; only Silicons keep a list of who may share with them."
            ),
        ));
    }
    let silicon = AccountUuid::new(record.uuid.clone()).map_err(|_| AppError::Internal {
        category: "account_uuid",
    })?;
    let caller = caller_uuid(context)?;
    if !state.accounts.manages_silicon(&caller, &silicon).await? {
        return Err(AppError::refused(
            "not_silicons_custodian",
            format!(
                "Only {named} itself or its custodian can see or change who may share with it."
            ),
        ));
    }
    Ok((silicon, record))
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

fn account_json(record: &AccountRecord) -> Value {
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
