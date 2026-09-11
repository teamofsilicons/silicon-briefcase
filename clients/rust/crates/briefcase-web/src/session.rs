use crate::{App, Failure, Result, bad};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use briefcase_client::{Client, Config, EnvironmentKey, IdempotencyKey, SessionTokens};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use uuid::Uuid;

pub(crate) struct LoginFlow {
    return_to: String,
    deadline: Instant,
    operation_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Start {
    return_to: Option<String>,
}
pub(crate) async fn start(State(app): State<App>, Json(input): Json<Start>) -> Result<Response> {
    let return_to = input.return_to.unwrap_or_else(|| "/".into());
    let target = url::Url::parse(&format!("{}{return_to}", app.origin))
        .map_err(|_| bad("Invalid return path"))?;
    if return_to.len() > 4096
        || (return_to != "/" && !return_to.starts_with("/org/"))
        || target.origin().ascii_serialization() != app.origin
        || target.path() != return_to
        || target.query().is_some()
        || target.fragment().is_some()
    {
        return Err(bad("Invalid return path"));
    }
    let nonce = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let mut flows = app.logins.lock().await;
    flows.retain(|_, flow| flow.deadline > Instant::now());
    if flows.len() >= 256 {
        return Err(Failure(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many pending sign-ins. Try again shortly.".into(),
        ));
    }
    // IAM preserves the callback's query parameters when appending its SLT.
    // The stable endpoint matches the published Briefcase URL; a per-attempt
    // state value still binds the returned token to this browser's login flow.
    let callback = format!("{}/auth/callback?state={nonce}", app.origin);
    let mut url = url::Url::parse("https://auth.iam.teamofsilicons.com/login")
        .map_err(|_| bad("Invalid IAM sign-in URL"))?;
    url.query_pairs_mut()
        .append_pair("app_id", "tos>briefcase")
        .append_pair("redirect_uri", &callback);
    flows.insert(
        nonce.clone(),
        LoginFlow {
            return_to,
            deadline: Instant::now() + Duration::from_secs(600),
            operation_id: Uuid::new_v4(),
        },
    );
    let cookie = HeaderValue::from_str(&format!(
        "{}-login={nonce}; Path=/; HttpOnly; SameSite=Lax; Max-Age=600{}",
        app.cookie,
        if app.secure { "; Secure" } else { "" }
    ))
    .map_err(|_| bad("Invalid login cookie"))?;
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(json!({"redirect_url":url.as_str()})),
    )
        .into_response())
}
#[derive(Deserialize)]
pub(crate) struct Callback {
    slt: String,
    state: String,
}
pub(crate) async fn callback(
    State(app): State<App>,
    headers: HeaderMap,
    input: std::result::Result<
        axum::extract::Query<Callback>,
        axum::extract::rejection::QueryRejection,
    >,
) -> Response {
    let result = match input {
        Ok(axum::extract::Query(input)) => finish_callback(&app, &headers, input).await,
        Err(_) => Err(unauthenticated()),
    };
    match result {
        Ok(mut response) => {
            *response.status_mut() = StatusCode::SEE_OTHER;
            *response.body_mut() = axum::body::Body::empty();
            response
        }
        Err(_error) => (
            StatusCode::SEE_OTHER,
            [(header::LOCATION, "/?signin_error=1")],
        )
            .into_response(),
    }
}
async fn finish_callback(app: &App, headers: &HeaderMap, input: Callback) -> Result<Response> {
    let nonce = input.state.as_str();
    if nonce.len() != 64 || input.slt.len() > 256 {
        return Err(unauthenticated());
    }
    let expected = format!("{}-login", app.cookie);
    let cookies: Vec<_> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|v| v.trim().split_once('='))
        .filter(|(k, _)| *k == expected)
        .collect();
    if cookies.len() != 1 || cookies[0].1 != nonce {
        return Err(unauthenticated());
    }
    let flows = app.logins.lock().await;
    let flow = flows
        .get(nonce)
        .filter(|f| f.deadline > Instant::now())
        .ok_or_else(unauthenticated)?;
    let input = Login {
        org: None,
        slt: input.slt,
        test_key: None,
        operation_id: flow.operation_id,
    };
    let return_to = flow.return_to.clone();
    drop(flows);
    let mut response = login(State(app.clone()), Json(input)).await?;
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&return_to).map_err(|_| bad("Invalid return path"))?,
    );
    // Retain the bounded flow until expiry so a lost callback response can use
    // the same IAM exchange identity and recover the existing browser session.
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{}-login=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
            app.cookie,
            if app.secure { "; Secure" } else { "" }
        ))
        .map_err(|_| bad("Invalid login cookie"))?,
    );
    Ok(response)
}

