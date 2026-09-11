//! Browser session boundary. Every file operation uses the official Rust SDK.
mod access;
mod environments;
mod files;
mod organization;
mod session;
mod staging;

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine as _;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, Semaphore};
use tower_http::{
    sensitive_headers::{SetSensitiveRequestHeadersLayer, SetSensitiveResponseHeadersLayer},
    services::{ServeDir, ServeFile},
};

#[derive(Clone)]
struct App {
    upstream: String,
    origin: String,
    cookie: &'static str,
    secure: bool,
    sessions: Arc<Mutex<HashMap<String, Arc<Mutex<session::Session>>>>>,
    login_lock: Arc<Mutex<()>>,
    login_salt: String,
    uploads: Arc<Semaphore>,
    staging: Arc<staging::Storage>,
    logins: Arc<Mutex<HashMap<String, session::LoginFlow>>>,
    document_csp: HeaderValue,
}

struct Failure(StatusCode, String);
type Result<T> = std::result::Result<T, Failure>;
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        (self.0, axum::Json(json!({"error":{"message":self.1}}))).into_response()
    }
}
impl From<briefcase_client::Error> for Failure {
    fn from(value: briefcase_client::Error) -> Self {
        match value {
            briefcase_client::Error::Api(error) => Self(
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::BAD_GATEWAY),
                error.message,
            ),
            briefcase_client::Error::Configuration(message) => {
                Self(StatusCode::BAD_REQUEST, message)
            }
            _ => Self(
                StatusCode::BAD_GATEWAY,
                "Briefcase could not confirm this request. Retry the same action.".into(),
            ),
        }
    }
}
fn bad(message: &str) -> Failure {
    Failure(StatusCode::BAD_REQUEST, message.into())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let origin = std::env::var("BRIEFCASE_WEB_ORIGIN").unwrap_or("http://localhost:4317".into());
    let parsed = url::Url::parse(&origin)?;
    anyhow::ensure!(
        parsed.origin().ascii_serialization() == origin
            && parsed.username().is_empty()
            && parsed.password().is_none(),
        "Expected a canonical origin"
    );
    let secure = parsed.scheme() == "https";
    anyhow::ensure!(
        secure
            || (parsed.scheme() == "http"
                && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))),
        "HTTP is only allowed on loopback"
    );
    let assets = std::env::var("BRIEFCASE_WEB_ASSETS").unwrap_or("../../web/dist/client".into());
    let html = std::fs::read_to_string(format!("{assets}/index.html"));
    anyhow::ensure!(
        !secure || html.is_ok(),
        "Build the browser assets before running the HTTPS deployment"
    );
    let document_csp = document_policy(html.as_deref().unwrap_or(""))?;
    let state = App {
        upstream: std::env::var("BRIEFCASE_API_URL")
            .unwrap_or("https://backend.briefcase.teamofsilicons.com/api/v1/".into()),
        origin,
        cookie: if secure {
            "__Host-briefcase"
        } else {
            "briefcase_dev"
        },
        secure,
        sessions: Arc::new(Mutex::new(HashMap::new())),
        login_lock: Arc::new(Mutex::new(())),
        login_salt: format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()),
        uploads: Arc::new(Semaphore::new(4)),
        staging: staging::Storage::from_env()?,
        logins: Default::default(),
        document_csp,
    };
    let json_routes = Router::new()
        .route(
            "/browser/session",
            get(session::status)
                .patch(session::select)
                .delete(session::logout),
        )
        .route("/browser/resolve", get(files::resolve))
        .route("/browser/login/start", post(session::start))
        .route("/browser/notifications", get(access::inbox))
        .route("/browser/notifications/read", post(access::mark_read))
        .route("/auth/callback", get(session::callback))
        .route("/browser/entries", get(files::list).post(files::mkdir))
        .route(
            "/browser/entries/{id}",
            get(files::stat).patch(files::rename).delete(files::trash),
        )
        .route("/browser/entries/{id}/content", get(files::content))
        .route("/browser/entries/{id}/versions", get(files::versions))
        .route(
            "/browser/entries/{id}/versions/{version}/restore",
            post(files::restore_version),
        )
        .route("/browser/entries/{id}/activity", get(files::activity))
        .route(
            "/browser/entries/{id}/permissions",
            get(files::permissions).post(files::grant),
        )
        .route(
            "/browser/entries/{id}/permissions/{grant}",
            axum::routing::delete(files::revoke),
        )
        .route("/browser/bin", get(files::bin))
        .route("/browser/bin/{id}/restore", post(files::restore_bin))
        .route("/browser/search", get(files::search))
        .route("/browser/usage", get(files::usage))
        .route(
            "/browser/environments",
            get(environments::list).post(environments::create),
        )
        .route(
            "/browser/environments/{id}",
            axum::routing::patch(environments::edit),
        )
        .route(
            "/browser/environments/{id}/pairing",
            post(environments::pair),
        )
        .route("/browser/environments/{id}/key", post(environments::reveal))
        .route("/browser/environments/{id}/view", post(session::enter_test))
        .route(
            "/browser/environments/{id}/{action}",
            post(environments::action),
        )
        .route(
            "/browser/storage/configuration",
            axum::routing::put(organization::configure_storage),
        )
        .route(
            "/browser/{*unmatched}",
            axum::routing::any(|| async { Failure(StatusCode::NOT_FOUND, "Not found.".into()) }),
        )
        .layer(DefaultBodyLimit::max(16 * 1024));
    let router = json_routes
        .merge(
            Router::new()
                .route("/browser/upload", post(files::upload))
                .layer(DefaultBodyLimit::disable()),
        )
        .fallback_service(
            ServeDir::new(&assets).fallback(ServeFile::new(format!("{assets}/index.html"))),
        )
        .layer(middleware::from_fn_with_state(state.clone(), boundary))
        .layer(SetSensitiveRequestHeadersLayer::new([
            header::COOKIE,
            header::AUTHORIZATION,
        ]))
        .layer(SetSensitiveResponseHeadersLayer::new([header::SET_COOKIE]))
        .with_state(state);
    let address = std::env::var("BRIEFCASE_WEB_LISTEN").unwrap_or("127.0.0.1:4318".into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    println!("Briefcase browser gateway listening on {address}");
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn boundary(State(app): State<App>, mut request: Request, next: Next) -> Response {
    if request.uri().path().starts_with("/browser/") || request.uri().path() == "/auth/callback" {
        // Media/download elements cannot attach custom headers. Their public selector
        // chooses only a child already authenticated under this browser's parent session.
        if let Some(query) = request.uri().query() {
            let selectors: Vec<_> = url::form_urlencoded::parse(query.as_bytes())
                .filter(|(k, _)| k == "test_environment")
                .map(|(_, v)| v.into_owned())
                .collect();
            if !selectors.is_empty() {
                if selectors.len() != 1
                    || uuid::Uuid::parse_str(&selectors[0]).is_err()
                    || request
                        .headers()
                        .get("x-briefcase-environment")
                        .is_some_and(|h| h.to_str().ok() != Some(selectors[0].as_str()))
                {
                    return bad("Invalid testing environment.").into_response();
                }
                match HeaderValue::from_str(&selectors[0]) {
                    Ok(value) => {
                        request
                            .headers_mut()
                            .insert("x-briefcase-environment", value);
                    }
                    Err(_) => return bad("Invalid testing environment.").into_response(),
                }
            }
        }
        let h = request.headers();
        let mutating = !matches!(
            *request.method(),
            axum::http::Method::GET | axum::http::Method::HEAD
        );
        let callback =
            request.uri().path() == "/auth/callback" && request.method() == axum::http::Method::GET;
        if (!callback && h.get("sec-fetch-site").is_some_and(|v| v == "cross-site"))
            || (mutating
                && (h.get(header::ORIGIN).and_then(|v| v.to_str().ok())
                    != Some(app.origin.as_str())
                    || h.get("x-briefcase-browser").is_none_or(|v| v != "1")))
        {
            return Failure(
                StatusCode::FORBIDDEN,
                "Open Briefcase on its configured origin to continue.".into(),
            )
            .into_response();
        }
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    // Content handlers apply their stricter sandbox policy themselves.
    if !headers.contains_key("content-security-policy") {
        headers.insert("content-security-policy", app.document_csp.clone());
    }
    response
}

// The generated HTML is trusted build output, never uploaded content. Hash its
// exact inline hydration scripts instead of enabling arbitrary inline script.
fn document_policy(html: &str) -> anyhow::Result<HeaderValue> {
    let mut scripts = String::from("script-src 'self'");
    for part in html.split("<script").skip(1) {
        if let Some((attributes, rest)) = part.split_once('>')
            && !attributes.contains("src=")
            && let Some((script, _)) = rest.split_once("</script>")
        {
            let digest =
                base64::engine::general_purpose::STANDARD.encode(Sha256::digest(script.as_bytes()));
            scripts.push_str(&format!(" 'sha256-{digest}'"));
        }
    }
    Ok(HeaderValue::from_str(&format!(
        "default-src 'self'; {scripts}; style-src 'self' 'unsafe-inline'; img-src 'self' blob:; media-src 'self' blob:; object-src 'none'; frame-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"
    ))?)
}
