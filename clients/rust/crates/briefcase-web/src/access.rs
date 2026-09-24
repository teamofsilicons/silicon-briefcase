//! Browser access workflows; authorization remains in the Briefcase API.
use crate::{App, Failure, Result, bad, lifetime, session};
use axum::extract::{Path, Query};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use briefcase_client::ExpiryChange;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
pub(crate) struct Page {
    pub cursor: Option<String>,
}
#[derive(Deserialize)]
pub(crate) struct LinkUpdate {
    enabled: bool,
    /// Turns the link on as an expiring link that stops working after this long.
    expires_in_minutes: Option<u32>,
    operation_id: Uuid,
}
#[derive(Deserialize)]
pub(crate) struct InvitationInput {
    #[serde(flatten)]
    invite: briefcase_client::Invite,
    operation_id: Uuid,
}
#[derive(Deserialize)]
pub(crate) struct Mutation {
    operation_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpiryUpdate {
    expires_in_minutes: Option<u32>,
    #[serde(default)]
    permanent: bool,
    operation_id: Uuid,
}
fn key(id: Uuid) -> Result<briefcase_client::IdempotencyKey> {
    if id.is_nil() {
        return Err(crate::bad("Missing operation identity"));
    }
    Ok(briefcase_client::IdempotencyKey::new(id.to_string())?)
}
pub(crate) async fn link(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<briefcase_client::LinkAccess>> {
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .link_access(id)
            .await?,
    ))
}
pub(crate) async fn set_link(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<LinkUpdate>,
) -> Result<Json<briefcase_client::LinkAccess>> {
    let key = key(input.operation_id)?;
    let client = session::client(&app, &headers).await?;
    Ok(Json(match (input.enabled, input.expires_in_minutes) {
        (true, Some(minutes)) => {
            client
                .set_expiring_link_access(id, lifetime(minutes)?, &key)
                .await?
        }
        (false, Some(_)) => return Err(bad("Turn link sharing on to give it an end time.")),
        (enabled, None) => client.set_link_access(id, enabled, &key).await?,
    }))
}
pub(crate) async fn logs(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(page): Query<Page>,
) -> Result<Json<briefcase_client::LogPage>> {
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .logs(id, page.cursor.as_deref())
            .await?,
    ))
}
pub(crate) async fn invitations(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(page): Query<Page>,
) -> Result<Json<briefcase_client::InvitationPage>> {
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .invitations(id, page.cursor.as_deref())
            .await?,
    ))
}
pub(crate) async fn invite(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<InvitationInput>,
) -> Result<Json<briefcase_client::Invitation>> {
    if let Some(minutes) = input.invite.expires_in_minutes {
        lifetime(minutes)?;
    }
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .invite(id, &input.invite, &key(input.operation_id)?)
            .await?,
    ))
}
pub(crate) async fn revoke(
    State(app): State<App>,
    headers: HeaderMap,
    Path((id, grant)): Path<(Uuid, Uuid)>,
    Json(input): Json<Mutation>,
) -> Result<Json<serde_json::Value>> {
    session::client(&app, &headers)
        .await?
        .revoke_invitation(id, grant, &key(input.operation_id)?)
        .await?;
    Ok(Json(serde_json::json!({"revoked":true})))
}
/// Restarts a live expiring share's clock, or makes it permanent.
pub(crate) async fn change_expiring(
    State(app): State<App>,
    headers: HeaderMap,
    Path((id, grant)): Path<(Uuid, Uuid)>,
    Json(input): Json<ExpiryUpdate>,
) -> Result<Json<briefcase_client::Invitation>> {
    let change = match (input.expires_in_minutes, input.permanent) {
        (Some(minutes), false) => ExpiryChange::ExpireIn(lifetime(minutes)?),
        (None, true) => ExpiryChange::Permanent,
        _ => return Err(bad("Choose a new end time or make the share permanent.")),
    };
    let key = key(input.operation_id)?;
    match session::client(&app, &headers)
        .await?
        .change_expiring_share(id, grant, change, &key)
        .await
    {
        Ok(invitation) => Ok(Json(invitation)),
        // Expiry is strict: an ended share is gone rather than editable.
        Err(briefcase_client::Error::Api(error)) if error.status == 404 => Err(Failure(
            StatusCode::NOT_FOUND,
            "This expiring share has already ended.".into(),
        )),
        Err(error) => Err(error.into()),
    }
}

pub(crate) async fn inbox(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<briefcase_client::NotificationInbox>> {
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .notifications()
            .await?,
    ))
}

pub(crate) async fn mark_read(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<briefcase_client::NotificationInbox>> {
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .mark_notifications_read()
            .await?,
    ))
}