pub(crate) struct Session {
    auth_config: Config,
    config: Config,
    tokens: SessionTokens,
    expires: Instant,
    deadline: Instant,
    refresh_key: IdempotencyKey,
    org: Option<String>,
    organizations: Vec<String>,
    testing: bool,
    rejected: bool,
    test_environment: Option<briefcase_client::TestingEnvironment>,
    test_sessions: std::collections::HashMap<Uuid, String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Login {
    org: Option<String>,
    slt: String,
    test_key: Option<String>,
    operation_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectOrganization {
    org: String,
}

fn view(session: &Session) -> Value {
    json!({"authenticated":true,"org":session.org,"organizations":session.organizations,"actor":session.tokens.actor,"testing":session.testing,"test_environment":session.test_environment.as_ref().map(|e| json!({"id":e.id,"name":e.name}))})
}
fn cookie(app: &App, id: &str, age: u32) -> Result<HeaderValue> {
    HeaderValue::from_str(&format!(
        "{}={id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={age}{}",
        app.cookie,
        if app.secure { "; Secure" } else { "" }
    ))
    .map_err(|_| bad("Invalid session"))
}
fn identifier(app: &App, headers: &HeaderMap) -> Option<String> {
    let mut found = None;
    for line in headers.get_all(header::COOKIE) {
        for item in line.to_str().ok()?.split(';') {
            if let Some((name, value)) = item.trim().split_once('=')
                && name == app.cookie
            {
                if found.is_some()
                    || value.len() != 64
                    || !value.bytes().all(|v| v.is_ascii_hexdigit())
                {
                    return None;
                }
                found = Some(value.to_owned());
            }
        }
    }
    found
}
async fn production_session(app: &App, headers: &HeaderMap) -> Result<Arc<Mutex<Session>>> {
    let id = identifier(app, headers).ok_or_else(unauthenticated)?;
    let session = app
        .sessions
        .lock()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(unauthenticated)?;
    let parent = session.lock().await;
    if parent.deadline <= Instant::now() || parent.rejected || parent.testing {
        return Err(unauthenticated());
    }
    drop(parent);
    Ok(session)
}
fn selected_environment(headers: &HeaderMap) -> Result<Option<Uuid>> {
    let values: Vec<_> = headers.get_all("x-briefcase-environment").iter().collect();
    match values.as_slice() {
        [] => Ok(None),
        [value] => Ok(Some(
            value
                .to_str()
                .ok()
                .and_then(|v| Uuid::parse_str(v).ok())
                .filter(|v| !v.is_nil())
                .ok_or_else(|| bad("Invalid testing environment."))?,
        )),
        _ => Err(bad("Ambiguous testing environment.")),
    }
}
async fn lookup(app: &App, headers: &HeaderMap) -> Result<Arc<Mutex<Session>>> {
    let parent = production_session(app, headers).await?;
    let Some(environment) = selected_environment(headers)? else {
        return Ok(parent);
    };
    let child = parent
        .lock()
        .await
        .test_sessions
        .get(&environment)
        .cloned()
        .ok_or_else(unauthenticated)?;
    let session = app
        .sessions
        .lock()
        .await
        .get(&child)
        .cloned()
        .ok_or_else(unauthenticated)?;
    let matches = session
        .lock()
        .await
        .test_environment
        .as_ref()
        .is_some_and(|e| e.id == environment);
    if !matches {
        return Err(unauthenticated());
    }
    Ok(session)
}
fn unauthenticated() -> Failure {
    Failure(
        StatusCode::UNAUTHORIZED,
        "Sign in to Briefcase to continue.".into(),
    )
}
pub(crate) async fn login(State(app): State<App>, Json(input): Json<Login>) -> Result<Response> {
    let (id, value) = establish(&app, input).await?;
    Ok((
        [(header::SET_COOKIE, cookie(&app, &id, 28800)?)],
        Json(value),
    )
        .into_response())
}
async fn establish(app: &App, input: Login) -> Result<(String, Value)> {
    if input.operation_id.is_nil()
        || input
            .org
            .as_ref()
            .is_some_and(|org| org.is_empty() || org.len() > 128 || org.trim() != org)
        || (input.org.is_none() && input.test_key.is_some())
        || input.slt.len() > 256
    {
        return Err(bad("Invalid IAM sign-in request."));
    }
    let _flight = app.login_lock.try_lock().map_err(|_| {
        Failure(
            StatusCode::TOO_MANY_REQUESTS,
            "Another sign-in is being processed. Please retry.".into(),
        )
    })?;
    let mut hash = Sha256::new();
    hash.update(app.login_salt.as_bytes());
    hash.update(serde_json::to_vec(&input).map_err(|_| bad("Invalid login"))?);
    let id = format!("{:x}", hash.finalize());
    let previous = {
        let mut sessions = app.sessions.lock().await;
        sessions.retain(|_, v| {
            v.try_lock()
                .map_or(true, |s| s.deadline > Instant::now() && !s.rejected)
        });
        if sessions.len() >= 1024 {
            return Err(Failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "Browser sessions are at capacity. Try again later.".into(),
            ));
        }
        sessions.get(&id).cloned()
    };
    if let Some(previous) = previous {
        let previous = previous.lock().await;
        return Ok((id, view(&previous)));
    }
    let mut config = Config::for_sign_in(&app.upstream)?.with_auto_update(false);
    let testing = input.test_key.is_some();
    if let Some(key) = input.test_key {
        config = config
            .with_environment(EnvironmentKey::new(key)?)
            .with_organization(
                input
                    .org
                    .clone()
                    .ok_or_else(|| bad("Choose a testing organization."))?,
            )?;
    }
    let client = match Client::connect(config.clone()).await {
        Ok(client) => client,
        Err(error) => {
            return Err(error.into());
        }
    };
    let idempotency_key = IdempotencyKey::new(format!("browser-login-{}", input.operation_id))?;
    let tokens = match client
        .login_with_slt_with_key(&input.slt, &idempotency_key)
        .await
    {
        Ok(tokens) => tokens,
        Err(error) => {
            return Err(error.into());
        }
    };
    // Only live IAM consent snapshots confer workspace authority. A legacy
    // token's org_id must never restore a revoked or migrated grant.
    let organizations = tokens.organizations.clone();
    let session_org = input
        .org
        .clone()
        .filter(|org| organizations.contains(org))
        .or_else(|| (organizations.len() == 1).then(|| organizations[0].clone()));
    let auth_config = config.clone();
    let config = session_org
        .as_deref()
        .map(|org| config.clone().with_organization(org))
        .transpose()?
        .unwrap_or_else(|| config.clone());
    let session = Session {
        auth_config,
        expires: Instant::now() + Duration::from_secs(tokens.expires_in.min(86400)),
        deadline: Instant::now() + Duration::from_secs(28800),
        config,
        tokens,
        refresh_key: IdempotencyKey::random(),
        org: session_org,
        organizations,
        testing,
        rejected: false,
        test_environment: None,
        test_sessions: Default::default(),
    };
    let response = view(&session);
    app.sessions
        .lock()
        .await
        .insert(id.clone(), Arc::new(Mutex::new(session)));
    Ok((id, response))
}

pub(crate) async fn select(
    State(app): State<App>,
    headers: HeaderMap,
    body: std::result::Result<Json<SelectOrganization>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>> {
    let Json(input) = body.map_err(|_| bad("Invalid workspace selection."))?;
    if input.org.is_empty() || input.org.len() > 128 || input.org.trim() != input.org {
        return Err(bad("Invalid workspace selection."));
    }
    let session = lookup(&app, &headers).await?;
    let mut session = session.lock().await;
    if session.deadline <= Instant::now() || session.rejected {
        return Err(unauthenticated());
    }
    refresh_if_needed(&mut session).await?;
    if !session.organizations.iter().any(|org| org == &input.org) {
        return Err(Failure(
            StatusCode::FORBIDDEN,
            "That workspace is not available in this IAM session.".into(),
        ));
    }
    session.config = session.auth_config.clone().with_organization(&input.org)?;
    session.org = Some(input.org);
    Ok(Json(view(&session)))
}

pub(crate) async fn client(app: &App, headers: &HeaderMap) -> Result<Client> {
    let client = authenticated_client(app, headers).await?;
    if headers
        .get("x-briefcase-organization")
        .is_some_and(|value| value.to_str().ok() != Some(client.organization()))
    {
        return Err(Failure(
            StatusCode::CONFLICT,
            "The workspace changed in another tab. Reload before continuing.".into(),
        ));
    }
    if client.organization().is_empty() {
        return Err(Failure(
            StatusCode::FORBIDDEN,
            "Choose an authorised workspace before accessing files.".into(),
        ));
    }
    Ok(client)
}
async fn refresh_if_needed(session: &mut Session) -> Result<()> {
    if session.expires <= Instant::now() + Duration::from_secs(30) {
        let client = Client::new_unchecked(session.auth_config.clone())?;
        let result = client
            .refresh_session_with_key(&session.tokens.refresh_token, &session.refresh_key)
            .await;
        match result {
            Ok(tokens) => {
                session.expires =
                    Instant::now() + Duration::from_secs(tokens.expires_in.min(86400));
                session.organizations = tokens.organizations.clone();
                if session
                    .org
                    .as_ref()
                    .is_some_and(|org| !session.organizations.iter().any(|item| item == org))
                {
                    session.org = None;
                    session.config = session.auth_config.clone();
                } else if let Some(org) = session.org.as_deref() {
                    session.config = session.auth_config.clone().with_organization(org)?;
                }
                session.tokens = tokens;
                session.refresh_key = IdempotencyKey::random();
            }
            Err(error) => {
                if error.is_unauthenticated() {
                    session.rejected = true;
                }
                return Err(error.into());
            }
        }
    }
    Ok(())
}
async fn authenticated_client(app: &App, headers: &HeaderMap) -> Result<Client> {
    let session = lookup(app, headers).await?;
    let mut session = session.lock().await;
    if session.deadline <= Instant::now() || session.rejected {
        return Err(unauthenticated());
    }
    refresh_if_needed(&mut session).await?;
    Ok(Client::new_unchecked(
        session
            .config
            .clone()
            .with_token(session.tokens.access_token.clone()),
    )?)
}
pub(crate) async fn status(State(app): State<App>, headers: HeaderMap) -> Result<Json<Value>> {
    if identifier(&app, &headers).is_none() {
        return Ok(Json(json!({"authenticated":false})));
    }
    let session = lookup(&app, &headers).await?;
    let mut session = session.lock().await;
    if session.deadline <= Instant::now() || session.rejected {
        return Err(unauthenticated());
    }
    refresh_if_needed(&mut session).await?;
    Ok(Json(view(&session)))
}
pub(crate) async fn logout(State(app): State<App>, headers: HeaderMap) -> Result<Response> {
    if let Some(environment) = selected_environment(&headers)? {
        let parent = production_session(&app, &headers).await?;
        if let Some(id) = parent.lock().await.test_sessions.remove(&environment) {
            app.sessions.lock().await.remove(&id);
        }
        return Ok(Json(json!({"authenticated":false})).into_response());
    }
    if let Some(id) = identifier(&app, &headers) {
        app.sessions.lock().await.remove(&id);
    }
    Ok((
        [(header::SET_COOKIE, cookie(&app, "", 0)?)],
        Json(json!({"authenticated":false})),
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnterTest {
    slt: Option<String>,
    operation_id: Uuid,
}
pub(crate) async fn enter_test(
    State(app): State<App>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    Json(input): Json<EnterTest>,
) -> Result<Json<Value>> {
    if selected_environment(&headers)?.is_some() {
        return Err(bad(
            "Return to production before choosing another testing environment.",
        ));
    }
    let parent = production_session(&app, &headers).await?;
    let management = client(&app, &headers).await?;
    let environment = management.testing_environment(id).await?;
    if environment.status != briefcase_client::TestingEnvironmentStatus::Active
        || environment.org_id != management.organization()
    {
        return Err(bad(
            "This testing environment is not active in your organization.",
        ));
    }
    // Root retrieval is separately authorized; the key never leaves the gateway.
    let key = management.testing_environment_key(id).await?;
    if input.slt.as_deref().is_none_or(str::is_empty) {
        let mut selected = headers.clone();
        selected.insert(
            "x-briefcase-environment",
            HeaderValue::from_str(&id.to_string()).map_err(|_| bad("Invalid environment"))?,
        );
        let saved = lookup(&app, &selected).await?;
        let mut saved = saved.lock().await;
        if saved.deadline <= Instant::now()
            || saved.rejected
            || !saved.test_environment.as_ref().is_some_and(|e| {
                e.version == environment.version && e.key_generation == key.key_generation
            })
        {
            return Err(unauthenticated());
        }
        refresh_if_needed(&mut saved).await?;
        if !Client::new_unchecked(
            saved
                .config
                .clone()
                .with_token(saved.tokens.access_token.clone()),
        )?
        .login_status()
        .await?
        .authenticated
        {
            return Err(unauthenticated());
        }
        return Ok(Json(view(&saved)));
    }
    let (child_id, _) = establish(
        &app,
        Login {
            org: Some(environment.org_id.clone()),
            slt: input.slt.unwrap_or_default(),
            test_key: Some(key.key.expose_secret().to_owned()),
            operation_id: input.operation_id,
        },
    )
    .await?;
    let child = app
        .sessions
        .lock()
        .await
        .get(&child_id)
        .cloned()
        .ok_or_else(unauthenticated)?;
    let mut parent = parent.lock().await;
    if parent.deadline <= Instant::now() || parent.rejected {
        return Err(unauthenticated());
    }
    let mut child = child.lock().await;
    if child.org.as_deref() != Some(environment.org_id.as_str()) {
        return Err(bad("The test account has no access to this organization."));
    }
    child.deadline = child.deadline.min(parent.deadline);
    child.test_environment = Some(environment);
    parent.test_sessions.insert(id, child_id);
    Ok(Json(view(&child)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App {
            upstream: "http://127.0.0.1:9/api/v1/".into(),
            origin: "http://localhost:4318".into(),
            cookie: "briefcase_dev",
            secure: false,
            sessions: Default::default(),
            login_lock: Default::default(),
            login_salt: "unit-test".into(),
            uploads: Arc::new(tokio::sync::Semaphore::new(4)),
            staging: crate::staging::Storage::from_env().unwrap(),
            logins: Default::default(),
            document_csp: HeaderValue::from_static("default-src 'self'"),
        }
    }
    fn session(test: Option<Uuid>) -> Session {
        let mut config = Config::new("http://127.0.0.1:9/api/v1/", "tos").unwrap();
        if test.is_some() {
            config = config.with_environment(EnvironmentKey::new("a".repeat(32)).unwrap());
        }
        let environment = test.map(|id| serde_json::from_value(json!({
            "id":id,"org_id":"tos","name":"Demo","description":null,"status":"active",
            "iam_environment_id":Uuid::new_v4(),"iam_app_id":"tos>briefcase",
            "created_by":{"type":"carbon","id":"saket"},"key_generation":1,"key_rotated_at":null,
            "last_activity_at":"2026-09-11T00:00:00Z","cleaned_at":null,"deleted_at":null,"purge_after":null,
            "version":1,"created_at":"2026-09-11T00:00:00Z","updated_at":"2026-09-11T00:00:00Z"
        })).unwrap());
        Session {
            auth_config: config.clone(), config, tokens: serde_json::from_value(json!({
                "access_token":"test-token","refresh_token":"test-refresh","token_type":"Bearer","expires_in":3600,
                "scope":"profile","actor":{"principal_id":Uuid::new_v4(),"type":"carbon","public_id":"saket"},
                "org_id":"tos","organizations":["tos"]
            })).unwrap(), expires: Instant::now()+Duration::from_secs(3600),deadline:Instant::now()+Duration::from_secs(3600),
            refresh_key:IdempotencyKey::random(),org:Some("tos".into()),organizations:vec!["tos".into()],
            testing:test.is_some(),rejected:false,test_environment:environment,test_sessions:Default::default(),
        }
    }
    fn headers(cookie: &str, environment: Option<Uuid>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("briefcase_dev={cookie}")).unwrap(),
        );
        if let Some(id) = environment {
            headers.insert(
                "x-briefcase-environment",
                HeaderValue::from_str(&id.to_string()).unwrap(),
            );
        }
        headers
    }
    #[tokio::test]
    async fn test_views_are_bound_to_the_parent_browser_and_keep_production_separate() {
        let app = app();
        let id = Uuid::new_v4();
        let owner = "a".repeat(64);
        let outsider = "b".repeat(64);
        let mut parent = session(None);
        parent.test_sessions.insert(id, "child".into());
        let child = Arc::new(Mutex::new(session(Some(id))));
        app.sessions.lock().await.extend([
            (owner.clone(), Arc::new(Mutex::new(parent))),
            (outsider.clone(), Arc::new(Mutex::new(session(None)))),
            ("child".into(), child.clone()),
        ]);
        let selected = lookup(&app, &headers(&owner, Some(id))).await.ok().unwrap();
        assert!(Arc::ptr_eq(&selected, &child));
        assert!(lookup(&app, &headers(&outsider, Some(id))).await.is_err());
        assert!(
            lookup(&app, &headers(&owner, Some(Uuid::new_v4())))
                .await
                .is_err()
        );
        assert!(
            !lookup(&app, &headers(&owner, None))
                .await
                .ok()
                .unwrap()
                .lock()
                .await
                .testing
        );
        assert!(
            client(&app, &headers(&owner, Some(id)))
                .await
                .ok()
                .unwrap()
                .config()
                .environment()
                .is_some()
        );
        assert!(
            client(&app, &headers(&owner, None))
                .await
                .ok()
                .unwrap()
                .config()
                .environment()
                .is_none()
        );
        child.lock().await.deadline = Instant::now();
        assert!(client(&app, &headers(&owner, Some(id))).await.is_err());
        assert!(client(&app, &headers(&owner, None)).await.is_ok());
    }
    #[tokio::test]
    async fn signing_out_of_test_does_not_sign_out_of_production() {
        let app = app();
        let owner = "a".repeat(64);
        let id = Uuid::new_v4();
        let mut parent = session(None);
        parent.test_sessions.insert(id, "child".into());
        app.sessions.lock().await.extend([
            (owner.clone(), Arc::new(Mutex::new(parent))),
            ("child".into(), Arc::new(Mutex::new(session(Some(id))))),
        ]);
        let response = logout(State(app.clone()), headers(&owner, Some(id)))
            .await
            .ok()
            .unwrap();
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        assert!(lookup(&app, &headers(&owner, None)).await.is_ok());
        assert!(lookup(&app, &headers(&owner, Some(id))).await.is_err());
        app.sessions
            .lock()
            .await
            .get(&owner)
            .unwrap()
            .lock()
            .await
            .deadline = Instant::now();
        assert!(lookup(&app, &headers(&owner, None)).await.is_err());
    }
    #[test]
    fn malformed_or_duplicate_environment_selectors_never_fall_back_to_production() {
        let mut h = HeaderMap::new();
        assert!(selected_environment(&h).ok().unwrap().is_none());
        h.insert(
            "x-briefcase-environment",
            HeaderValue::from_static("invalid"),
        );
        assert!(selected_environment(&h).is_err());
        h.insert(
            "x-briefcase-environment",
            HeaderValue::from_str(&Uuid::new_v4().to_string()).unwrap(),
        );
        assert!(selected_environment(&h).ok().unwrap().is_some());
        h.append(
            "x-briefcase-environment",
            HeaderValue::from_str(&Uuid::new_v4().to_string()).unwrap(),
        );
        assert!(selected_environment(&h).is_err());
    }
}
