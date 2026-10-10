//! Signing in at Silicon Accounts as Briefcase's public client, against a mock.
#![cfg(feature = "accounts")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use briefcase_client::accounts::{DeviceEvent, DeviceRequest, SignIn, SignInError, SltRefusal};
use secrecy::ExposeSecret as _;
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, body_string_contains, method, path},
};

fn tokens(access: &str, refresh: &str) -> Value {
    json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": 1800,
        "refresh_token": refresh,
        "refresh_token_expires_at": "2028-01-01T00:00:00.000Z",
        "scope": "profile email",
        "membership_id": "briefcase:zQo",
        "account": {
            "uuid": "zQo",
            "membership_id": "briefcase:zQo",
            "kind": "carbon",
            "id": "c:ada",
            "display_name": "Ada",
            "pfp_url": "",
            "email": "ada@example.test"
        }
    })
}

fn oauth(status: u16, error: &str, description: &str) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .set_body_json(json!({"error": error, "error_description": description}))
}

async fn device_authorization(server: &MockServer, expires_in: u64) {
    Mock::given(method("POST"))
        .and(path("/v1/device/authorize"))
        .and(body_partial_json(json!({"client_id": "briefcase"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "sad_device_secret",
            "user_code": "WDJB-MJHT",
            "verification_uri": format!("{}/device", server.uri()),
            "verification_uri_complete": format!("{}/device?user_code=WDJB-MJHT", server.uri()),
            "expires_in": expires_in,
            "interval": 1
        })))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_device_sign_in_waits_through_pending_and_slow_down_then_returns_the_account() {
    let server = MockServer::start().await;
    device_authorization(&server, 600).await;
    let poll = || {
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .and(body_string_contains(
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
            ))
            .and(body_string_contains("client_id=briefcase"))
            .and(body_string_contains("device_code=sad_device_secret"))
    };
    poll()
        .respond_with(oauth(400, "authorization_pending", "Not approved yet."))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    poll()
        .respond_with(oauth(400, "slow_down", "Polling too fast."))
        .up_to_n_times(1)
        .with_priority(2)
        .mount(&server)
        .await;
    poll()
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens("eyJ.access", "sar_one")))
        .with_priority(3)
        .mount(&server)
        .await;

    let sign_in = SignIn::new(&server.uri()).unwrap();
    let device = sign_in
        .start_device_sign_in(&DeviceRequest {
            scope: Some("email".to_owned()),
            label: Some("briefcase CLI on test".to_owned()),
        })
        .await
        .unwrap();
    assert_eq!(device.user_code, "WDJB-MJHT");
    assert!(device.browser_url().ends_with("user_code=WDJB-MJHT"));
    assert!(!format!("{device:?}").contains("sad_device_secret"));

    let mut events = Vec::new();
    let started = std::time::Instant::now();
    let signed_in = sign_in
        .finish_device_sign_in(&device, |event| events.push(event))
        .await
        .unwrap();
    assert_eq!(
        events,
        vec![
            DeviceEvent::Pending,
            DeviceEvent::SlowDown {
                interval: Duration::from_secs(6)
            }
        ]
    );
    // One second, one second, then the six the slow_down asked for.
    assert!(
        started.elapsed() >= Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(signed_in.account.uuid, "zQo");
    assert_eq!(signed_in.account.id, "c:ada");
    assert_eq!(signed_in.account.display_name.as_deref(), Some("Ada"));
    assert_eq!(signed_in.access_token.expose_secret(), "eyJ.access");
    assert_eq!(
        signed_in.refresh_token.as_ref().unwrap().expose_secret(),
        "sar_one"
    );
    assert_eq!(signed_in.expires_in, Duration::from_mins(30));
    assert_eq!(signed_in.scope, vec!["profile", "email"]);
    assert!(!format!("{signed_in:?}").contains("sar_one"));

    let authorize = &server.received_requests().await.unwrap()[0];
    let body: Value = serde_json::from_slice(&authorize.body).unwrap();
    assert_eq!(body["scope"], "email");
    assert_eq!(body["client_label"], "briefcase CLI on test");
}

#[tokio::test]
async fn a_denied_device_sign_in_names_the_code() {
    let server = MockServer::start().await;
    device_authorization(&server, 600).await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(oauth(400, "access_denied", "The Carbon denied it."))
        .mount(&server)
        .await;
    let sign_in = SignIn::new(&server.uri()).unwrap();
    let device = sign_in
        .start_device_sign_in(&DeviceRequest::default())
        .await
        .unwrap();
    let error = sign_in
        .finish_device_sign_in(&device, |_| {})
        .await
        .unwrap_err();
    assert!(
        matches!(error, SignInError::DeviceDenied { ref user_code } if user_code == "WDJB-MJHT")
    );
    assert_eq!(error.code(), "device_sign_in_denied");
}

#[tokio::test]
async fn a_device_code_that_expires_stops_the_wait() {
    let server = MockServer::start().await;
    device_authorization(&server, 2).await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(oauth(400, "authorization_pending", "Not approved yet."))
        .mount(&server)
        .await;
    let sign_in = SignIn::new(&server.uri()).unwrap();
    let device = sign_in
        .start_device_sign_in(&DeviceRequest::default())
        .await
        .unwrap();
    let error = sign_in
        .finish_device_sign_in(&device, |_| {})
        .await
        .unwrap_err();
    assert_eq!(error.code(), "device_code_expired");

    // Silicon Accounts saying so ends the wait just the same.
    let server = MockServer::start().await;
    device_authorization(&server, 600).await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(oauth(400, "expired_token", "The device code expired."))
        .mount(&server)
        .await;
    let sign_in = SignIn::new(&server.uri()).unwrap();
    let device = sign_in
        .start_device_sign_in(&DeviceRequest::default())
        .await
        .unwrap();
    let error = sign_in
        .finish_device_sign_in(&device, |_| {})
        .await
        .unwrap_err();
    assert_eq!(error.code(), "device_code_expired");
}

