//! Silicon Accounts authentication, the account cache, the custodian rule, the
//! webhook and proofs, exercised through the router against PostgreSQL and a
//! Silicon Accounts stand-in.
//!
//! ```bash
//! BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5460/briefcase_mig \
//!   cargo test --lib api::accounts_tests -- --test-threads=1
//! ```

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tower::ServiceExt as _;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

use super::{
    router,
    state::AppState,
    tests::{TEST_WEBHOOK_SECRET, test_accounts_settings, test_state_with_accounts},
};
use crate::infrastructure::accounts::test_support::{
    TestIssuer, carbon_summary, signed_delivery, silicon_summary, valid_proof,
};

pub(super) struct Fixture {
    pub(super) server: MockServer,
    pub(super) issuer: TestIssuer,
    pub(super) router: Router,
    pub(super) state: AppState,
    pub(super) admin: sqlx::PgPool,
    suffix: String,
}

/// Builds the API against the test database and a Silicon Accounts stand-in,
/// or returns `None` when `BRIEFCASE_TEST_DATABASE_URL` is unset.
pub(super) async fn fixture(proof_issuers: &str) -> anyhow::Result<Option<Fixture>> {
    let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
        eprintln!("skipping: BRIEFCASE_TEST_DATABASE_URL is not set");
        return Ok(None);
    };
    let admin = sqlx::PgPool::connect_with(
        url.parse::<sqlx::postgres::PgConnectOptions>()?
            .options([("search_path", "public")]),
    )
    .await?;
    crate::infrastructure::postgres::migrate(&admin).await?;
    let api = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE briefcase_api")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with((*admin.connect_options()).clone())
        .await?;
    let server = MockServer::start().await;
    let issuer = TestIssuer::new(11, "k-test", &server.uri());
    Mock::given(method("GET"))
        .and(path("/.well-known/jwks.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issuer.jwks_json()))
        .mount(&server)
        .await;
    let settings = test_accounts_settings(&server.uri(), proof_issuers)?;
    let (state, server_settings, webhook) =
        test_state_with_accounts(api, &settings, Some(issuer.jwks()))?;
    let router = router(state.clone(), &server_settings, &webhook);
    Ok(Some(Fixture {
        server,
        issuer,
        router,
        state,
        admin,
        suffix: Uuid::new_v4().simple().to_string()[..12].to_owned(),
    }))
}

impl Fixture {
    /// A uuid unique to this test run, such as `Ada3f2a…`.
    pub(super) fn uuid(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.suffix)
    }

    /// A public id unique to this test run, such as `c:ada-3f2a…`.
    pub(super) fn id(&self, prefix: &str) -> String {
        format!("{prefix}-{}", self.suffix)
    }

    pub(super) async fn send(
        &self,
        method: Method,
        uri: &str,
        authorization: Option<&str>,
        body: Option<Value>,
        extra: &[(&str, &str)],
    ) -> anyhow::Result<(StatusCode, Value)> {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(value) = authorization {
            request = request.header(header::AUTHORIZATION, value);
        }
        for (name, value) in extra {
            request = request.header(*name, *value);
        }
        let request = match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))?,
            None => request.body(Body::empty())?,
        };
        let response = self.router.clone().oneshot(request).await?;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Ok((status, value))
    }

    pub(super) async fn bearer(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let idempotency = Uuid::new_v4().to_string();
        self.send(
            method,
            uri,
            Some(&format!("Bearer {token}")),
            body,
            &[("idempotency-key", idempotency.as_str())],
        )
        .await
    }

    /// Delivers a signed webhook event and returns the status.
    pub(super) async fn deliver(
        &self,
        event_type: &str,
        occurred_at: OffsetDateTime,
        data: &Value,
    ) -> anyhow::Result<StatusCode> {
        let event_id = Uuid::now_v7().to_string();
        self.deliver_event(&event_id, event_type, occurred_at, data)
            .await
    }

    pub(super) async fn deliver_event(
        &self,
        event_id: &str,
        event_type: &str,
        occurred_at: OffsetDateTime,
        data: &Value,
    ) -> anyhow::Result<StatusCode> {
        let delivery =
            signed_delivery(TEST_WEBHOOK_SECRET, event_id, event_type, occurred_at, data);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/webhook/")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-accounts-timestamp", delivery.timestamp.as_str())
            .header("x-accounts-signature", delivery.signature.as_str())
            .header("x-accounts-event-id", delivery.event_id.as_str())
            .header("x-accounts-event-type", event_type)
            .body(Body::from(delivery.body))?;
        Ok(self.router.clone().oneshot(request).await?.status())
    }

    /// Serves `GET /v1/userinfo` for a Carbon.
    pub(super) async fn stub_userinfo(&self, uuid: &str, id: &str, name: &str) {
        Mock::given(method("GET"))
            .and(path("/v1/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "uuid": uuid, "membership_id": format!("briefcase:{uuid}"), "kind": "carbon",
                "id": id, "display_name": name, "pfp_url": "https://pfp.example/a.png",
                "version": 4, "sub": uuid,
            })))
            .mount(&self.server)
            .await;
    }

    /// Serves `POST /v1/oauth/introspect` with one answer.
    pub(super) async fn stub_introspection(&self, active: bool, subject: &str) {
        Mock::given(method("POST"))
            .and(path("/v1/oauth/introspect"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "active": active, "sub": subject })),
            )
            .mount(&self.server)
            .await;
    }

    /// Serves `GET /v1/accounts/by-id/{id}` and `GET /v1/accounts/{uuid}`.
    pub(super) async fn stub_account(&self, uuid: &str, id: &str, summary: Value) {
        Mock::given(method("GET"))
            .and(path(format!("/v1/accounts/by-id/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(summary.clone()))
            .mount(&self.server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/v1/accounts/{uuid}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(summary))
            .mount(&self.server)
            .await;
    }

    pub(super) async fn me(&self, token: &str) -> anyhow::Result<(StatusCode, Value)> {
        self.bearer(Method::GET, "/api/v1/me", token, None).await
    }

    pub(super) async fn account_row(&self, uuid: &str) -> anyhow::Result<Value> {
        let row: Option<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(account) FROM briefcase.accounts AS account WHERE account_uuid = $1",
        )
        .bind(uuid)
        .fetch_optional(&self.admin)
        .await?;
        Ok(row.unwrap_or(Value::Null))
    }
}

fn error_code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn me_verifies_access_tokens_and_records_the_account() -> anyhow::Result<()> {
    let Some(f) = fixture("").await? else {
        return Ok(());
    };
    let (ada, ada_id) = (f.uuid("Ada"), f.id("c:ada"));
    f.stub_userinfo(&ada, &ada_id, "Ada Lovelace").await;

    let token = f.issuer.token(&ada, "carbon", &ada_id);
    let (status, body) = f.bearer(Method::GET, "/api/v1/me", &token, None).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["uuid"], ada.as_str());
    assert_eq!(body["id"], ada_id.as_str());
    assert_eq!(body["kind"], "carbon");
    assert_eq!(body["display_name"], "Ada Lovelace");
    assert!(body["custodian"].is_null());
    let row = f.account_row(&ada).await?;
    assert_eq!(row["kind"], "carbon");
    assert_eq!(row["public_id"], ada_id.as_str());
    assert_eq!(row["status"], "active");

    for (claims_change, code) in [
        (json!({"aud": "remind"}), "token_wrong_audience"),
        (
            json!({"iss": "https://accounts.example"}),
            "token_wrong_issuer",
        ),
        (json!({"exp": 1_000_000_000_i64}), "token_expired"),
        (json!({"kind": null}), "token_missing_kind"),
        (json!({"sub": "not a uuid"}), "token_invalid_subject"),
    ] {
        let mut claims = f.issuer.claims(&ada, "carbon", &ada_id, 0, 1_800);
        if let (Some(target), Some(change)) = (claims.as_object_mut(), claims_change.as_object()) {
            for (key, value) in change {
                target.insert(key.clone(), value.clone());
            }
        }
        let (status, body) = f
            .bearer(Method::GET, "/api/v1/me", &f.issuer.sign(&claims), None)
            .await?;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{code}: {body}");
        assert_eq!(error_code(&body), code, "{body}");
        assert!(
            !body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        );
    }
    Ok(())
}

