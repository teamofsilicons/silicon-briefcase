//! Explicit bug report intake; no telemetry or external issue tracker required.
use super::super::{auth::IamAction, extract, mapping::metadata_error, state::AppState};
use crate::{
    error::AppError,
    infrastructure::postgres::{PostgresRepository, ReportReceipt},
};
use axum::{Json, extract::State, http::HeaderMap};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BugReport {
    message: String,
    #[serde(default)]
    pr: Option<String>,
    #[serde(default)]
    isi: Option<String>,
}

fn validate(body: &BugReport) -> Result<(), AppError> {
    if body.message.trim().is_empty() || body.message.len() > 16_384 {
        return Err(AppError::validation(
            "report_message_must_be_1_to_16384_bytes",
        ));
    }
    if let Some(isi) = &body.isi
        && (isi.trim().is_empty() || isi.len() > 256 || isi.chars().any(char::is_control))
    {
        return Err(AppError::validation("invalid_isi"));
    }
    if let Some(pr) = &body.pr {
        let url = url::Url::parse(pr).map_err(|_| AppError::validation("invalid_report_pr_url"))?;
        let valid = url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url
                .path()
                .strip_prefix("/teamofsilicons/silicon-briefcase/pull/")
                .is_some_and(|id| {
                    !id.is_empty()
                        && id.bytes().all(|b| b.is_ascii_digit())
                        && id.parse::<u64>().is_ok_and(|id| id > 0)
                });
        if !valid {
            return Err(AppError::validation(
                "report_pr_must_reference_briefcase_pull_request",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BugReport>,
) -> Result<Json<ReportReceipt>, AppError> {
    validate(&body)?;
    let org = extract::organization_resource(&headers)?;
    let context = extract::authenticate(&state, &headers, IamAction::SubmitReport, &org).await?;
    let metadata = extract::mutation(&headers, "submit_report", &org, &body, true)?;
    let mut repository = PostgresRepository::new(state.database.clone());
    if let Some(testing) = &state.testing {
        repository = repository.with_test_pool(testing.test_pool().clone());
    }
    let result = Box::pin(extract::scoped(
        &context,
        repository.submit_report(
            &context,
            &body.message,
            body.pr.as_deref(),
            body.isi.as_deref(),
            &metadata,
        ),
    ))
    .await
    .map_err(|error| metadata_error(error.into()))?;
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_accept_optional_pr_and_reject_empty_messages_and_unrelated_urls() {
        let mut report = BugReport {
            message: "Steps to reproduce".into(),
            pr: None,
            isi: None,
        };
        assert!(validate(&report).is_ok());
        report.pr = Some("https://github.com/teamofsilicons/silicon-briefcase/pull/42".into());
        assert!(validate(&report).is_ok());
        for url in [
            "https://github.com/elsewhere/repo/pull/1",
            "https://github.com/teamofsilicons/silicon-briefcase/pull/0",
            "https://token@github.com/teamofsilicons/silicon-briefcase/pull/1",
        ] {
            report.pr = Some(url.into());
            assert!(validate(&report).is_err());
        }
        report.pr = None;
        report.message = " \n".into();
        assert!(validate(&report).is_err());
    }
}
