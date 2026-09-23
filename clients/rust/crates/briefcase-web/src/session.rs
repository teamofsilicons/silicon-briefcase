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
    time::{Duration, SystemTime},
};
use tokio::sync::Mutex;
use uuid::Uuid;

const SESSION_SECONDS: u32 = 900 * 24 * 60 * 60;

fn storage_failure() -> Failure {
    Failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "The session could not be saved. Retry the same action.".into(),
    )
}

#[derive(Deserialize, Serialize)]
struct SavedSession {
    api: String,
    auth_org: String,
    environment: Option<EnvironmentKey>,
    tokens: SessionTokens,
    expires: SystemTime,
    deadline: SystemTime,
    refresh_key: String,
    refresh_started_at: Option<SystemTime>,
    org: Option<String>,
    organizations: Vec<String>,
    testing: bool,
    rejected: bool,
    test_environment: Option<TestSelection>,
    test_sessions: std::collections::HashMap<Uuid, String>,
}

impl Session {
    fn save(&mut self) -> Result<()> {
        self.durable_dirty = true;
        let Some((storage, id)) = &self.durable else {
            self.durable_dirty = false;
            return Ok(());
        };
        let result = storage
            .write(
                id,
                &SavedSession {
                    api: self.auth_config.api_base().to_string(),
                    auth_org: self.auth_config.organization().to_owned(),
                    environment: self.auth_config.environment().cloned(),
                    tokens: self.tokens.clone(),
                    expires: self.expires,
                    deadline: self.deadline,
                    refresh_key: self.refresh_key.as_str().to_owned(),
                    refresh_started_at: self.refresh_started_at,
                    org: self.org.clone(),
                    organizations: self.organizations.clone(),
                    testing: self.testing,
                    rejected: self.rejected,
                    test_environment: self.test_environment.clone(),
                    test_sessions: self.test_sessions.clone(),
                },
            )
            .map_err(|_| storage_failure());
        if result.is_ok() {
            self.durable_dirty = false;
        }
        result
    }
}

pub(crate) fn restore(
    storage: &Arc<crate::session_store::Storage>,
) -> anyhow::Result<std::collections::HashMap<String, Arc<Mutex<Session>>>> {
    let mut sessions = std::collections::HashMap::new();
    for id in storage.identifiers()? {
        let Some(saved): Option<SavedSession> = storage.read(&id)? else {
            continue;
        };
        if saved.rejected || saved.deadline <= SystemTime::now() {
            continue;
        }
        let mut auth_config = Config::for_sign_in(&saved.api)?.with_auto_update(false);
        if !saved.auth_org.is_empty() {
            auth_config = auth_config.with_organization(&saved.auth_org)?;
        }
        if let Some(environment) = saved.environment {
            auth_config = auth_config.with_environment(environment);
        }
        let config = match &saved.org {
            Some(org) => auth_config.clone().with_organization(org)?,
            None => auth_config.clone(),
        };
        sessions.insert(
            id.clone(),
            Arc::new(Mutex::new(Session {
                auth_config,
                config,
                tokens: saved.tokens,
                expires: saved.expires,
                deadline: saved.deadline,
                refresh_key: IdempotencyKey::new(saved.refresh_key)?,
                refresh_started_at: saved.refresh_started_at,
                durable: Some((storage.clone(), id)),
                durable_dirty: false,
                org: saved.org,
                organizations: saved.organizations,
                testing: saved.testing,
                rejected: saved.rejected,
                test_environment: saved.test_environment,
                test_sessions: saved.test_sessions,
            })),
        );
    }
    Ok(sessions)
}