#[tokio::test]
async fn an_access_token_works_in_the_accounts_own_drive() -> anyhow::Result<()> {
    let Some(f) = fixture("").await? else {
        return Ok(());
    };
    let (ada, ada_id) = (f.uuid("Ada"), f.id("c:ada"));
    let token = f.issuer.token(&ada, "carbon", &ada_id);
    let home = format!("private/{ada}");
    let (status, body) = f
        .bearer(
            Method::POST,
            "/api/v1/entries",
            &token,
            Some(json!({ "name": "notes", "parent_path": home })),
        )
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["path"], format!("{home}/notes"));
    let (status, listing) = f
        .bearer(
            Method::GET,
            &format!("/api/v1/entries?path={home}"),
            &token,
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert!(
        listing["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["name"] == "notes"))
    );
    // The tenant is the account itself: another account never sees it.
    let bob = f.uuid("Bob");
    let other = f.issuer.token(&bob, "carbon", &f.id("c:bob"));
    let (status, _) = f
        .bearer(
            Method::GET,
            &format!("/api/v1/entries?path={home}/notes"),
            &other,
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn sharing_resolves_ids_through_accounts_and_sensitive_changes_introspect()
-> anyhow::Result<()> {
    let Some(f) = fixture("").await? else {
        return Ok(());
    };
    let (ada, ada_id) = (f.uuid("Ada"), f.id("c:ada"));
    let (bob, bob_id) = (f.uuid("Bob"), f.id("c:bob"));
    f.stub_account(&bob, &bob_id, carbon_summary(&bob, &bob_id))
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v1/accounts/by-id/{}", f.id("c:nobody"))))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": { "code": "account_not_found", "message": "No account has that id." }
        })))
        .mount(&f.server)
        .await;
    let token = f.issuer.token(&ada, "carbon", &ada_id);
    let (status, folder) = f
        .bearer(
            Method::POST,
            "/api/v1/entries",
            &token,
            Some(json!({ "name": "shared", "parent_path": format!("private/{ada}") })),
        )
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{folder}");
    let entry = folder["id"].as_str().unwrap_or_default().to_owned();
    let invitations = format!("/api/v1/entries/{entry}/invitations");

    // Sharing is sensitive: a token Silicon Accounts no longer considers
    // active is refused even though it still verifies locally.
    f.stub_introspection(false, &ada).await;
    let (status, body) = f
        .bearer(
            Method::POST,
            &invitations,
            &token,
            Some(json!({ "principal": { "type": "carbon", "id": bob_id } })),
        )
        .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(error_code(&body), "token_revoked");

    f.server.reset().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/jwks.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(f.issuer.jwks_json()))
        .mount(&f.server)
        .await;
    f.stub_account(&bob, &bob_id, carbon_summary(&bob, &bob_id))
        .await;
    f.stub_introspection(true, &ada).await;
    // A fresh token, so the earlier inactive answer is not reused.
    let token = f
        .issuer
        .sign(&f.issuer.claims(&ada, "carbon", &ada_id, 1, 1_800));
    let (status, grant) = f
        .bearer(
            Method::POST,
            &invitations,
            &token,
            Some(json!({ "principal": { "type": "carbon", "id": bob_id.to_uppercase() } })),
        )
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    assert_eq!(grant["principal"]["id"], bob.as_str(), "stored by uuid");

    for (principal, code) in [
        (
            json!({ "type": "carbon", "id": f.id("c:nobody") }),
            "unknown_account",
        ),
        (
            json!({ "type": "silicon", "id": bob_id }),
            "principal_kind_mismatch",
        ),
        (
            json!({ "type": "tag", "id": "research" }),
            "tag_recipients_retired",
        ),
        (
            json!({ "type": "email", "id": "bob@example.test" }),
            "email_recipients_retired",
        ),
        (
            json!({ "type": "carbon", "id": "bob smith" }),
            "invalid_account_id",
        ),
    ] {
        let (status, body) = f
            .bearer(
                Method::POST,
                &invitations,
                &token,
                Some(json!({ "principal": principal })),
            )
            .await?;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{code}: {body}");
        assert_eq!(error_code(&body), code, "{body}");
    }
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn the_webhook_verifies_signatures_and_applies_each_event_once() -> anyhow::Result<()> {
    let Some(f) = fixture("").await? else {
        return Ok(());
    };
    let now = OffsetDateTime::now_utc();
    // No signature headers, a wrong signature, and a stale timestamp: 401.
    let (status, body) = f
        .send(
            Method::POST,
            "/webhook/",
            None,
            Some(json!({"type": "ping"})),
            &[],
        )
        .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(&body), "webhook_signature_missing");
    let delivery = signed_delivery(
        "whsec_some_other_secret_0123456789",
        "e-1",
        "ping",
        now,
        &json!({}),
    );
    let (status, body) = f
        .send(
            Method::POST,
            "/webhook/",
            None,
            Some(serde_json::from_str(&delivery.body)?),
            &[
                ("x-accounts-timestamp", delivery.timestamp.as_str()),
                ("x-accounts-signature", delivery.signature.as_str()),
            ],
        )
        .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(&body), "webhook_signature_invalid");
    let stale = (now - time::Duration::minutes(10)).unix_timestamp();
    let body_text = json!({"event_id": "e-2", "type": "ping", "data": {}}).to_string();
    let signature =
        silicon_accounts_client::sign_webhook(TEST_WEBHOOK_SECRET, stale, body_text.as_bytes());
    let request = Request::builder()
        .method(Method::POST)
        .uri("/webhook/")
        .header("x-accounts-timestamp", stale.to_string())
        .header("x-accounts-signature", signature)
        .body(Body::from(body_text))?;
    assert_eq!(
        f.router.clone().oneshot(request).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    // A correctly signed body that is not an event: 400.
    let timestamp = now.unix_timestamp();
    let garbage = b"{\"not\":\"an event\"}";
    let request = Request::builder()
        .method(Method::POST)
        .uri("/webhook/")
        .header("x-accounts-timestamp", timestamp.to_string())
        .header(
            "x-accounts-signature",
            silicon_accounts_client::sign_webhook(TEST_WEBHOOK_SECRET, timestamp, garbage),
        )
        .body(Body::from(&garbage[..]))?;
    assert_eq!(
        f.router.clone().oneshot(request).await?.status(),
        StatusCode::BAD_REQUEST
    );

    // ping and an unknown type are acknowledged and recorded as ignored.
    let ping = Uuid::now_v7().to_string();
    assert_eq!(
        f.deliver_event(&ping, "ping", now, &json!({})).await?,
        StatusCode::NO_CONTENT
    );
    let unknown = Uuid::now_v7().to_string();
    assert_eq!(
        f.deliver_event(
            &unknown,
            "account.something_new",
            now,
            &json!({"uuid": "Zz9"})
        )
        .await?,
        StatusCode::NO_CONTENT
    );
    // The same event delivered twice is applied once.
    let (ada, ada_id) = (f.uuid("Ada"), f.id("c:ada"));
    let renamed = f.id("c:ada-king");
    let changed = Uuid::now_v7().to_string();
    let data = json!({"uuid": ada, "membership_id": format!("briefcase:{ada}"), "kind": "carbon", "old_id": ada_id, "new_id": renamed});
    assert_eq!(
        f.deliver_event(&changed, "account.id_changed", now, &data)
            .await?,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.deliver_event(&changed, "account.id_changed", now, &data)
            .await?,
        StatusCode::NO_CONTENT
    );
    let outcomes: Vec<(String, String)> = sqlx::query_as(
        "SELECT event_id, outcome FROM briefcase.accounts_webhook_receipts \
          WHERE event_id = ANY($1) ORDER BY event_id",
    )
    .bind(vec![ping.clone(), unknown.clone(), changed.clone()])
    .fetch_all(&f.admin)
    .await?;
    let outcome = |id: &str| {
        outcomes
            .iter()
            .find(|(event, _)| event == id)
            .map(|(_, outcome)| outcome.clone())
    };
    assert_eq!(outcomes.len(), 3, "{outcomes:?}");
    assert_eq!(outcome(&ping).as_deref(), Some("ignored"));
    assert_eq!(outcome(&unknown).as_deref(), Some("ignored"));
    assert_eq!(outcome(&changed).as_deref(), Some("applied"));
    assert_eq!(f.account_row(&ada).await?["public_id"], renamed.as_str());
    Ok(())
}

