//! Browser access workflows; authorization remains in the Briefcase API.
use crate::{App, Result, session};
use axum::{Json, extract::State, http::HeaderMap};

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
