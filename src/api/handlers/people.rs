//! Private invitation history and the caller's consented Contacts integration.
use super::super::{auth::Action, extract, state::AppState};
use super::{accounts::private_json, drives::account_json};
use crate::{application::context::ExecutionContext, error::AppError, infrastructure::contacts};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use reqwest::Method;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use uuid::Uuid;

fn db(_: sqlx::Error) -> AppError {
    AppError::Internal {
        category: "people_store",
    }
}
pub(super) fn caller(context: &ExecutionContext) -> Result<&str, AppError> {
    context
        .authorization()
        .account()
        .map(|a| a.uuid.as_str())
        .ok_or(AppError::Internal {
            category: "account_context",
        })
}
async fn transaction<'a>(
    state: &'a AppState,
    owner: &str,
) -> Result<Transaction<'a, Postgres>, AppError> {
    let mut tx = state.database.begin().await.map_err(db)?;
    sqlx::query("SELECT set_config('briefcase.actor_id',$1,true)")
        .bind(owner)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    Ok(tx)
}
pub(super) async fn connected(state: &AppState, owner: &str) -> Result<bool, AppError> {
    let mut tx = transaction(state, owner).await?;
    let result = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM briefcase.contact_connections WHERE account_uuid=$1)",
    )
    .bind(owner)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(result)
}
pub(super) async fn proof(
    state: &AppState,
    owner: &str,
    token: &str,
    write: bool,
) -> Result<SecretString, AppError> {
    if !connected(state, owner).await? {
        return Err(AppError::refused(
            "contacts_not_connected",
            "Connect Silicon Contacts in Briefcase to use your saved contacts.",
        ));
    }
    state.accounts.gateway().contacts_proof(token, write).await
}
pub(super) fn gateway(state: &AppState) -> Result<&contacts::ContactsGateway, AppError> {
    state.contacts.as_deref().ok_or_else(contacts::unavailable)
}
pub(super) async fn load_contacts(
    state: &AppState,
    owner: &str,
    token: &str,
) -> Result<Vec<Value>, AppError> {
    let proof = proof(state, owner, token, false).await?;
    gateway(state)?.all(proof.expose_secret()).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Connection {
    enabled: bool,
}
pub(crate) async fn connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Connection>,
) -> Result<Response, AppError> {
    let (context, token) =
        extract::authenticate_with_token(&state, &headers, Action::ReadAccount, "contacts").await?;
    let owner = caller(&context)?;
    // The explicit Connect action grants only address-book access, never messages/settings.
    if body.enabled {
        let proof = state
            .accounts
            .gateway()
            .contacts_proof(token.expose_secret(), false)
            .await?;
        gateway(&state)?
            .json(proof.expose_secret(), Method::GET, &["me"], &[], None)
            .await?;
    }
    let mut tx = transaction(&state, owner).await?;
    if body.enabled {
        sqlx::query("INSERT INTO briefcase.contact_connections(account_uuid) VALUES($1) ON CONFLICT DO NOTHING").bind(owner).execute(&mut *tx).await.map_err(db)?;
    } else {
        sqlx::query("DELETE FROM briefcase.contact_connections WHERE account_uuid=$1")
            .bind(owner)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
    }
    tx.commit().await.map_err(db)?;
    Ok(private_json(json!({"connected":body.enabled})))
}