#[tokio::test]
async fn sign_outs_removals_and_deletion_refuse_older_tokens() -> anyhow::Result<()> {
    let Some(f) = fixture("").await? else {
        return Ok(());
    };
    let (ada, ada_id) = (f.uuid("Ada"), f.id("c:ada"));
    f.stub_userinfo(&ada, &ada_id, "Ada").await;
    let membership = json!({"uuid": ada, "membership_id": format!("briefcase:{ada}")});
    let at = |offset: i64| {
        f.issuer
            .sign(&f.issuer.claims(&ada, "carbon", &ada_id, offset, 3_600))
    };
    let early = at(-120);
    assert_eq!(f.me(&early).await?.0, StatusCode::OK);

    // Briefcase's own sign-out of one session leaves the others alone.
    let now = OffsetDateTime::now_utc();
    let mut signed_out = membership.clone();
    signed_out["reason"] = json!("app_revoked");
    assert_eq!(
        f.deliver(
            "membership.signed_out",
            now - time::Duration::seconds(60),
            &signed_out
        )
        .await?,
        StatusCode::NO_CONTENT
    );
    assert_eq!(f.me(&early).await?.0, StatusCode::OK);

    // Any other sign-out ends every token issued before it.
    signed_out["reason"] = json!("stk_rotated");
    assert_eq!(
        f.deliver(
            "membership.signed_out",
            now - time::Duration::seconds(60),
            &signed_out
        )
        .await?,
        StatusCode::NO_CONTENT
    );
    let (status, body) = f.me(&early).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(&body), "token_revoked");
    assert_eq!(
        f.me(&at(-30)).await?.0,
        StatusCode::OK,
        "a later sign-in works"
    );

    // Removing Briefcase's access refuses older tokens until a new sign-in.
    assert_eq!(
        f.deliver("membership.access_removed", now, &membership)
            .await?,
        StatusCode::NO_CONTENT
    );
    assert_eq!(f.account_row(&ada).await?["status"], "access_removed");
    assert_eq!(f.me(&at(-30)).await?.0, StatusCode::UNAUTHORIZED);
    assert_eq!(f.me(&at(5)).await?.0, StatusCode::OK);
    assert_eq!(f.account_row(&ada).await?["status"], "active");

    // A deleted account is refused whatever its token, and forgotten.
    assert_eq!(
        f.deliver(
            "account.deleted",
            now + time::Duration::seconds(10),
            &membership
        )
        .await?,
        StatusCode::NO_CONTENT
    );
    // Issued after the deletion (and within the 30-second clock leeway).
    let (status, body) = f.me(&at(20)).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(&body), "account_deleted");
    let row = f.account_row(&ada).await?;
    assert_eq!(row["status"], "deleted");
    assert!(row["display_name"].is_null() && row["email"].is_null());
    Ok(())
}

