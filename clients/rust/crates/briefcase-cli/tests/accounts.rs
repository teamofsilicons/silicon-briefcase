//! Signing in, refreshing, signing out and discovery, process by process,
//! against mock Silicon Accounts and Briefcase servers.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{
    Home, api, briefcase, briefcase_service, briefcase_with_stdin, json_error, oauth_error, stderr,
    stdout, tokens,
};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, header, method, path},
};

async fn device_authorization(accounts: &MockServer, expires_in: u64) {
    Mock::given(method("POST"))
        .and(path("/v1/device/authorize"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "sad_device_secret",
            "user_code": "WDJB-MJHT",
            "verification_uri": format!("{}/device", accounts.uri()),
            "verification_uri_complete": format!("{}/device?user_code=WDJB-MJHT", accounts.uri()),
            "expires_in": expires_in,
            "interval": 1
        })))
        .mount(accounts)
        .await;
}

fn lines(output: &std::process::Output) -> Vec<Value> {
    stdout(output)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap_or_else(|_| panic!("not JSON: {line}")))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_carbon_signs_in_with_a_device_code_through_pending_and_slow_down() {
    let accounts = MockServer::start().await;
    let service = briefcase_service(&accounts.uri()).await;
    device_authorization(&accounts, 600).await;
    let poll = || {
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .and(body_string_contains("device_code=sad_device_secret"))
            .and(body_string_contains("client_id=briefcase"))
    };
    poll()
        .respond_with(oauth_error(
            400,
            "authorization_pending",
            "Not approved yet.",
        ))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&accounts)
        .await;
    poll()
        .respond_with(oauth_error(400, "slow_down", "Too fast."))
        .up_to_n_times(1)
        .with_priority(2)
        .mount(&accounts)
        .await;
    poll()
        .respond_with(
            ResponseTemplate::new(200).set_body_json(tokens("access-device", "sar_device")),
        )
        .with_priority(3)
        .mount(&accounts)
        .await;

    let home = Home::new();
    let output = briefcase(
        &home,
        &[
            "--json",
            "--url",
            &api(&service),
            "--accounts-url",
            &accounts.uri(),
            "login",
        ],
        &[],
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    let events = lines(&output);
    assert_eq!(events[0]["event"], "device_code");
    assert_eq!(events[0]["user_code"], "WDJB-MJHT");
    assert_eq!(events[1]["event"], "slow_down");
    assert_eq!(events[1]["interval"], 6);
    let result = events.last().unwrap();
    assert_eq!(result["authenticated"], true);
    assert_eq!(result["uuid"], "zQo");
    assert_eq!(result["id"], "c:ada");
    assert_eq!(result["kind"], "carbon");
    assert_eq!(result["method"], "device");
    assert!(!stdout(&output).contains("sad_device_secret"));
    assert!(!stdout(&output).contains("sar_device"));

    // The request carried Briefcase's client id, the default label and asked for email.
    let authorize = accounts
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.url.path() == "/v1/device/authorize")
        .unwrap();
    let body: Value = serde_json::from_slice(&authorize.body).unwrap();
    assert_eq!(body["client_id"], "briefcase");
    assert_eq!(body["scope"], "email");
    assert_eq!(body["client_label"], "briefcase CLI on testhost");

    let credentials = home.credentials();
    let session = &credentials["sessions"]["default"];
    assert_eq!(credentials["format"], 2);
    assert_eq!(session["refresh_token"], "sar_device");
    assert_eq!(session["account"]["uuid"], "zQo");
    assert_eq!(session["accounts_url"], accounts.uri());
    assert_eq!(
        home.configuration()["profiles"]["default"]["url"],
        api(&service)
    );
    #[cfg(unix)]
    assert_eq!(common::mode(&home.state().join("credentials.json")), 0o600);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_device_code_is_refused_and_nothing_is_saved() {
    let accounts = MockServer::start().await;
    let service = briefcase_service(&accounts.uri()).await;
    device_authorization(&accounts, 600).await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(oauth_error(400, "access_denied", "Denied."))
        .mount(&accounts)
        .await;
    let home = Home::new();
    let output = briefcase(
        &home,
        &[
            "--json",
            "--url",
            &api(&service),
            "--accounts-url",
            &accounts.uri(),
            "login",
        ],
        &[],
    )
    .await;
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    assert_eq!(
        json_error(&output)["error"]["code"],
        "device_sign_in_denied"
    );
    assert!(!home.state().join("credentials.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_device_code_stops_the_wait() {
    let accounts = MockServer::start().await;
    let service = briefcase_service(&accounts.uri()).await;
    device_authorization(&accounts, 2).await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(oauth_error(
            400,
            "authorization_pending",
            "Not approved yet.",
        ))
        .mount(&accounts)
        .await;
    let home = Home::new();
    let output = briefcase(
        &home,
        &[
            "--url",
            &api(&service),
            "--accounts-url",
            &accounts.uri(),
            "login",
        ],
        &[],
    )
    .await;
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("WDJB-MJHT"), "{text}");
    assert!(text.contains("expired before it was approved"), "{text}");
    assert!(text.contains("hint: Run `briefcase login` again"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_sign_in_falls_back_when_briefcase_does_not_ask_for_email() {
    let accounts = MockServer::start().await;
    let service = briefcase_service(&accounts.uri()).await;
    Mock::given(method("POST"))
        .and(path("/v1/device/authorize"))
        .and(body_string_contains("\"scope\""))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": {
            "code": "invalid_scope",
            "message": "The scope 'email' isn't a detail this app asks for."
        }})))
        .expect(1)
        .with_priority(1)
        .mount(&accounts)
        .await;
    device_authorization(&accounts, 600).await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens("access", "sar_x")))
        .mount(&accounts)
        .await;
    let home = Home::new();
    let output = briefcase(
        &home,
        &[
            "--json",
            "--url",
            &api(&service),
            "--accounts-url",
            &accounts.uri(),
            "login",
        ],
        &[],
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(lines(&output).last().unwrap()["authenticated"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silicon_signs_in_with_a_short_lived_token_on_standard_input() {
    let accounts = MockServer::start().await;
    let service = briefcase_service(&accounts.uri()).await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .and(body_string_contains(
            "grant_type=urn%3Asilicon%3Aparams%3Aoauth%3Agrant-type%3Aslt",
        ))
        .and(body_string_contains("slt=slt_from_stdin_secret"))
        .and(body_string_contains("client_id=briefcase"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens("access-slt", "sar_slt")))
        .expect(1)
        .mount(&accounts)
        .await;
    let home = Home::new();
    let output = briefcase_with_stdin(
        &home,
        &[
            "--json",
            "--url",
            &api(&service),
            "--accounts-url",
            &accounts.uri(),
            "login",
            "--slt-stdin",
        ],
        &[],
        b"slt_from_stdin_secret\n",
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    let result = &lines(&output)[0];
    assert_eq!(result["method"], "short_lived_token");
    assert!(!stdout(&output).contains("slt_from_stdin_secret"));
    assert!(!stderr(&output).contains("slt_from_stdin_secret"));
    let request = accounts
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.url.path() == "/v1/oauth/token")
        .unwrap();
    assert!(!String::from_utf8_lossy(&request.body).contains("client_secret"));
    assert!(!request.headers.contains_key("authorization"));

    // The positional form of the contract works the same way.
    let positional = MockServer::start().await;
    let service = briefcase_service(&positional.uri()).await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .and(body_string_contains("slt=slt_positional"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens("access-p", "sar_p")))
        .expect(1)
        .mount(&positional)
        .await;
    let home = Home::new();
    let output = briefcase(
        &home,
        &[
            "--url",
            &api(&service),
            "--accounts-url",
            &positional.uri(),
            "login",
            "slt_positional",
        ],
        &[],
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Signed in to Briefcase as c:ada (Ada), a Carbon"));
}

#[tokio::test(flavor = "multi_thread")]
async fn every_refused_short_lived_token_says_why_and_how_to_get_a_fresh_one() {
    for (description, code) in [
        (
            "The short-lived token was already used; each one works once.",
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
        let accounts = MockServer::start().await;
        let service = briefcase_service(&accounts.uri()).await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .respond_with(oauth_error(400, "invalid_grant", description))
            .mount(&accounts)
            .await;
        let home = Home::new();
        let output = briefcase_with_stdin(
            &home,
            &[
                "--json",
                "--url",
                &api(&service),
                "--accounts-url",
                &accounts.uri(),
                "login",
                "--slt-stdin",
            ],
            &[],
            b"slt_refused_secret",
        )
        .await;
        assert_eq!(
            output.status.code(),
            Some(4),
            "{description}: {}",
            stderr(&output)
        );
        let error = json_error(&output);
        assert_eq!(error["error"]["code"], code, "{description}");
        assert_eq!(error["error"]["message"], description);
        assert!(
            error["error"]["hint"].as_str().unwrap().contains(
                "silicon-accounts login --app briefcase -q | briefcase login --slt-stdin"
            )
        );
        assert!(!stderr(&output).contains("slt_refused_secret"));
        assert!(!home.state().join("credentials.json").exists());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mismatched_deployment_is_refused_before_the_token_is_spent() {
    let accounts = MockServer::start().await;
    let service = briefcase_service("https://accounts.elsewhere.example").await;
    let home = Home::new();
    let output = briefcase_with_stdin(
        &home,
        &[
            "--json",
            "--url",
            &api(&service),
            "--accounts-url",
            &accounts.uri(),
            "login",
            "--slt-stdin",
        ],
        &[],
        b"slt_unspent",
    )
    .await;
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let error = json_error(&output);
    assert_eq!(error["error"]["code"], "accounts_mismatch");
    assert!(
        error["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("ACCOUNTS_URL=https://accounts.elsewhere.example")
    );
    assert!(accounts.received_requests().await.unwrap().is_empty());

    // A Briefcase from before Silicon Accounts has no /api/v1/accounts.
    let old = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/accounts"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&old)
        .await;
    let output = briefcase_with_stdin(
        &home,
        &[
            "--json",
            "--url",
            &api(&old),
            "--accounts-url",
            &accounts.uri(),
            "login",
            "--slt-stdin",
        ],
        &[],
        b"slt_unspent",
    )
    .await;
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_error(&output)["error"]["code"], "service_not_accounts");
    assert!(accounts.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_in_with_a_token_override_or_an_empty_token_is_refused_locally() {
    let home = Home::new();
    let output = briefcase(
        &home,
        &["login", "slt_x"],
        &[("BRIEFCASE_TOKEN", "eyJ.token")],
    )
    .await;
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("not a way to sign in"));
    let output = briefcase_with_stdin(&home, &["login", "--slt-stdin"], &[], b"  \n").await;
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("was empty"));
}

async fn listing(service: &MockServer, token: &str) {
    Mock::given(method("GET"))
        .and(path("/api/v1/entries"))
        .and(header("authorization", format!("Bearer {token}").as_str()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"items": [], "next_cursor": null})),
        )
        .mount(service)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expiring_sign_in_is_refreshed_and_the_new_pair_saved_before_it_is_used() {
    let accounts = MockServer::start().await;
    let service = briefcase_service(&accounts.uri()).await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=sar_one"))
        .and(body_string_contains("client_id=briefcase"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens("access-two", "sar_two")))
        .expect(1)
        .mount(&accounts)
        .await;
    listing(&service, "access-two").await;
    let home = Home::new();
    home.sign_in(&api(&service), &accounts.uri(), 30);
    let output = briefcase(&home, &["--json", "ls"], &[]).await;
    assert!(output.status.success(), "{}", stderr(&output));
    let session = &home.credentials()["sessions"]["default"];
    assert_eq!(session["refresh_token"], "sar_two");
    assert_eq!(session["access_token"], "access-two");
    // A fresh sign-in is used as it is.
    let output = briefcase(&home, &["--json", "ls"], &[]).await;
    assert!(output.status.success(), "{}", stderr(&output));
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_commands_refresh_once_because_a_used_refresh_token_ends_the_sign_in() {
    let accounts = MockServer::start().await;
    let service = briefcase_service(&accounts.uri()).await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .and(body_string_contains("refresh_token=sar_one"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(tokens("access-two", "sar_two"))
                .set_delay(std::time::Duration::from_millis(300)),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&accounts)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(oauth_error(
            400,
            "invalid_grant",
            "This refresh token was already used once. Presenting a used refresh token revokes the whole sign-in.",
        ))
        .with_priority(2)
        .mount(&accounts)
        .await;
    listing(&service, "access-two").await;
    let home = Home::new();
    home.sign_in(&api(&service), &accounts.uri(), 20);
    // Four processes at once; each runs on its own blocking thread.
    let arguments = ["--json", "ls"];
    let outputs = tokio::join!(
        briefcase(&home, &arguments, &[]),
        briefcase(&home, &arguments, &[]),
        briefcase(&home, &arguments, &[]),
        briefcase(&home, &arguments, &[]),
    );
    for output in [outputs.0, outputs.1, outputs.2, outputs.3] {
        assert!(output.status.success(), "{}", stderr(&output));
    }
    let refreshes = accounts
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| {
            String::from_utf8_lossy(&request.body).contains("grant_type=refresh_token")
        })
        .count();
    assert_eq!(
        refreshes, 1,
        "the refresh token must be presented exactly once"
    );
    assert_eq!(
        home.credentials()["sessions"]["default"]["refresh_token"],
        "sar_two"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_refresh_signs_the_profile_out_and_says_why() {
    let accounts = MockServer::start().await;
    let service = briefcase_service(&accounts.uri()).await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(oauth_error(
            400,
            "invalid_grant",
            "The sign-in was revoked.",
        ))
        .mount(&accounts)
        .await;
    let home = Home::new();
    home.sign_in(&api(&service), &accounts.uri(), 10);
    let output = briefcase(&home, &["--json", "ls"], &[]).await;
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    assert_eq!(json_error(&output)["error"]["code"], "sign_in_ended");
    assert!(
        home.credentials()["sessions"]
            .as_object()
            .unwrap()
            .is_empty()
    );

    let status = briefcase(&home, &["login", "status", "--json"], &[]).await;
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["authenticated"], false);
    assert_eq!(status["reason"], "sign_in_ended");
    assert!(status["message"].as_str().unwrap().contains("c:ada"));
    assert!(status["hint"].as_str().unwrap().contains("briefcase login"));
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_revokes_the_refresh_token_with_the_client_id_and_forgets_the_sign_in() {
    let accounts = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/revoke"))
        .and(body_string_contains("token=sar_one"))
        .and(body_string_contains("token_type_hint=refresh_token"))
        .and(body_string_contains("client_id=briefcase"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"revoked": true})))
        .expect(1)
        .mount(&accounts)
        .await;
    let home = Home::new();
    home.sign_in("http://127.0.0.1:9/api/v1/", &accounts.uri(), 1800);
    let output = briefcase(&home, &["--json", "logout"], &[]).await;
    assert!(output.status.success(), "{}", stderr(&output));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["signed_out"], true);
    assert_eq!(result["revoked"], true);
    assert_eq!(result["account"]["id"], "c:ada");
    assert!(
        home.credentials()["sessions"]
            .as_object()
            .unwrap()
            .is_empty()
    );

    let again = briefcase(&home, &["--json", "logout"], &[]).await;
    assert!(again.status.success());
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(again["signed_out"], false);
    assert_eq!(again["reason"], "not_signed_in");
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_still_forgets_the_sign_in_when_silicon_accounts_cannot_be_reached() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let home = Home::new();
    home.sign_in("http://127.0.0.1:9/api/v1/", &closed, 1800);
    let output = briefcase(&home, &["--json", "logout"], &[]).await;
    assert!(output.status.success(), "{}", stderr(&output));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["signed_out"], true);
    assert_eq!(result["revoked"], false);
    assert!(
        result["warning"]
            .as_str()
            .unwrap()
            .contains("could not be revoked")
    );
    assert!(
        home.credentials()["sessions"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn login_status_verifies_with_briefcase_unless_offline() {
    let accounts = MockServer::start().await;
    let service = briefcase_service(&accounts.uri()).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header("authorization", "Bearer access-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "uuid": "zQo", "id": "c:ada-renamed", "kind": "carbon",
            "display_name": "Ada L.", "pfp_url": null, "custodian": null
        })))
        .expect(1)
        .mount(&service)
        .await;
    let home = Home::new();
    home.sign_in(&api(&service), &accounts.uri(), 1800);

    let offline = briefcase(&home, &["login", "status", "--json", "--offline"], &[]).await;
    assert!(offline.status.success());
    let offline: Value = serde_json::from_slice(&offline.stdout).unwrap();
    assert_eq!(offline["authenticated"], true);
    assert_eq!(offline["verified"], false);
    assert_eq!(offline["id"], "c:ada");

    let verified = briefcase(&home, &["login", "status", "--json"], &[]).await;
    assert!(verified.status.success(), "{}", stderr(&verified));
    let verified: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(verified["authenticated"], true);
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["uuid"], "zQo");
    assert_eq!(verified["id"], "c:ada-renamed");
    assert_eq!(verified["display_name"], "Ada L.");
    for key in ["kind", "expires_at", "refresh_expires_at"] {
        assert!(verified.get(key).is_some(), "{key}");
    }
    // The renamed id is remembered.
    assert_eq!(
        home.credentials()["sessions"]["default"]["account"]["id"],
        "c:ada-renamed"
    );

    let human = briefcase(&home, &["login", "status", "--offline"], &[]).await;
    assert!(human.status.success());
    assert!(stdout(&human).contains("Signed in as c:ada-renamed (Ada L.), a carbon."));
}

#[tokio::test(flavor = "multi_thread")]
async fn login_status_reports_a_sign_in_briefcase_refuses_as_signed_out() {
    let accounts = MockServer::start().await;
    let service = briefcase_service(&accounts.uri()).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": {
            "code": "token_revoked",
            "message": "This sign-in was ended at Silicon Accounts."
        }})))
        .mount(&service)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens("access-two", "sar_two")))
        .mount(&accounts)
        .await;
    let home = Home::new();
    home.sign_in(&api(&service), &accounts.uri(), 1800);
    let output = briefcase(&home, &["login", "status", "--json"], &[]).await;
    assert!(output.status.success());
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["authenticated"], false);
    assert_eq!(status["reason"], "token_revoked");
    // One forced refresh was tried before giving up.
    assert_eq!(accounts.received_requests().await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_from_before_silicon_accounts_reads_as_signed_out_and_never_crashes() {
    let home = Home::new();
    std::fs::create_dir_all(home.state()).unwrap();
    std::fs::write(
        home.state().join("config.json"),
        r#"{"current_profile":"default","profiles":{"default":{"url":"https://backend.briefcase.teamofsilicons.com/api/v1/","org":"tos"}}}"#,
    )
    .unwrap();
    std::fs::write(
        home.state().join("credentials.json"),
        r#"{"sessions":{"default":{"access_token":"oat_old","refresh_token":"ort_old",
            "expires_at":"2026-09-01T00:00:00Z",
            "actor":{"principal_id":"01a067ce-7f19-7790-820a-0be6b3d4f803","type":"carbon","public_id":"c:saket"},
            "org_id":"tos","organizations":["tos"]}},"tokens":{},"pending_mutations":{}}"#,
    )
    .unwrap();
    let status = briefcase(&home, &["login", "status", "--json"], &[]).await;
    assert!(status.status.success(), "{}", stderr(&status));
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["authenticated"], false);
    assert_eq!(status["reason"], "signed_in_before_silicon_accounts");
    assert!(status["message"].as_str().unwrap().contains("c:saket"));

    let listing = briefcase(&home, &["--json", "ls"], &[]).await;
    assert_eq!(listing.status.code(), Some(4));
    assert_eq!(
        json_error(&listing)["error"]["code"],
        "signed_in_before_silicon_accounts"
    );
    assert!(!stderr(&listing).contains("oat_old"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_credentials_file_is_reported_and_set_aside_by_the_next_sign_in() {
    let accounts = MockServer::start().await;
    let service = briefcase_service(&accounts.uri()).await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens("access", "sar_new")))
        .mount(&accounts)
        .await;
    let home = Home::new();
    std::fs::create_dir_all(home.state()).unwrap();
    std::fs::write(home.state().join("credentials.json"), b"{ broken").unwrap();

    let status = briefcase(&home, &["login", "status", "--json"], &[]).await;
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["authenticated"], false);
    assert_eq!(status["reason"], "unreadable_state");

    let output = briefcase_with_stdin(
        &home,
        &[
            "--url",
            &api(&service),
            "--accounts-url",
            &accounts.uri(),
            "login",
            "--slt-stdin",
        ],
        &[],
        b"slt_ok",
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).contains("kept as"));
    let aside: Vec<_> = std::fs::read_dir(home.state())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("credentials.json.unreadable-")
        })
        .collect();
    assert_eq!(aside.len(), 1);
    assert_eq!(std::fs::read(aside[0].path()).unwrap(), b"{ broken");
    assert_eq!(
        home.credentials()["sessions"]["default"]["refresh_token"],
        "sar_new"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_works_signed_out_offline_in_an_empty_home_and_writes_nothing() {
    let home = Home::new();
    let help = briefcase(&home, &["--help"], &[]).await;
    assert!(help.status.success());
    assert!(stdout(&help).contains("briefcase login"));
    assert!(!stdout(&help).contains("--org"));

    let accounts = briefcase(&home, &["accounts", "--json"], &[]).await;
    assert!(accounts.status.success());
    let accounts: Value = serde_json::from_slice(&accounts.stdout).unwrap();
    assert_eq!(
        accounts,
        json!({
            "app_id": "briefcase",
            "name": "Briefcase",
            "client_id": "briefcase",
            "accounts_url": "https://accounts.teamofsilicons.com",
            "api_url": "https://api.briefcase.teamofsilicons.com/api/v1/",
            "web_url": "https://briefcase.teamofsilicons.com",
            "version": env!("CARGO_PKG_VERSION"),
            "contract_version": "4.0.0",
            "api_version": "v1",
            "sign_in": {
                "device_flow": true,
                "public_client": true,
                "carbon": "briefcase login",
                "silicon": "silicon-accounts login --app briefcase -q | briefcase login --slt-stdin"
            },
            "install": "silicon-apps install briefcase",
            "docs": "https://docs.briefcase.teamofsilicons.com/",
            "repository": "https://github.com/teamofsilicons/silicon-briefcase",
            "package": "https://crates.io/crates/briefcase-client"
        })
    );
    let iam = briefcase(&home, &["iam", "--json"], &[]).await;
    assert!(iam.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&iam.stdout).unwrap(),
        accounts
    );

    let status = briefcase(&home, &["login", "status", "--json"], &[]).await;
    assert!(status.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&status.stdout).unwrap(),
        json!({"authenticated": false})
    );
    let human = briefcase(&home, &["login", "status"], &[]).await;
    assert_eq!(human.status.code(), Some(1));
    assert!(stdout(&human).starts_with("Not signed in."));

    // Environment overrides show in the discovery answer.
    let local = briefcase(
        &home,
        &["accounts", "--json"],
        &[
            ("ACCOUNTS_URL", "http://localhost:9590/"),
            ("BRIEFCASE_URL", "http://127.0.0.1:4101/api/v1"),
        ],
    )
    .await;
    let local: Value = serde_json::from_slice(&local.stdout).unwrap();
    assert_eq!(local["accounts_url"], "http://localhost:9590");
    assert_eq!(local["api_url"], "http://127.0.0.1:4101/api/v1/");

    assert!(!home.state().exists(), "discovery must not create state");
    assert_eq!(std::fs::read_dir(home.home()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_saved_sign_in_is_never_sent_to_another_deployment() {
    let accounts = MockServer::start().await;
    let service = briefcase_service(&accounts.uri()).await;
    let other = briefcase_service(&accounts.uri()).await;
    let home = Home::new();
    home.sign_in(&api(&service), &accounts.uri(), 1800);
    let output = briefcase(&home, &["--json", "--url", &api(&other), "ls"], &[]).await;
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert_eq!(
        json_error(&output)["error"]["code"],
        "sign_in_bound_elsewhere"
    );
    assert!(
        other
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| !request.headers.contains_key("authorization"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn retired_options_and_commands_say_what_replaced_them() {
    let home = Home::new();
    for (arguments, extra, code) in [
        (vec!["--json", "--org", "tos", "ls"], vec![], "retired"),
        (
            vec![
                "--json",
                "--test",
                "01a067ce-7f19-7790-820a-0be6b3d4f800",
                "ls",
            ],
            vec![],
            "retired",
        ),
        (
            vec!["--json", "--app-secret", "ask_secret_value_x", "ls"],
            vec![],
            "retired",
        ),
        (
            vec!["--json", "ls"],
            vec![("BRIEFCASE_TEST", "01a067ce-7f19-7790-820a-0be6b3d4f800")],
            "testing_environments_retired",
        ),
        (vec!["--json", "env", "list"], vec![], "retired"),
        (vec!["--json", "system", "update"], vec![], "retired"),
        (vec!["--json", "daemon", "start"], vec![], "retired"),
    ] {
        let output = briefcase(&home, &arguments, &extra).await;
        assert_eq!(
            output.status.code(),
            Some(2),
            "{arguments:?}: {}",
            stderr(&output)
        );
        let error = json_error(&output);
        assert_eq!(error["error"]["code"], code, "{arguments:?}");
        assert!(error["error"]["hint"].as_str().is_some(), "{arguments:?}");
        assert!(!stderr(&output).contains("ask_secret_value_x"));
    }
    // BRIEFCASE_APP_SECRET is the service's secret now; the CLI never reads it.
    let output = briefcase(
        &home,
        &["accounts", "--json"],
        &[("BRIEFCASE_APP_SECRET", "sa_app_secret")],
    )
    .await;
    assert!(output.status.success());
    // BRIEFCASE_ORG only earns a notice.
    let output = briefcase(
        &home,
        &["login", "status", "--json"],
        &[("BRIEFCASE_ORG", "tos")],
    )
    .await;
    assert!(output.status.success());
    assert!(stderr(&output).contains("BRIEFCASE_ORG is ignored"));
    // `daemon run`, what an old login service starts, exits 0.
    let output = briefcase(&home, &["daemon", "run"], &[]).await;
    assert!(output.status.success(), "{}", stderr(&output));
}

#[tokio::test(flavor = "multi_thread")]
async fn argument_errors_never_repeat_a_token() {
    let home = Home::new();
    for arguments in [
        vec!["--token", "sar_secret_refresh", "no-such-command"],
        vec!["login", "slt_secret_value", "extra-argument"],
        vec!["--token=eyJsecret.jwt.value", "ls", "--bogus"],
    ] {
        let output = briefcase(&home, &arguments, &[]).await;
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        let text = format!("{}{}", stdout(&output), stderr(&output));
        for secret in [
            "sar_secret_refresh",
            "slt_secret_value",
            "eyJsecret.jwt.value",
        ] {
            assert!(!text.contains(secret), "{arguments:?}: {text}");
        }
    }
}