#[derive(Deserialize, Serialize)]
pub(crate) struct LoginFlow {
    return_to: String,
    deadline: SystemTime,
    operation_id: Uuid,
    telemetry: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Start {
    return_to: Option<String>,
}
pub(crate) async fn start(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Start>,
) -> Result<Response> {
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
    flows.retain(|_, flow| flow.deadline > SystemTime::now());
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
        .append_pair("app_id", "briefcase")
        .append_pair("redirect_uri", &callback);
    flows.insert(
        nonce.clone(),
        LoginFlow {
            return_to,
            deadline: SystemTime::now() + Duration::from_secs(600),
            operation_id: Uuid::new_v4(),
            telemetry: crate::telemetry::enabled(&headers),
        },
    );
    if let Some(storage) = &app.session_storage {
        storage
            .write("logins", &*flows)
            .map_err(|_| storage_failure())?;
    }
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
        .filter(|f| f.deadline > SystemTime::now())
        .ok_or_else(unauthenticated)?;
    let input = Login {
        org: None,
        slt: input.slt,
        test_key: None,
        operation_id: flow.operation_id,
    };
    let return_to = flow.return_to.clone();
    let mut headers = headers.clone();
    if !flow.telemetry {
        headers.insert("x-briefcase-telemetry", HeaderValue::from_static("off"));
    }
    drop(flows);
    let mut response = login(State(app.clone()), headers.clone(), Json(input)).await?;
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
    expires: SystemTime,
    deadline: SystemTime,
    refresh_key: IdempotencyKey,
    refresh_started_at: Option<SystemTime>,
    durable: Option<(Arc<crate::session_store::Storage>, String)>,
    durable_dirty: bool,
    org: Option<String>,
    organizations: Vec<String>,
    testing: bool,
    rejected: bool,
    test_environment: Option<TestSelection>,
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
    identifier_named(app.cookie, headers)
}
fn identifier_named(cookie_name: &str, headers: &HeaderMap) -> Option<String> {
    let mut found = None;
    for line in headers.get_all(header::COOKIE) {
        for item in line.to_str().ok()?.split(';') {
            if let Some((name, value)) = item.trim().split_once('=')
                && name == cookie_name
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
    if parent.deadline <= SystemTime::now() || parent.rejected || parent.testing {
        return Err(unauthenticated());
    }
    drop(parent);
    Ok(session)
}
pub(crate) fn selected_environment(headers: &HeaderMap) -> Result<Option<Uuid>> {
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
async fn standalone_test(
    app: &App,
    headers: &HeaderMap,
    environment: Uuid,
) -> Result<Arc<Mutex<Session>>> {
    let id = identifier_named(&format!("{}-testing", app.cookie), headers)
        .ok_or_else(unauthenticated)?;
    let session = app
        .sessions
        .lock()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(unauthenticated)?;
    let value = session.lock().await;
    if !value.testing
        || value.rejected
        || value.deadline <= SystemTime::now()
        || !value
            .test_environment
            .as_ref()
            .is_some_and(|e| e.id == environment)
    {
        return Err(unauthenticated());
    }
    drop(value);
    Ok(session)
}

async fn lookup(app: &App, headers: &HeaderMap) -> Result<Arc<Mutex<Session>>> {
    let Some(environment) = selected_environment(headers)? else {
        return production_session(app, headers).await;
    };
    if let Ok(parent) = production_session(app, headers).await {
        let child = parent.lock().await.test_sessions.get(&environment).cloned();
        if let Some(child) = child {
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
            return Ok(session);
        }
    }
    // A test-only browser session has its own cookie and can never satisfy a
    // production lookup, including after Exit testing clears the selector.
    standalone_test(app, headers, environment).await
}

fn unauthenticated() -> Failure {
    Failure(
        StatusCode::UNAUTHORIZED,
        "Sign in to Briefcase to continue.".into(),
    )
}
pub(crate) async fn login(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Result<Response> {
    if let Some(app_secret) = input.test_key {
        return enter_secret(
            State(app),
            headers,
            Json(EnterSecret {
                app_secret,
                slt: input.slt,
                org: input.org,
                operation_id: input.operation_id,
            }),
        )
        .await;
    }
    let (id, value) = establish(&app, input, crate::telemetry::enabled(&headers)).await?;
    Ok((
        [(header::SET_COOKIE, cookie(&app, &id, SESSION_SECONDS)?)],
        Json(value),
    )
        .into_response())
}
async fn establish(app: &App, input: Login, telemetry: bool) -> Result<(String, Value)> {
    if input.operation_id.is_nil()
        || input
            .org
            .as_ref()
            .is_some_and(|org| org.is_empty() || org.len() > 128 || org.trim() != org)
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
                .map_or(true, |s| s.deadline > SystemTime::now() && !s.rejected)
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
        let mut previous = previous.lock().await;
        if previous.durable_dirty {
            previous.save()?;
        }
        return Ok((id, view(&previous)));
    }
    // A callback retry belongs to the original login operation. It must not
    // resurrect that login after an explicit logout or its family deadline.
    if let Some(storage) = &app.session_storage
        && let Some(saved) = storage
            .read::<SavedSession>(&id)
            .map_err(|_| storage_failure())?
        && (saved.rejected || saved.deadline <= SystemTime::now())
    {
        return Err(unauthenticated());
    }
    let mut config = Config::for_sign_in(&app.upstream)?
        .with_auto_update(false)
        .with_telemetry(telemetry)
        .with_telemetry_source(briefcase_client::telemetry::Source::Web);
    let testing = input.test_key.is_some();
    if let Some(key) = input.test_key {
        config = config.with_environment(EnvironmentKey::new(key)?);
        if let Some(org) = &input.org {
            config = config.with_organization(org.clone())?;
        }
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
    let mut session = Session {
        auth_config,
        expires: SystemTime::now() + Duration::from_secs(tokens.expires_in.min(86400)),
        deadline: SystemTime::now() + Duration::from_secs(u64::from(SESSION_SECONDS)),
        config,
        tokens,
        refresh_key: IdempotencyKey::random(),
        refresh_started_at: None,
        durable: app
            .session_storage
            .clone()
            .map(|storage| (storage, id.clone())),
        durable_dirty: false,
        org: session_org,
        organizations,
        testing,
        rejected: false,
        test_environment: None,
        test_sessions: Default::default(),
    };
    session.save()?;
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
    if session.deadline <= SystemTime::now() || session.rejected {
        return Err(unauthenticated());
    }
    refresh_if_needed(&mut session, crate::telemetry::enabled(&headers)).await?;
    if !session.organizations.iter().any(|org| org == &input.org) {
        return Err(Failure(
            StatusCode::FORBIDDEN,
            "That workspace is not available in this IAM session.".into(),
        ));
    }
    session.config = session.auth_config.clone().with_organization(&input.org)?;
    session.org = Some(input.org);
    session.save()?;
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
async fn refresh_if_needed(session: &mut Session, telemetry: bool) -> Result<()> {
    // Never serve an advanced in-memory token until its successor and retry
    // metadata survive a restart. A failed rename/fsync leaves this flag set.
    if session.durable_dirty {
        session.save()?;
    }
    for _ in 0..2 {
        if session.expires > SystemTime::now() + Duration::from_secs(30)
            && session.refresh_started_at.is_none()
        {
            return Ok(());
        }
        let started_at = *session
            .refresh_started_at
            .get_or_insert_with(SystemTime::now);
        session.save()?;
        let client = Client::new_unchecked(
            session
                .auth_config
                .clone()
                .with_telemetry(telemetry)
                .with_telemetry_source(briefcase_client::telemetry::Source::Web),
        )?;
        let result = client
            .refresh_session_with_key(&session.tokens.refresh_token, &session.refresh_key)
            .await;
        match result {
            Ok(tokens) => {
                session.expires = started_at + Duration::from_secs(tokens.expires_in.min(86400));
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
                session.refresh_started_at = None;
                session.save()?;
            }
            Err(error) => {
                if error.is_unauthenticated() {
                    session.rejected = true;
                    session.save()?;
                }
                return Err(error.into());
            }
        }
    }
    if session.expires <= SystemTime::now() + Duration::from_secs(30) {
        return Err(Failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "The recovered session needs renewal. Retry the same action.".into(),
        ));
    }
    Ok(())
}
async fn authenticated_client(app: &App, headers: &HeaderMap) -> Result<Client> {
    let session = lookup(app, headers).await?;
    let mut session = session.lock().await;
    if session.deadline <= SystemTime::now() || session.rejected {
        return Err(unauthenticated());
    }
    refresh_if_needed(&mut session, crate::telemetry::enabled(headers)).await?;
    Ok(Client::new_unchecked(
        session
            .config
            .clone()
            .with_token(session.tokens.access_token.clone())
            .with_telemetry(crate::telemetry::enabled(headers))
            .with_telemetry_source(briefcase_client::telemetry::Source::Web),
    )?)
}
pub(crate) async fn status(State(app): State<App>, headers: HeaderMap) -> Result<Json<Value>> {
    if identifier(&app, &headers).is_none()
        && identifier_named(&format!("{}-testing", app.cookie), &headers).is_none()
    {
        return Ok(Json(json!({"authenticated":false})));
    }
    let session = lookup(&app, &headers).await?;
    let mut session = session.lock().await;
    if session.deadline <= SystemTime::now() || session.rejected {
        return Err(unauthenticated());
    }
    refresh_if_needed(&mut session, crate::telemetry::enabled(&headers)).await?;
    Ok(Json(view(&session)))
}
pub(crate) async fn logout(State(app): State<App>, headers: HeaderMap) -> Result<Response> {
    if let Some(environment) = selected_environment(&headers)? {
        if let Ok(parent) = production_session(&app, &headers).await {
            let mut parent = parent.lock().await;
            if let Some(id) = parent.test_sessions.get(&environment).cloned() {
                reject(&app, &id).await?;
                parent.test_sessions.remove(&environment);
                parent.save()?;
                return Ok(Json(json!({"authenticated":false})).into_response());
            }
        }
        standalone_test(&app, &headers, environment).await?;
        if let Some(id) = identifier_named(&format!("{}-testing", app.cookie), &headers) {
            reject(&app, &id).await?;
        }
        return Ok((
            [(header::SET_COOKIE, testing_cookie(&app, "", 0)?)],
            Json(json!({"authenticated":false})),
        )
            .into_response());
    }
    if let Some(id) = identifier(&app, &headers) {
        reject(&app, &id).await?;
    }
    Ok((
        [(header::SET_COOKIE, cookie(&app, "", 0)?)],
        Json(json!({"authenticated":false})),
    )
        .into_response())
}

async fn reject(app: &App, id: &str) -> Result<()> {
    let session = app.sessions.lock().await.get(id).cloned();
    if let Some(session) = session {
        let mut session = session.lock().await;
        session.rejected = true;
        session.save()?;
        app.sessions.lock().await.remove(id);
    }
    Ok(())
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
        if saved.deadline <= SystemTime::now()
            || saved.rejected
            || !saved.test_environment.as_ref().is_some_and(|e| {
                e.version == environment.version && e.key_generation == key.key_generation
            })
        {
            return Err(unauthenticated());
        }
        refresh_if_needed(&mut saved, crate::telemetry::enabled(&headers)).await?;
        if !Client::new_unchecked(
            saved
                .config
                .clone()
                .with_token(saved.tokens.access_token.clone())
                .with_telemetry(crate::telemetry::enabled(&headers))
                .with_telemetry_source(briefcase_client::telemetry::Source::Web),
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
        crate::telemetry::enabled(&headers),
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
    if parent.deadline <= SystemTime::now() || parent.rejected {
        return Err(unauthenticated());
    }
    let mut child = child.lock().await;
    if child.org.as_deref() != Some(environment.org_id.as_str()) {
        return Err(bad("The test account has no access to this organization."));
    }
    child.deadline = child.deadline.min(parent.deadline);
    child.test_environment = Some(TestSelection {
        id: environment.id,
        name: environment.name,
        version: environment.version,
        key_generation: environment.key_generation,
    });
    parent.test_sessions.insert(id, child_id);
    child.save()?;
    parent.save()?;
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
            session_storage: None,
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
            config = config
                .with_environment(EnvironmentKey::new(format!("ask_{}", "a".repeat(43))).unwrap());
        }
        let environment = test.map(|id| serde_json::from_value(json!({
            "id":id,"org_id":"tos","name":"Demo","description":null,"status":"active",
            "iam_environment_id":Uuid::new_v4(),"iam_app_id":"briefcase",
            "created_by":{"type":"carbon","id":"saket"},"key_generation":1,"key_rotated_at":null,
            "last_activity_at":"2026-09-11T00:00:00Z","cleaned_at":null,"deleted_at":null,"purge_after":null,
            "version":1,"created_at":"2026-09-11T00:00:00Z","updated_at":"2026-09-11T00:00:00Z"
        })).unwrap());
        Session {
            auth_config: config.clone(), config, tokens: serde_json::from_value(json!({
                "access_token":"test-token","refresh_token":"test-refresh","token_type":"Bearer","expires_in":3600,
                "scope":"profile","actor":{"principal_id":Uuid::new_v4(),"type":"carbon","public_id":"saket"},
                "org_id":"tos","organizations":["tos"]
            })).unwrap(), expires: SystemTime::now()+Duration::from_secs(3600),deadline:SystemTime::now()+Duration::from_secs(3600),
            refresh_key:IdempotencyKey::random(), refresh_started_at:None, durable:None, durable_dirty:false, org:Some("tos".into()),organizations:vec!["tos".into()],
            testing:test.is_some(),rejected:false,test_environment:environment,test_sessions:Default::default(),
        }
    }

    #[tokio::test]
    async fn restart_preserves_production_test_isolation_and_durable_logout() {
        let root = tempfile::tempdir().unwrap();
        let owner = "a".repeat(64);
        let child_id = "b".repeat(64);
        let environment = Uuid::new_v4();
        {
            let storage =
                crate::session_store::Storage::open(root.path(), "api", "origin").unwrap();
            let mut parent = session(None);
            parent.deadline = SystemTime::now() + Duration::from_secs(u64::from(SESSION_SECONDS));
            parent.durable = Some((storage.clone(), owner.clone()));
            parent.test_sessions.insert(environment, child_id.clone());
            parent
                .save()
                .unwrap_or_else(|failure| panic!("{}", failure.1));
            let mut child = session(Some(environment));
            child.durable = Some((storage.clone(), child_id.clone()));
            child
                .save()
                .unwrap_or_else(|failure| panic!("{}", failure.1));
        }
        {
            let storage =
                crate::session_store::Storage::open(root.path(), "api", "origin").unwrap();
            let mut app = app();
            app.sessions = Arc::new(Mutex::new(restore(&storage).unwrap()));
            app.session_storage = Some(storage);
            let parent = lookup(&app, &headers(&owner, None)).await.ok().unwrap();
            assert!(
                parent.lock().await.deadline > SystemTime::now() + Duration::from_secs(8 * 3600)
            );
            let child = lookup(&app, &headers(&owner, Some(environment)))
                .await
                .ok()
                .unwrap();
            assert!(child.lock().await.testing);
            assert!(lookup(&app, &headers(&child_id, None)).await.is_err());
            logout(State(app.clone()), headers(&owner, Some(environment)))
                .await
                .ok()
                .unwrap();
            assert!(lookup(&app, &headers(&owner, None)).await.is_ok());
        }
        let storage = crate::session_store::Storage::open(root.path(), "api", "origin").unwrap();
        let mut app = app();
        app.sessions = Arc::new(Mutex::new(restore(&storage).unwrap()));
        assert!(lookup(&app, &headers(&owner, None)).await.is_ok());
        assert!(
            lookup(&app, &headers(&owner, Some(environment)))
                .await
                .is_err()
        );
        logout(State(app.clone()), headers(&owner, None))
            .await
            .ok()
            .unwrap();
        assert!(restore(&storage).unwrap().is_empty());
    }

    #[tokio::test]
    async fn pending_sign_in_keeps_its_operation_identity_across_restart() {
        let root = tempfile::tempdir().unwrap();
        let operation;
        let nonce;
        {
            let storage =
                crate::session_store::Storage::open(root.path(), "api", "origin").unwrap();
            let mut app = app();
            app.session_storage = Some(storage);
            start(
                State(app.clone()),
                HeaderMap::new(),
                Json(Start {
                    return_to: Some("/".into()),
                }),
            )
            .await
            .ok()
            .unwrap();
            let logins = app.logins.lock().await;
            let (saved_nonce, flow) = logins.iter().next().unwrap();
            operation = flow.operation_id;
            nonce = saved_nonce.clone();
        }
        let storage = crate::session_store::Storage::open(root.path(), "api", "origin").unwrap();
        let logins: std::collections::HashMap<String, LoginFlow> =
            storage.read("logins").unwrap().unwrap();
        assert_eq!(logins[&nonce].operation_id, operation);
        assert!(logins[&nonce].deadline > SystemTime::now());
    }

    #[tokio::test]
    async fn logout_waits_for_a_refresh_owner_and_cannot_resurrect_on_restart() {
        let root = tempfile::tempdir().unwrap();
        let storage = crate::session_store::Storage::open(root.path(), "api", "origin").unwrap();
        let mut app = app();
        let id = "a".repeat(64);
        let mut saved = session(None);
        saved.durable = Some((storage.clone(), id.clone()));
        saved.save().ok().unwrap();
        let saved = Arc::new(Mutex::new(saved));
        app.sessions.lock().await.insert(id.clone(), saved.clone());
        app.session_storage = Some(storage.clone());
        let mut refresh_owner = saved.lock().await;
        let logout = tokio::spawn(logout(State(app), headers(&id, None)));
        tokio::task::yield_now().await;
        assert!(!logout.is_finished());
        refresh_owner.tokens.refresh_token = "rotated".into();
        refresh_owner.save().ok().unwrap();
        drop(refresh_owner);
        assert!(logout.await.unwrap().is_ok());
        assert!(restore(&storage).unwrap().is_empty());
        assert!(saved.lock().await.rejected);
    }

    #[tokio::test]
    async fn an_old_login_retry_cannot_replace_a_durable_logout() {
        let root = tempfile::tempdir().unwrap();
        let storage = crate::session_store::Storage::open(root.path(), "api", "origin").unwrap();
        let mut app = app();
        app.session_storage = Some(storage.clone());
        let input = Login {
            org: None,
            slt: "already-exchanged".into(),
            test_key: None,
            operation_id: Uuid::new_v4(),
        };
        let mut hash = Sha256::new();
        hash.update(app.login_salt.as_bytes());
        hash.update(serde_json::to_vec(&input).unwrap());
        let id = format!("{:x}", hash.finalize());
        let mut saved = session(None);
        saved.rejected = true;
        saved.durable = Some((storage, id));
        saved.save().ok().unwrap();
        let failure = establish(&app, input, false).await.err().unwrap();
        assert_eq!(failure.0, StatusCode::UNAUTHORIZED);
        assert!(app.sessions.lock().await.is_empty());
    }

    #[tokio::test]
    async fn uncertain_refresh_survives_restart_and_delayed_replay_is_renewed() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_json, header, method, path},
        };
        let server = MockServer::start().await;
        let root = tempfile::tempdir().unwrap();
        let id = "a".repeat(64);
        let key = "original-refresh-attempt";
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/refresh"))
            .and(header("idempotency-key", key))
            .respond_with(
                ResponseTemplate::new(503)
                    .set_body_json(json!({"error":{"code":"unavailable","message":"retry"}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        {
            let storage =
                crate::session_store::Storage::open(root.path(), "api", "origin").unwrap();
            let mut saved = session(None);
            saved.auth_config = Config::for_sign_in(&format!("{}/api/v1/", server.uri())).unwrap();
            saved.config = saved.auth_config.clone().with_organization("tos").unwrap();
            saved.expires = SystemTime::UNIX_EPOCH;
            saved.refresh_started_at = Some(SystemTime::UNIX_EPOCH);
            saved.refresh_key = IdempotencyKey::new(key).unwrap();
            saved.durable = Some((storage, id.clone()));
            assert!(refresh_if_needed(&mut saved, false).await.is_err());
            assert!(!saved.rejected);
        }
        server.reset().await;
        for (old, new) in [("test-refresh", "replayed"), ("replayed", "fresh")] {
            let mut tokens = session(None).tokens;
            tokens.access_token = format!("access-{new}");
            tokens.refresh_token = new.into();
            let mock = Mock::given(method("POST"))
                .and(path("/api/v1/auth/refresh"))
                .and(body_json(json!({"refresh_token":old})));
            if old == "test-refresh" {
                mock.and(header("idempotency-key", key))
                    .respond_with(ResponseTemplate::new(200).set_body_json(&tokens))
                    .expect(1)
                    .mount(&server)
                    .await;
            } else {
                mock.respond_with(ResponseTemplate::new(200).set_body_json(&tokens))
                    .expect(1)
                    .mount(&server)
                    .await;
            }
        }
        let storage = crate::session_store::Storage::open(root.path(), "api", "origin").unwrap();
        let sessions = restore(&storage).unwrap();
        let mut recovered = sessions[&id].lock().await;
        refresh_if_needed(&mut recovered, false)
            .await
            .unwrap_or_else(|failure| panic!("{}", failure.1));
        assert_eq!(recovered.tokens.refresh_token, "fresh");
        assert!(recovered.refresh_started_at.is_none());
        let reloaded = restore(&storage).unwrap();
        assert_eq!(reloaded[&id].lock().await.tokens.refresh_token, "fresh");
    }

    #[tokio::test]
    async fn failed_rotation_write_must_be_repaired_before_a_session_can_be_used() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        let root = tempfile::tempdir().unwrap();
        let storage = crate::session_store::Storage::open(root.path(), "api", "origin").unwrap();
        let id = "a".repeat(64);
        let state_file = root.path().join(format!("{id}.json"));
        let previous_file = root.path().join("before-refresh.json");
        let mut saved = session(None);
        saved.auth_config = Config::for_sign_in(&format!("{}/api/v1/", server.uri())).unwrap();
        saved.config = saved.auth_config.clone().with_organization("tos").unwrap();
        saved.expires = SystemTime::UNIX_EPOCH;
        saved.durable = Some((storage.clone(), id.clone()));
        saved.save().ok().unwrap();
        let mut tokens = saved.tokens.clone();
        tokens.access_token = "access-new".into();
        tokens.refresh_token = "refresh-new".into();
        let blocked = state_file.clone();
        let backup = previous_file.clone();
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/refresh"))
            .respond_with(move |_: &wiremock::Request| {
                // The old credential and attempt are safely on disk when IAM
                // rotates, then the final durable replacement fails.
                std::fs::rename(&blocked, &backup).unwrap();
                std::fs::create_dir(&blocked).unwrap();
                ResponseTemplate::new(200).set_body_json(&tokens)
            })
            .expect(1)
            .mount(&server)
            .await;
        assert!(refresh_if_needed(&mut saved, false).await.is_err());
        assert_eq!(saved.tokens.refresh_token, "refresh-new");
        assert!(saved.durable_dirty);
        // A still-valid access token must not make the next request bypass saving.
        assert!(refresh_if_needed(&mut saved, false).await.is_err());
        std::fs::remove_dir(&state_file).unwrap();
        std::fs::rename(&previous_file, &state_file).unwrap();
        refresh_if_needed(&mut saved, false).await.ok().unwrap();
        assert!(!saved.durable_dirty);
        let reloaded = restore(&storage).unwrap();
        assert_eq!(
            reloaded[&id].lock().await.tokens.refresh_token,
            "refresh-new"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn standalone_test_cookie_never_authenticates_production_or_another_environment() {
        let app = app();
        let environment = Uuid::new_v4();
        let token = "c".repeat(64);
        let test = Arc::new(Mutex::new(session(Some(environment))));
        app.sessions
            .lock()
            .await
            .insert(token.clone(), test.clone());
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("briefcase_dev-testing={token}")).unwrap(),
        );
        assert!(lookup(&app, &headers).await.is_err());
        headers.insert(
            "x-briefcase-environment",
            HeaderValue::from_str(&environment.to_string()).unwrap(),
        );
        assert!(Arc::ptr_eq(
            &lookup(&app, &headers).await.ok().unwrap(),
            &test
        ));
        headers.insert(
            "x-briefcase-environment",
            HeaderValue::from_str(&Uuid::new_v4().to_string()).unwrap(),
        );
        assert!(lookup(&app, &headers).await.is_err());
        headers.insert(
            "x-briefcase-environment",
            HeaderValue::from_str(&environment.to_string()).unwrap(),
        );
        assert!(logout(State(app.clone()), headers.clone()).await.is_ok());
        assert!(lookup(&app, &headers).await.is_err());
        assert!(!app.sessions.lock().await.contains_key(&token));
    }

    #[tokio::test]
    async fn secret_and_test_identity_can_sign_in_without_a_production_cookie_or_org() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_json, header as matches_header, method, path},
        };
        let server = MockServer::start().await;
        let mut app = app();
        app.upstream = format!("{}/api/v1/", server.uri());
        let environment = Uuid::new_v4();
        let secret = format!("ask_{}", "a".repeat(43));
        Mock::given(method("GET")).and(path("/api/v1/testing-environment"))
            .and(matches_header("x-briefcase-app-secret", secret.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":environment,"name":"Sandbox","description":null,"key_generation":1,"created_at":"2026-09-13T00:00:00Z"})))
            .mount(&server).await;
        let operations: Vec<_> = briefcase_client::OPERATIONS
            .iter()
            .map(|op| json!({"id":op.id,"version":op.version,"method":op.method,"path":op.path}))
            .collect();
        Mock::given(method("GET")).and(path("/api/version"))
            .respond_with(ResponseTemplate::new(200).insert_header("briefcase-api-version", "v1").set_body_json(json!({"service":"silicon-briefcase","selected_api_version":"v1","supported_api_versions":["v1"],"contract_version":"1.0.0","build":"test","operations":operations})))
            .mount(&server).await;
        Mock::given(method("POST")).and(path("/api/v1/auth/slt"))
            .and(matches_header("x-briefcase-app-secret", secret.as_str()))
            .and(body_json(json!({"slt":"worker:tos"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token":"test-access","refresh_token":"test-refresh","token_type":"Bearer","expires_in":3600,"scope":"profile","actor":{"principal_id":Uuid::new_v4(),"type":"silicon","public_id":"worker:tos"},"org_id":"tos","organizations":["tos"]})))
            .expect(1).mount(&server).await;
        let mut privacy = HeaderMap::new();
        privacy.insert(
            header::COOKIE,
            HeaderValue::from_static("briefcase_telemetry=off"),
        );
        let response = enter_secret(
            State(app.clone()),
            privacy,
            Json(EnterSecret {
                app_secret: secret,
                slt: "worker:tos".into(),
                org: None,
                operation_id: Uuid::new_v4(),
            }),
        )
        .await
        .unwrap_or_else(|failure| panic!("{}: {}", failure.0, failure.1));
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.starts_with("briefcase_dev-testing="));
        assert!(cookie.contains("HttpOnly"));
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(cookie.split(';').next().unwrap()).unwrap(),
        );
        assert!(
            lookup(&app, &headers).await.is_err(),
            "test cookie cannot become production"
        );
        headers.insert(
            "x-briefcase-environment",
            HeaderValue::from_str(&environment.to_string()).unwrap(),
        );
        let child = lookup(&app, &headers).await.ok().unwrap();
        let child = child.lock().await;
        assert_eq!(child.tokens.actor.public_id, "worker:tos");
        assert_eq!(child.org.as_deref(), Some("tos"));
        assert!(child.config.environment().is_some());
        drop(child);
        let status = status(State(app), headers).await.ok().unwrap();
        assert_eq!(status.0["authenticated"], true);
        assert_eq!(status.0["test_environment"]["id"], environment.to_string());
        for request in server.received_requests().await.unwrap() {
            assert_eq!(request.headers["x-briefcase-telemetry"], "off");
            assert_eq!(request.headers["x-briefcase-source"], "web");
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
        child.lock().await.deadline = SystemTime::now();
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
            .deadline = SystemTime::now();
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

#[derive(Clone, Deserialize, Serialize)]
struct TestSelection {
    id: Uuid,
    name: String,
    version: i64,
    key_generation: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnterSecret {
    app_secret: String,
    slt: String,
    org: Option<String>,
    operation_id: Uuid,
}
/// Signs into a sandbox independently, preserving any existing production session.
pub(crate) async fn enter_secret(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<EnterSecret>,
) -> Result<Response> {
    if selected_environment(&headers)?.is_some() {
        return Err(bad(
            "Return to production before selecting another environment.",
        ));
    }
    let parent = production_session(&app, &headers).await.ok();
    let config = Config::for_sign_in(&app.upstream)?
        .with_environment(EnvironmentKey::new(input.app_secret.clone())?)
        .with_auto_update(false)
        .with_telemetry(crate::telemetry::enabled(&headers))
        .with_telemetry_source(briefcase_client::telemetry::Source::Web);
    let current = Client::new_unchecked(config)?
        .current_testing_environment()
        .await?;
    let (child_id, _) = establish(
        &app,
        Login {
            org: input.org,
            slt: input.slt,
            test_key: Some(input.app_secret),
            operation_id: input.operation_id,
        },
        crate::telemetry::enabled(&headers),
    )
    .await?;
    let child = app
        .sessions
        .lock()
        .await
        .get(&child_id)
        .cloned()
        .ok_or_else(unauthenticated)?;
    let mut child = child.lock().await;
    child.test_environment = Some(TestSelection {
        id: current.id,
        name: current.name,
        version: 0,
        key_generation: current.key_generation,
    });
    child.save()?;
    let value = view(&child);
    drop(child);
    if let Some(parent) = parent {
        let mut parent = parent.lock().await;
        if !parent.rejected && parent.deadline > SystemTime::now() {
            parent.test_sessions.insert(current.id, child_id.clone());
            parent.save()?;
        }
    }
    // Its own cookie lets this test session continue even if production later
    // expires or signs out; the test world's credentials still govern access.
    Ok((
        [(
            header::SET_COOKIE,
            testing_cookie(&app, &child_id, SESSION_SECONDS)?,
        )],
        Json(value),
    )
        .into_response())
}

fn testing_cookie(app: &App, id: &str, age: u32) -> Result<HeaderValue> {
    HeaderValue::from_str(&format!(
        "{}-testing={id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={age}{}",
        app.cookie,
        if app.secure { "; Secure" } else { "" }
    ))
    .map_err(|_| bad("Invalid testing session"))
}