#[tokio::test]
async fn profile_events_keep_the_newest_state_whatever_the_order() -> anyhow::Result<()> {
    let Some(f) = fixture("").await? else {
        return Ok(());
    };
    let (ada, first, second) = (f.uuid("Ada"), f.id("c:ada"), f.id("c:ada-two"));
    let now = OffsetDateTime::now_utc();
    let changed = |old: &str, new: &str| json!({"uuid": ada, "membership_id": format!("briefcase:{ada}"), "kind": "carbon", "old_id": old, "new_id": new});
    // The newer rename arrives first; the older one must not undo it.
    f.deliver("account.id_changed", now, &changed(&first, &second))
        .await?;
    f.deliver(
        "account.id_changed",
        now - time::Duration::minutes(5),
        &changed("c:ada-zero", &first),
    )
    .await?;
    assert_eq!(f.account_row(&ada).await?["public_id"], second.as_str());
    let history: Vec<String> = sqlx::query_scalar(
        "SELECT public_id FROM briefcase.account_id_history WHERE account_uuid = $1 ORDER BY public_id",
    )
    .bind(&ada)
    .fetch_all(&f.admin)
    .await?;
    let mut expected = vec!["c:ada-zero".to_owned(), first.clone()];
    expected.sort();
    assert_eq!(history, expected, "byte order (COLLATE \"C\")");

    let updated = |version: i64, name: &str| {
        json!({"uuid": ada, "membership_id": format!("briefcase:{ada}"), "changed": ["display_name"],
               "account": {"uuid": ada, "membership_id": format!("briefcase:{ada}"), "kind": "carbon",
                           "id": second, "display_name": name, "pfp_url": "https://pfp.example/x.png", "version": version}})
    };
    f.deliver("account.updated", now, &updated(3, "Ada v3"))
        .await?;
    f.deliver("account.updated", now, &updated(2, "Ada v2"))
        .await?;
    let row = f.account_row(&ada).await?;
    assert_eq!(row["display_name"], "Ada v3");
    assert_eq!(row["profile_version"], 3);
    Ok(())
}