#[cfg(test)]
mod expiring_tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path},
    };

    async fn upstream() -> (MockServer, App, HeaderMap) {
        let server = MockServer::start().await;
        let (app, headers) = session::tests::signed_in(&format!("{}/api/v1/", server.uri())).await;
        (server, app, headers)
    }
    fn conflict(code: &str) -> ResponseTemplate {
        ResponseTemplate::new(409).set_body_json(json!({"error":{
            "code":code,"message":"The request conflicts with the current resource state."}}))
    }
    fn invitation(id: Uuid, expires_at: Option<&str>) -> serde_json::Value {
        json!({"id":id,"principal":{"type":"carbon","id":"c:alex"},"access":["read"],
            "inherit":false,"expires_at":expires_at})
    }

    #[tokio::test]
    async fn a_expiring_invitation_carries_its_lifetime_upstream() {
        let (server, app, headers) = upstream().await;
        let (entry, grant, operation) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/entries/{entry}/invitations")))
            .and(header("idempotency-key", operation.to_string().as_str()))
            .and(body_json(
                json!({"principal":{"type":"carbon","id":"c:alex"},
                "access":["read"],"inherit":false,"expires_in_minutes":1440}),
            ))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(invitation(grant, Some("2026-09-25T12:00:00Z"))),
            )
            .expect(1)
            .mount(&server)
            .await;
        let input: InvitationInput = serde_json::from_value(json!({
            "principal":{"type":"carbon","id":"c:alex"},"access":["read"],"inherit":false,
            "expires_in_minutes":1440,"operation_id":operation}))
        .unwrap();
        let result = invite(
            State(app.clone()),
            headers.clone(),
            Path(entry),
            Json(input),
        )
        .await
        .unwrap_or_else(|failure| panic!("{}", failure.1));
        assert_eq!(result.0.expires_at.as_deref(), Some("2026-09-25T12:00:00Z"));

        // Out of range never reaches the API.
        for minutes in [0, 43_201] {
            let input: InvitationInput = serde_json::from_value(json!({
                "principal":{"type":"carbon","id":"c:alex"},"access":["read"],"inherit":false,
                "expires_in_minutes":minutes,"operation_id":Uuid::new_v4()}))
            .unwrap();
            let Err(failure) = invite(
                State(app.clone()),
                headers.clone(),
                Path(entry),
                Json(input),
            )
            .await
            else {
                panic!("accepted {minutes} minutes");
            };
            assert_eq!(failure.0, StatusCode::BAD_REQUEST);
            assert_eq!(failure.1, "Choose a time between 1 minute and 30 days.");
        }
    }

    #[tokio::test]
    async fn a_expiring_invitation_with_more_than_read_is_explained() {
        let (server, app, headers) = upstream().await;
        let entry = Uuid::new_v4();
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/entries/{entry}/invitations")))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({"error":{
                "code":"expiring_share_is_read_only","message":"The request contains invalid data."}})))
            .mount(&server)
            .await;
        let input: InvitationInput = serde_json::from_value(json!({
            "principal":{"type":"tag","id":"design"},"access":["read","update"],"inherit":true,
            "expires_in_minutes":60,"operation_id":Uuid::new_v4()}))
        .unwrap();
        let Err(failure) = invite(State(app), headers, Path(entry), Json(input)).await else {
            panic!("accepted a writable expiring share");
        };
        assert_eq!(failure.0, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            failure
                .1
                .starts_with("An expiring share can only let people view")
        );
    }

    #[tokio::test]
    async fn a_expiring_share_can_be_restarted_or_made_permanent() {
        let (server, app, headers) = upstream().await;
        let (entry, grant) = (Uuid::new_v4(), Uuid::new_v4());
        let route = format!("/api/v1/entries/{entry}/invitations/{grant}");
        Mock::given(method("PATCH"))
            .and(path(route.as_str()))
            .and(body_json(json!({"expires_in_minutes":90})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(invitation(grant, Some("2026-09-24T13:30:00Z"))),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(route.as_str()))
            .and(body_json(json!({"permanent":true})))
            .respond_with(ResponseTemplate::new(200).set_body_json(invitation(grant, None)))
            .expect(1)
            .mount(&server)
            .await;
        let change = |body: serde_json::Value| {
            let (app, headers) = (app.clone(), headers.clone());
            async move {
                change_expiring(
                    State(app),
                    headers,
                    Path((entry, grant)),
                    Json(serde_json::from_value(body).unwrap()),
                )
                .await
            }
        };
        let restarted = change(json!({"expires_in_minutes":90,"operation_id":Uuid::new_v4()}))
            .await
            .unwrap_or_else(|failure| panic!("{}", failure.1));
        assert_eq!(
            restarted.0.expires_at.as_deref(),
            Some("2026-09-24T13:30:00Z")
        );
        let kept = change(json!({"permanent":true,"operation_id":Uuid::new_v4()}))
            .await
            .unwrap_or_else(|failure| panic!("{}", failure.1));
        assert!(kept.0.expires_at.is_none());
        for ambiguous in [
            json!({"operation_id":Uuid::new_v4()}),
            json!({"expires_in_minutes":5,"permanent":true,"operation_id":Uuid::new_v4()}),
            json!({"expires_in_minutes":0,"operation_id":Uuid::new_v4()}),
        ] {
            let Err(failure) = change(ambiguous).await else {
                panic!("accepted an invalid change");
            };
            assert_eq!(failure.0, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn changing_an_ended_or_permanent_share_is_explained() {
        let (server, app, headers) = upstream().await;
        let (entry, ended, permanent) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        Mock::given(method("PATCH"))
            .and(path(format!("/api/v1/entries/{entry}/invitations/{ended}")))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error":{
                "code":"not_found","message":"The requested resource was not found."}})))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!(
                "/api/v1/entries/{entry}/invitations/{permanent}"
            )))
            .respond_with(conflict("not_an_expiring_share"))
            .mount(&server)
            .await;
        for (grant, status, message) in [
            (
                ended,
                StatusCode::NOT_FOUND,
                "This expiring share has already ended.",
            ),
            (
                permanent,
                StatusCode::CONFLICT,
                "This share is permanent, so it has no end time to change.",
            ),
        ] {
            let Err(failure) = change_expiring(
                State(app.clone()),
                headers.clone(),
                Path((entry, grant)),
                Json(
                    serde_json::from_value(
                        json!({"expires_in_minutes":10,"operation_id":Uuid::new_v4()}),
                    )
                    .unwrap(),
                ),
            )
            .await
            else {
                panic!("changed {grant}");
            };
            assert_eq!((failure.0, failure.1.as_str()), (status, message));
        }
    }

    #[tokio::test]
    async fn a_expiring_link_is_turned_on_with_its_lifetime() {
        let (server, app, headers) = upstream().await;
        let entry = Uuid::new_v4();
        let route = format!("/api/v1/entries/{entry}/link-access");
        let link = |expires_at: Option<&str>| {
            json!({"can_manage":true,"enabled":true,"effective":true,"inherited_from":null,
                "url":"https://briefcase.teamofsilicons.com/org/tos/public/a.txt",
                "expires_at":expires_at})
        };
        Mock::given(method("PUT"))
            .and(path(route.as_str()))
            .and(body_json(json!({"enabled":true,"expires_in_minutes":60})))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(link(Some("2026-09-24T13:00:00Z"))),
            )
            .expect(1)
            .mount(&server)
            .await;
        // Enabling again without a lifetime makes an expiring link permanent.
        Mock::given(method("PUT"))
            .and(path(route.as_str()))
            .and(body_json(json!({"enabled":true})))
            .respond_with(ResponseTemplate::new(200).set_body_json(link(None)))
            .expect(1)
            .mount(&server)
            .await;
        let set = |body: serde_json::Value| {
            let (app, headers) = (app.clone(), headers.clone());
            async move {
                set_link(
                    State(app),
                    headers,
                    Path(entry),
                    Json(serde_json::from_value(body).unwrap()),
                )
                .await
            }
        };
        let expiring = set(json!({"enabled":true,"expires_in_minutes":60,
            "operation_id":Uuid::new_v4()}))
        .await
        .unwrap_or_else(|failure| panic!("{}", failure.1));
        assert_eq!(
            expiring.0.expires_at.as_deref(),
            Some("2026-09-24T13:00:00Z")
        );
        let permanent = set(json!({"enabled":true,"operation_id":Uuid::new_v4()}))
            .await
            .unwrap_or_else(|failure| panic!("{}", failure.1));
        assert!(permanent.0.expires_at.is_none());
        for invalid in [
            json!({"enabled":false,"expires_in_minutes":60,"operation_id":Uuid::new_v4()}),
            json!({"enabled":true,"expires_in_minutes":43_201,"operation_id":Uuid::new_v4()}),
        ] {
            let Err(failure) = set(invalid).await else {
                panic!("accepted an invalid link change");
            };
            assert_eq!(failure.0, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn a_expiring_link_over_a_permanent_link_is_explained() {
        let (server, app, headers) = upstream().await;
        let entry = Uuid::new_v4();
        Mock::given(method("PUT"))
            .and(path(format!("/api/v1/entries/{entry}/link-access")))
            .respond_with(conflict("link_already_permanent"))
            .mount(&server)
            .await;
        let Err(failure) = set_link(
            State(app),
            headers,
            Path(entry),
            Json(
                serde_json::from_value(json!({"enabled":true,"expires_in_minutes":60,
                    "operation_id":Uuid::new_v4()}))
                .unwrap(),
            ),
        )
        .await
        else {
            panic!("replaced a permanent link");
        };
        assert_eq!(failure.0, StatusCode::CONFLICT);
        assert!(failure.1.contains("Turn link sharing off first"));
    }
}