pub(crate) async fn list_contacts(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let (context, token) =
        extract::authenticate_with_token(&state, &headers, Action::ReadAccount, "contacts").await?;
    let owner = caller(&context)?;
    if !connected(&state, owner).await? {
        return Ok(private_json(json!({"connected":false,"items":[]})));
    }
    let items = load_contacts(&state, owner, token.expose_secret()).await?;
    Ok(private_json(json!({"connected":true,"items":items})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SaveContact {
    id: String,
    name: String,
    trust: u8,
}
pub(crate) async fn save_contact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SaveContact>,
) -> Result<Response, AppError> {
    if body.trust == 0
        || body.name.trim().is_empty()
        || body.name.len() > 200
        || body.id.len() > 254
    {
        return Err(AppError::unprocessable(
            "contact_invalid",
            "Enter a name up to 200 bytes and a trust level from 1 to 255.",
        ));
    }
    let (context, token) =
        extract::authenticate_with_token(&state, &headers, Action::ReadAccount, "contacts").await?;
    let owner = caller(&context)?;
    let read = proof(&state, owner, token.expose_secret(), false).await?;
    let gateway = gateway(&state)?;
    let account = gateway
        .json(
            read.expose_secret(),
            Method::GET,
            &["lookup"],
            &[("q", body.id)],
            None,
        )
        .await?;
    let uuid = account["uuid"].as_str().ok_or_else(contacts::unavailable)?;
    if uuid == owner {
        return Err(AppError::unprocessable(
            "contact_self",
            "You cannot add yourself as a contact.",
        ));
    }
    let previous = match gateway
        .json(
            read.expose_secret(),
            Method::GET,
            &["contacts", uuid],
            &[],
            None,
        )
        .await
    {
        Ok(value) => value,
        Err(AppError::NotFound) => Value::Null,
        Err(error) => return Err(error),
    };
    // Contacts PUT/POST replace the snapshot. Retain every field this small editor doesn't edit.
    let mut saved = json!({"id":uuid,"name":body.name.trim(),"trust":body.trust});
    for field in [
        "note",
        "phone",
        "email",
        "photo_id",
        "birthday_month",
        "birthday_day",
        "birthday_year",
    ] {
        if let Some(value) = previous.get(field) {
            saved[field] = value.clone();
        }
    }
    let write = proof(&state, owner, token.expose_secret(), true).await?;
    Ok(private_json(
        gateway
            .json(
                write.expose_secret(),
                Method::POST,
                &["contacts"],
                &[],
                Some(&saved),
            )
            .await?,
    ))
}

pub(crate) async fn photo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let (context, token) =
        extract::authenticate_with_token(&state, &headers, Action::ReadAccount, "contacts-photo")
            .await?;
    let proof = proof(&state, caller(&context)?, token.expose_secret(), false).await?;
    let bytes = gateway(&state)?
        .bytes(
            proof.expose_secret(),
            Method::GET,
            &["photos", &id.to_string()],
            &[],
            None,
        )
        .await?;
    // Only Contacts' decoded/re-encoded PNGs are served. Never relay an upstream URL or cookie.
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(contacts::unavailable());
    }
    Ok((
        StatusCode::OK,
        [
            ("content-type", "image/png"),
            ("cache-control", "private, no-store"),
            ("x-content-type-options", "nosniff"),
        ],
        bytes,
    )
        .into_response())
}