#[tokio::test]
async fn custodians_come_from_lookups_and_webhooks_and_define_the_circle() -> anyhow::Result<()> {
    let Some(f) = fixture("").await? else {
        return Ok(());
    };
    let (ada, ada_id) = (f.uuid("Ada"), f.id("c:ada"));
    let (bob, bob_id) = (f.uuid("Bob"), f.id("c:bob"));
    let (scout, scout_id) = (f.uuid("Sct"), f.id("si:scout"));
    let (pilot, pilot_id) = (f.uuid("Plt"), f.id("si:pilot"));
    f.stub_account(
        &scout,
        &scout_id,
        silicon_summary(&scout, &scout_id, (&ada, &ada_id)),
    )
    .await;
    f.stub_account(
        &pilot,
        &pilot_id,
        silicon_summary(&pilot, &pilot_id, (&ada, &ada_id)),
    )
    .await;
    f.stub_account(&ada, &ada_id, carbon_summary(&ada, &ada_id))
        .await;
    f.stub_account(&bob, &bob_id, carbon_summary(&bob, &bob_id))
        .await;
    f.stub_userinfo(&scout, &scout_id, "Scout").await;

    // A Silicon's first sign-in looks its custodian up.
    let token = f.issuer.token(&scout, "silicon", &scout_id);
    let (status, body) = f.bearer(Method::GET, "/api/v1/me", &token, None).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["custodian"]["uuid"], ada.as_str());
    assert_eq!(f.account_row(&scout).await?["custodian_uuid"], ada.as_str());

    let directory = &f.state.accounts;
    let uuid = |value: &str| crate::domain::account::AccountUuid::new(value);
    assert!(directory.same_circle(&uuid(&scout)?, &uuid(&ada)?).await?);
    assert!(
        directory
            .same_circle(&uuid(&scout)?, &uuid(&pilot)?)
            .await?
    );
    assert!(!directory.same_circle(&uuid(&ada)?, &uuid(&bob)?).await?);
    assert!(
        directory
            .is_custodian_of(&uuid(&ada)?, &uuid(&scout)?)
            .await?
    );
    assert_eq!(directory.custodial_silicons(&uuid(&ada)?).await?.len(), 2);

    // A transfer moves the Silicon into its new custodian's circle.
    let transfer = json!({"uuid": scout, "membership_id": format!("briefcase:{scout}"),
                          "from": {"uuid": ada, "id": ada_id}, "to": {"uuid": bob, "id": bob_id}});
    assert_eq!(
        f.deliver(
            "silicon.custodian_changed",
            OffsetDateTime::now_utc(),
            &transfer
        )
        .await?,
        StatusCode::NO_CONTENT
    );
    assert!(
        directory
            .is_custodian_of(&uuid(&bob)?, &uuid(&scout)?)
            .await?
    );
    assert!(
        !directory
            .is_custodian_of(&uuid(&ada)?, &uuid(&scout)?)
            .await?
    );
    assert!(
        !directory
            .same_circle(&uuid(&scout)?, &uuid(&pilot)?)
            .await?
    );
    assert!(directory.same_circle(&uuid(&scout)?, &uuid(&bob)?).await?);
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn proofs_act_for_the_account_inside_the_issuing_apps_folder() -> anyhow::Result<()> {
    let Some(f) =
        fixture("briefcase.folders.create=interface briefcase.entries.list=interface|waveform")
            .await?
    else {
        return Ok(());
    };
    let (ada, ada_id) = (f.uuid("Ada"), f.id("c:ada"));
    let user = (ada.as_str(), ada_id.as_str(), "carbon");
    let proofs = [
        (
            "sap_interface_ok",
            valid_proof(
                "interface",
                "briefcase",
                user,
                &["briefcase.folders.create", "briefcase.entries.list"],
            ),
        ),
        (
            "sap_other_receiver",
            valid_proof("interface", "remind", user, &["briefcase.folders.create"]),
        ),
        (
            "sap_read_only",
            valid_proof("interface", "briefcase", user, &["briefcase.files.read"]),
        ),
        (
            "sap_extend",
            valid_proof("extend", "briefcase", user, &["briefcase.folders.create"]),
        ),
        ("sap_revoked", json!({"valid": false, "expires_at": null})),
    ];
    for (token, answer) in &proofs {
        Mock::given(method("POST"))
            .and(path("/v1/proofs/verify"))
            .and(body_string_contains(*token))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer.clone()))
            .mount(&f.server)
            .await;
    }
    let mut app_proof = valid_proof(
        "interface",
        "briefcase",
        user,
        &["briefcase.folders.create"],
    );
    app_proof["kind"] = json!("app_verification");
    app_proof["user"] = Value::Null;
    Mock::given(method("POST"))
        .and(path("/v1/proofs/verify"))
        .and(body_string_contains("sap_app_level"))
        .respond_with(ResponseTemplate::new(200).set_body_json(app_proof))
        .mount(&f.server)
        .await;

    let create =
        |name: &str| json!({"operation_id": Uuid::now_v7(), "parent_path": "", "name": name});
    let proof = |token: &str| format!("Proof {token}");
    let (status, folder) = f
        .send(
            Method::POST,
            "/api/v1/obo/folders/create",
            Some(&proof("sap_interface_ok")),
            Some(create("reports")),
            &[],
        )
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{folder}");
    assert!(
        folder["path"]
            .as_str()
            .is_some_and(|path| path.starts_with("apps/interface/")),
        "{folder}"
    );
    let (status, listing) = f
        .send(
            Method::POST,
            "/api/v1/obo/entries/list",
            Some(&proof("sap_interface_ok")),
            Some(json!({})),
            &[],
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert!(
        listing["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["name"] == "reports"))
    );

    for (token, status, code) in [
        ("sap_revoked", StatusCode::UNAUTHORIZED, "proof_invalid"),
        (
            "sap_other_receiver",
            StatusCode::UNAUTHORIZED,
            "proof_invalid",
        ),
        (
            "sap_read_only",
            StatusCode::FORBIDDEN,
            "proof_scope_missing",
        ),
        (
            "sap_extend",
            StatusCode::FORBIDDEN,
            "proof_issuer_not_allowed",
        ),
        (
            "sap_app_level",
            StatusCode::FORBIDDEN,
            "proof_kind_not_accepted",
        ),
    ] {
        let (actual, body) = f
            .send(
                Method::POST,
                "/api/v1/obo/folders/create",
                Some(&proof(token)),
                Some(create("x")),
                &[],
            )
            .await?;
        assert_eq!(actual, status, "{token}: {body}");
        assert_eq!(error_code(&body), code, "{token}: {body}");
    }
    // Each kind of route takes exactly its own credential.
    let bearer = format!("Bearer {}", f.issuer.token(&ada, "carbon", &ada_id));
    let (status, body) = f
        .send(
            Method::POST,
            "/api/v1/obo/folders/create",
            Some(&bearer),
            Some(create("x")),
            &[],
        )
        .await?;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::UNAUTHORIZED, "proof_required")
    );
    let (status, body) = f
        .send(
            Method::GET,
            "/api/v1/me",
            Some(&proof("sap_interface_ok")),
            None,
            &[],
        )
        .await?;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::UNAUTHORIZED, "proof_not_accepted_here")
    );

    // Once the account removes Briefcase's access, no app acts for it here.
    let membership = json!({"uuid": ada, "membership_id": format!("briefcase:{ada}")});
    f.deliver(
        "membership.access_removed",
        OffsetDateTime::now_utc(),
        &membership,
    )
    .await?;
    let (status, body) = f
        .send(
            Method::POST,
            "/api/v1/obo/entries/list",
            Some(&proof("sap_interface_ok")),
            Some(json!({})),
            &[],
        )
        .await?;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::FORBIDDEN, "account_access_removed")
    );
    Ok(())
}