#[tokio::test]
async fn an_app_without_device_sign_ins_is_told_exactly_that() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/device/authorize"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": {
            "code": "unauthorized_client",
            "message": "The app 'briefcase' hasn't turned on device sign-ins, so its tools can't start one.",
            "hint": "Its owner turns it on with PATCH /v1/apps/{app_id}/signin-config."
        }})))
        .mount(&server)
        .await;
    let error = SignIn::new(&server.uri())
        .unwrap()
        .start_device_sign_in(&DeviceRequest::default())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "public_client_disabled");
    assert!(error.message().contains("hasn't turned on device sign-ins"));
}

#[tokio::test]
async fn a_short_lived_token_is_exchanged_with_the_client_id_alone() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .and(body_string_contains(
            "grant_type=urn%3Asilicon%3Aparams%3Aoauth%3Agrant-type%3Aslt",
        ))
        .and(body_string_contains("slt=slt_good"))
        .and(body_string_contains("client_id=briefcase"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens("eyJ.slt", "sar_slt")))
        .expect(1)
        .mount(&server)
        .await;
    let signed_in = SignIn::new(&server.uri())
        .unwrap()
        .exchange_short_lived_token("  slt_good\n")
        .await
        .unwrap();
    assert_eq!(signed_in.account.uuid, "zQo");
    let request = &server.received_requests().await.unwrap()[0];
    let body = String::from_utf8_lossy(&request.body);
    assert!(!body.contains("client_secret"), "{body}");
    assert!(!request.headers.contains_key("authorization"));
}

#[tokio::test]
async fn every_refused_short_lived_token_reason_has_its_own_code() {
    for (description, code) in [
        (
            "The short-lived token was already used; each one works once. Get a new one.",
            "slt_already_used",
        ),
        (
            "The short-lived token expired at 2026-10-10T10:00:00.000Z (they last 120 seconds).",
            "slt_expired",
        ),
        (
            "The short-lived token was issued for the app 'dm', not for 'briefcase'.",
            "slt_wrong_app",
        ),
        (
            "The short-lived token is not known: it is mistyped or was never issued.",
            "slt_unknown",
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .respond_with(oauth(400, "invalid_grant", description))
            .mount(&server)
            .await;
        let error = SignIn::new(&server.uri())
            .unwrap()
            .exchange_short_lived_token("slt_refused")
            .await
            .unwrap_err();
        assert_eq!(error.code(), code, "{description}");
        assert_eq!(error.message(), description);
        assert!(
            error
                .hint()
                .unwrap()
                .contains("silicon-accounts login --app briefcase -q"),
            "{error:?}"
        );
    }
}

#[tokio::test]
async fn something_that_is_not_a_short_lived_token_is_refused_before_anything_is_sent() {
    let server = MockServer::start().await;
    let sign_in = SignIn::new(&server.uri()).unwrap();
    let error = sign_in
        .exchange_short_lived_token("sar_refresh_token_secret")
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SignInError::ShortLivedTokenRefused {
            reason: SltRefusal::NotAShortLivedToken,
            ..
        }
    ));
    assert!(!error.message().contains("sar_refresh_token_secret"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn an_app_without_a_public_client_cannot_exchange_short_lived_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(oauth(
            401,
            "unauthorized_client",
            "grant_type=urn:silicon:params:oauth:grant-type:slt needs a confidential client.",
        ))
        .mount(&server)
        .await;
    let error = SignIn::new(&server.uri())
        .unwrap()
        .exchange_short_lived_token("slt_x")
        .await
        .unwrap_err();
    assert_eq!(error.code(), "public_client_disabled");
}

#[tokio::test]
async fn a_refresh_rotates_the_pair_and_a_reused_token_ends_the_sign_in() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=sar_one"))
        .and(body_string_contains("client_id=briefcase"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens("eyJ.two", "sar_two")))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(oauth(
            400,
            "invalid_grant",
            "This refresh token was already used once. Presenting a used refresh token revokes the whole sign-in.",
        ))
        .with_priority(2)
        .mount(&server)
        .await;
    let sign_in = SignIn::new(&server.uri()).unwrap();
    let rotated = sign_in.refresh("sar_one").await.unwrap();
    assert_eq!(
        rotated.refresh_token.as_ref().unwrap().expose_secret(),
        "sar_two"
    );
    let error = sign_in.refresh("sar_one").await.unwrap_err();
    assert_eq!(error.code(), "sign_in_ended");
    assert!(error.hint().unwrap().contains("briefcase login"));
}

#[tokio::test]
async fn revoking_sends_the_refresh_token_with_the_client_id_and_reads_the_outcome() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/revoke"))
        .and(body_string_contains("token=sar_one"))
        .and(body_string_contains("token_type_hint=refresh_token"))
        .and(body_string_contains("client_id=briefcase"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"revoked": true})))
        .expect(1)
        .mount(&server)
        .await;
    let revocation = SignIn::new(&server.uri())
        .unwrap()
        .revoke("sar_one")
        .await
        .unwrap();
    assert!(revocation.revoked);
}

#[tokio::test]
async fn an_unreachable_silicon_accounts_is_transient() {
    // Nothing listens on a port whose listener was closed.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let uri = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let error = SignIn::new(&uri)
        .unwrap()
        .exchange_short_lived_token("slt_x")
        .await
        .unwrap_err();
    assert_eq!(error.code(), "accounts_unreachable");
    assert!(error.is_transient());
}