#[derive(sqlx::FromRow, Serialize)]
struct HistoryAccount {
    uuid: String,
    id: String,
    kind: String,
    display_name: Option<String>,
    pfp_url: Option<String>,
    invited: i64,
    received: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    last_invited_at: Option<OffsetDateTime>,
}
pub(crate) async fn recipients(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let (context, token) =
        extract::authenticate_with_token(&state, &headers, Action::ReadAccount, "recipients")
            .await?;
    let owner = caller(&context)?;
    let mut tx = transaction(&state, owner).await?;
    let rows=sqlx::query_as::<_,HistoryAccount>(
        "WITH peers AS (SELECT CASE WHEN sender=$1 THEN recipient ELSE sender END AS uuid, \
         sum(CASE WHEN sender=$1 THEN invites ELSE 0 END)::bigint AS invited, \
         sum(CASE WHEN recipient=$1 THEN invites ELSE 0 END)::bigint AS received, max(last_invited_at) AS last_invited_at \
         FROM briefcase.invite_history WHERE sender=$1 OR recipient=$1 GROUP BY 1) \
         SELECT a.account_uuid AS uuid,a.public_id AS id,a.kind,a.display_name,a.pfp_url,p.invited,p.received,p.last_invited_at \
         FROM peers p JOIN briefcase.accounts a ON a.account_uuid=p.uuid WHERE a.status<>'deleted' AND a.public_id IS NOT NULL")
        .bind(owner).fetch_all(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    let mut items: BTreeMap<String, Value> = rows
        .into_iter()
        .map(|row| (row.uuid.clone(), json!(row)))
        .collect();
    let mut status = "disconnected";
    if connected(&state, owner).await? {
        match load_contacts(&state, owner, token.expose_secret()).await {
            Ok(contacts) => {
                status = "connected";
                for contact in contacts {
                    let Some(uuid) = contact["uuid"].as_str() else {
                        continue;
                    };
                    if uuid == owner || contact["account"]["status"] == "deleted" {
                        continue;
                    }
                    let person=items.entry(uuid.to_owned()).or_insert_with(||json!({"uuid":uuid,"id":contact["account"]["id"],"kind":contact["account"]["kind"],"display_name":contact["account"]["display_name"],"pfp_url":contact["account"]["pfp_url"],"invited":0,"received":0,"last_invited_at":null}));
                    apply_contact(person, &contact);
                }
            }
            Err(_) => status = "unavailable",
        }
    }
    Ok(private_json(
        json!({"items":items.into_values().collect::<Vec<_>>(),"contacts_status":status}),
    ))
}

pub(super) fn apply_contact(person: &mut Value, contact: &Value) {
    person["contact"] =
        json!({"name":contact["name"],"trust":contact["trust"],"photo_id":contact["photo_id"]});
    person["fallback_pfp_url"] = contact["account"]["pfp_url"].clone();
    if contact["name"]
        .as_str()
        .is_some_and(|s| !s.trim().is_empty())
    {
        person["display_name"] = contact["name"].clone();
    }
    if let Some(id) = contact["account"]["id"]
        .as_str()
        .filter(|id| !id.is_empty())
    {
        person["id"] = json!(id);
    }
    if let Some(photo) = contact["photo_id"]
        .as_str()
        .filter(|p| Uuid::parse_str(p).is_ok())
    {
        person["pfp_url"] = json!(format!("/api/v1/contacts/photos/{photo}"));
    } else if contact["account"]["pfp_url"]
        .as_str()
        .is_some_and(|s| !s.is_empty())
    {
        person["pfp_url"] = contact["account"]["pfp_url"].clone();
    }
}

#[derive(Deserialize)]
pub(crate) struct Resolve {
    id: String,
}
pub(crate) async fn resolve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<Resolve>,
) -> Result<Response, AppError> {
    if query.id.len() > 254 {
        return Err(AppError::validation("invalid_account_id"));
    }
    let (context, token) =
        extract::authenticate_with_token(&state, &headers, Action::ReadAccount, "recipient")
            .await?;
    let owner = caller(&context)?;
    let record = state.accounts.resolve(query.id.trim()).await?;
    let mut person = account_json(&record);
    if connected(&state, owner).await? {
        let proof = proof(&state, owner, token.expose_secret(), false).await?;
        let gateway = gateway(&state)?;
        let profile = gateway
            .json(
                proof.expose_secret(),
                Method::GET,
                &["lookup"],
                &[("q", record.uuid.clone())],
                None,
            )
            .await?;
        person["display_name"] = profile["display_name"].clone();
        person["pfp_url"] = profile["pfp_url"].clone();
        match gateway
            .json(
                proof.expose_secret(),
                Method::GET,
                &["contacts", &record.uuid],
                &[],
                None,
            )
            .await
        {
            Ok(contact) => apply_contact(&mut person, &contact),
            Err(AppError::NotFound) => {}
            Err(error) => return Err(error),
        }
    } else if let Some(profile) = state
        .accounts
        .gateway()
        .recipient_profile(&record.uuid)
        .await?
    {
        person["display_name"] = json!(profile.0);
        person["pfp_url"] = json!(profile.1);
    }
    Ok(private_json(person))
}
