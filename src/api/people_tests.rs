//! Real PostgreSQL/RLS and router checks; external Accounts/Contacts are loopback fixtures.
use super::drives_tests::World;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, header, method, path},
};

#[tokio::test]
#[allow(clippy::too_many_lines)] // One recipient through restriction, add, remove, outage and reopening.
async fn restricted_invitations_follow_the_recipients_live_contacts() -> anyhow::Result<()> {
    let Some(mut w) =
        World::with_delegation("briefcase.invitations.create=interface", "interface").await?
    else {
        return Ok(());
    };
    let contacts = MockServer::start().await;
    w.f.with_contacts(&contacts.uri())?;
    let folder = w.folder(&w.bob, "incoming", None, None).await?;
    let settings = format!("/api/v1/accounts/{}/incoming-shares", w.scout.id);
    let allowance = format!(
        "/api/v1/silicons/{}/allowed-accounts/{}",
        w.scout.id, w.bob.id
    );
    assert_eq!(
        w.call(&w.scout, Method::PUT, &allowance, None).await?.0,
        StatusCode::CREATED
    );
    let (status, saved) = w
        .call(
            &w.ada,
            Method::PUT,
            &settings,
            Some(json!({"allow_from_anyone":false})),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        w.call(&w.scout, Method::GET, &settings, None).await?.1,
        json!({"allow_from_anyone":false})
    );
    assert_eq!(
        w.call(&w.scout, Method::PUT, &allowance, None).await?.0,
        StatusCode::FORBIDDEN
    );

    Mock::given(method("POST")).and(path("/v1/proofs/app-verification"))
        .and(body_partial_json(json!({"receiving_app":"silicon-contacts", "scopes":["briefcase.share.check",format!("recipient:{}",w.scout.uuid),format!("sender:{}",w.bob.uuid)]})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"proof_token":"sap_bound_pair"})))
        .mount(&w.f.server).await;
    let pair_check = || {
        Mock::given(method("POST"))
            .and(path("/v1/integrations/briefcase/share-check"))
            .and(header("authorization", "Proof sap_bound_pair"))
            .and(body_partial_json(
                json!({"recipient":w.scout.uuid,"sender":w.bob.uuid}),
            ))
    };
    let invite =
        json!({"principal":{"type":"silicon","id":w.scout.id},"access":["read"],"inherit":true});
    let restricted = "incoming_shares_restricted";
    {
        let _unsaved = pair_check()
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"saved":false})))
            .mount_as_scoped(&contacts)
            .await;
        for (url, body) in [
            (
                format!("/api/v1/entries/{folder}/invitations"),
                invite.clone(),
            ),
            (
                format!("/api/v1/entries/{folder}/permissions"),
                invite.clone(),
            ),
            (
                "/api/v1/entries".into(),
                json!({"name":"must-not-exist","invitees":[invite]}),
            ),
        ] {
            let (status, error) = w.call(&w.bob, Method::POST, &url, Some(body)).await?;
            assert_eq!(status, StatusCode::FORBIDDEN, "{url}: {error}");
            assert_eq!(error["error"]["code"], restricted);
        }
        let scope = "briefcase.invitations.create";
        Mock::given(method("POST"))
            .and(path("/v1/proofs/verify"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                crate::infrastructure::accounts::test_support::valid_proof(
                    "interface",
                    "briefcase",
                    (&w.bob.uuid, &w.bob.id, "carbon"),
                    &[scope],
                ),
            ))
            .mount(&w.f.server)
            .await;
        let (status, error) = w.f.send(Method::POST, "/api/v1/obo/invitations", Some("Proof sap_delegated_check"),
            Some(json!({"operation_id":uuid::Uuid::now_v7(),"entry_id":folder,"invitation":invite})), &[]).await?;
        assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
        assert_eq!(error["error"]["code"], restricted);
    }
    {
        let _saved = pair_check()
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"saved":true})))
            .mount_as_scoped(&contacts)
            .await;
        let (status, grant) = w
            .share(&w.bob, &folder, "silicon", &w.scout.id, &["read"])
            .await?;
        assert_eq!(status, StatusCode::CREATED, "{grant}");
    }
    let next = w
        .folder(&w.bob, "after-contact-removal", None, None)
        .await?;
    {
        let _removed = pair_check()
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"saved":false})))
            .mount_as_scoped(&contacts)
            .await;
        assert_eq!(
            w.share(&w.bob, &next, "silicon", &w.scout.id, &["read"])
                .await?
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.call(
                &w.scout,
                Method::GET,
                &format!("/api/v1/entries/{folder}"),
                None
            )
            .await?
            .0,
            StatusCode::OK
        );
    }
    let _offline = pair_check()
        .respond_with(ResponseTemplate::new(503))
        .mount_as_scoped(&contacts)
        .await;
    assert_eq!(
        w.share(&w.bob, &next, "silicon", &w.scout.id, &["read"])
            .await?
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        w.call(
            &w.scout,
            Method::PUT,
            &settings,
            Some(json!({"allow_from_anyone":true}))
        )
        .await?
        .0,
        StatusCode::OK
    );
    assert_eq!(
        w.share(&w.bob, &next, "silicon", &w.scout.id, &["read"])
            .await?
            .0,
        StatusCode::CREATED
    );
    Ok(())
}

#[tokio::test]
async fn each_recipient_controls_its_own_setting_and_accepts_its_custodian_circle()
-> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let restrict = Some(json!({"allow_from_anyone": false}));
    for owner in [&w.ada, &w.scout] {
        let url = format!("/api/v1/accounts/{}/incoming-shares", owner.uuid);
        let (status, value) = w.call(owner, Method::GET, &url, None).await?;
        assert_eq!(status, StatusCode::OK, "{value}");
        assert_eq!(value["allow_from_anyone"], true);
        assert_eq!(
            w.call(&w.bob, Method::GET, &url, None).await?.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.call(&w.bob, Method::PUT, &url, restrict.clone()).await?.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            w.call(owner, Method::PUT, &url, restrict.clone()).await?.0,
            StatusCode::OK
        );
    }
    let scout_folder = w.folder(&w.scout, "for-family", None, None).await?;
    let ada_folder = w.folder(&w.ada, "for-family", None, None).await?;
    assert_eq!(
        w.share(&w.scout, &scout_folder, "carbon", &w.ada.id, &["read"])
            .await?
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        w.share(&w.ada, &ada_folder, "silicon", &w.scout.id, &["read"])
            .await?
            .0,
        StatusCode::CREATED
    );
    let bob_folder = w.folder(&w.bob, "for-carbon", None, None).await?;
    // No Contacts gateway is configured. Restriction must not fall back to the Carbon's open default.
    assert_eq!(
        w.share(&w.bob, &bob_folder, "carbon", &w.ada.id, &["read"])
            .await?
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let sibling_id = w.f.id("si:sibling");
    let sibling_uuid = w.f.uuid("Sib");
    let sibling = super::drives_tests::Account {
        id: sibling_id.clone(),
        uuid: sibling_uuid.clone(),
        token: w.f.issuer.token(&sibling_uuid, "silicon", &sibling_id),
    };
    w.f.stub_account(
        &sibling_uuid,
        &sibling_id,
        crate::infrastructure::accounts::test_support::silicon_summary(
            &sibling_uuid,
            &sibling_id,
            (&w.ada.uuid, &w.ada.id),
        ),
    )
    .await;
    let sibling_folder = w.folder(&sibling, "sibling", None, None).await?;
    assert_eq!(
        w.share(&sibling, &sibling_folder, "silicon", &w.scout.id, &["read"])
            .await?
            .0,
        StatusCode::CREATED
    );
    let url = format!("/api/v1/accounts/{}/incoming-shares", w.scout.uuid);
    assert_eq!(
        w.call(
            &sibling,
            Method::PUT,
            &url,
            Some(json!({"allow_from_anyone":true}))
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    Ok(())
}

#[tokio::test]
async fn invitation_history_is_directional_private_and_retry_safe() -> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let folder = w.folder(&w.ada, "history", None, None).await?;
    let body =
        json!({"principal":{"type":"carbon","id":w.bob.id},"access":["read"],"inherit":true});
    let key = uuid::Uuid::new_v4().to_string();
    for _ in 0..2 {
        let (status, value) =
            w.f.send(
                Method::POST,
                &format!("/api/v1/entries/{folder}/invitations"),
                Some(&format!("Bearer {}", w.ada.token)),
                Some(body.clone()),
                &[("idempotency-key", &key)],
            )
            .await?;
        assert_eq!(status, StatusCode::CREATED, "{value}");
    }
    let other = w.folder(&w.bob, "return-share", None, None).await?;
    assert_eq!(
        w.share(&w.bob, &other, "carbon", &w.ada.id, &["read"])
            .await?
            .0,
        StatusCode::CREATED
    );
    // Refused attempts must not count.
    assert_ne!(
        w.share(&w.cat, &folder, "carbon", &w.bob.id, &["read"])
            .await?
            .0,
        StatusCode::CREATED
    );
    let (status, people) = w
        .call(&w.ada, Method::GET, "/api/v1/recipients", None)
        .await?;
    assert_eq!(status, StatusCode::OK, "{people}");
    let peer = &people["items"][0];
    assert_eq!(peer["uuid"], w.bob.uuid);
    assert_eq!(peer["invited"], 1);
    assert_eq!(peer["received"], 1);
    assert_eq!(people["contacts_status"], "disconnected");
    let (_, outsider) = w
        .call(&w.cat, Method::GET, "/api/v1/recipients", None)
        .await?;
    assert_eq!(outsider["items"], json!([]));
    // Runtime-role SQL cannot see another account's history even if explicitly queried.
    let mut tx = w.f.state.database.begin().await?;
    sqlx::query("SELECT set_config('briefcase.actor_id',$1,true)")
        .bind(&w.cat.uuid)
        .execute(&mut *tx)
        .await?;
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM briefcase.invite_history WHERE sender=$1")
            .bind(&w.ada.uuid)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(rows, 0);
    tx.commit().await?;
    // Revocation does not erase useful past invitation history.
    let (_, grants) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("/api/v1/entries/{folder}/invitations"),
            None,
        )
        .await?;
    let grant = grants["items"][0]["id"].as_str().unwrap_or_default();
    w.call(
        &w.ada,
        Method::DELETE,
        &format!("/api/v1/entries/{folder}/invitations/{grant}"),
        None,
    )
    .await?;
    let (_, people) = w
        .call(&w.ada, Method::GET, "/api/v1/recipients", None)
        .await?;
    assert_eq!(people["items"][0]["invited"], 1);
    let (status,created)=w.call(&w.ada,Method::POST,"/api/v1/entries",Some(json!({
        "name":"invited-on-creation", "invitees":[{"principal":{"type":"carbon","id":w.bob.id},"access":["read"],"inherit":true}]
    }))).await?;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (_, people) = w
        .call(&w.ada, Method::GET, "/api/v1/recipients", None)
        .await?;
    assert_eq!(people["items"][0]["invited"], 2);
    Ok(())
}

async fn contacts_world() -> anyhow::Result<Option<(World, MockServer)>> {
    let Some(mut w) = World::new().await? else {
        return Ok(None);
    };
    let server = MockServer::start().await;
    w.f.with_contacts(&server.uri())?;
    Mock::given(method("POST"))
        .and(path("/v1/proofs/user-verification"))
        .and(body_partial_json(
            json!({"subject_token":w.ada.token,"receiving_app":"silicon-contacts"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"proof_id":"contacts-test","proof_token":"sap_contacts_fixture"}),
        ))
        .mount(&w.f.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/me"))
        .and(header("authorization", "Proof sap_contacts_fixture"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"account":{"uuid":w.ada.uuid}})),
        )
        .mount(&server)
        .await;
    Ok(Some((w, server)))
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One consent/save/disconnect lifecycle checks isolation throughout.
async fn contacts_require_own_consent_and_save_preserves_other_fields() -> anyhow::Result<()> {
    let Some((w, server)) = contacts_world().await? else {
        return Ok(());
    };
    let (status, body) = w
        .call(
            &w.ada,
            Method::POST,
            "/api/v1/contacts",
            Some(json!({"id":w.bob.uuid,"name":"Bob","trust":2})),
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
    let (status, body) = w
        .call(
            &w.ada,
            Method::PUT,
            "/api/v1/contacts/connection",
            Some(json!({"enabled":true})),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, other) = w
        .call(&w.bob, Method::GET, "/api/v1/contacts", None)
        .await?;
    assert_eq!(other["connected"], false);
    let contact = json!({"uuid":w.bob.uuid,"account":{"uuid":w.bob.uuid,"id":w.bob.id,"kind":"carbon","display_name":"Bob","pfp_url":"https://accounts.example/bob.png"},"name":"My colleague","trust":2,"note":"Keep this","phone":"local-fixture","email":"fixture@example.test","photo_id":"00000000-0000-4000-8000-000000000099","birthday_month":4,"birthday_day":7,"birthday_year":null});
    Mock::given(method("GET"))
        .and(path("/v1/contacts"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"items":[contact],"total":1})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/lookup"))
        .respond_with(ResponseTemplate::new(200).set_body_json(contact["account"].clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v1/contacts/{}", w.bob.uuid)))
        .respond_with(ResponseTemplate::new(200).set_body_json(contact.clone()))
        .mount(&server)
        .await;
    Mock::given(method("POST")).and(path("/v1/contacts"))
        .and(body_partial_json(json!({"id":w.bob.uuid,"name":"Bobby","trust":255,"note":"Keep this","phone":"local-fixture","photo_id":contact["photo_id"],"birthday_month":4})))
        .respond_with(ResponseTemplate::new(200).set_body_json(contact.clone())).expect(1).mount(&server).await;
    let (_, people) = w
        .call(&w.ada, Method::GET, "/api/v1/recipients", None)
        .await?;
    assert_eq!(people["items"][0]["display_name"], "My colleague");
    assert_eq!(
        people["items"][0]["fallback_pfp_url"],
        "https://accounts.example/bob.png"
    );
    assert_eq!(
        people["items"][0]["pfp_url"],
        "/api/v1/contacts/photos/00000000-0000-4000-8000-000000000099"
    );
    let (status, body) = w
        .call(
            &w.ada,
            Method::POST,
            "/api/v1/contacts",
            Some(json!({"id":w.bob.uuid,"name":"Bobby","trust":255})),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    for trust in [0, 256] {
        assert!(
            !w.call(
                &w.ada,
                Method::POST,
                "/api/v1/contacts",
                Some(json!({"id":w.bob.uuid,"name":"Bobby","trust":trust}))
            )
            .await?
            .0
            .is_success()
        );
    }
    let requests = w.f.server.received_requests().await.unwrap_or_default();
    let proofs: Vec<Value> = requests
        .iter()
        .filter(|r| r.url.path() == "/v1/proofs/user-verification")
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect();
    assert!(proofs.iter().all(
        |p| p["scopes"] == json!(["contacts.read"]) || p["scopes"] == json!(["contacts.write"])
    ));
    w.call(
        &w.ada,
        Method::PUT,
        "/api/v1/contacts/connection",
        Some(json!({"enabled":false})),
    )
    .await?;
    let (_, people) = w
        .call(&w.ada, Method::GET, "/api/v1/recipients", None)
        .await?;
    assert_eq!(people["items"], json!([]));
    Ok(())
}

#[tokio::test]
async fn shared_filters_use_sharers_and_filter_before_pagination() -> anyhow::Result<()> {
    let Some((w, server)) = contacts_world().await? else {
        return Ok(());
    };
    w.call(
        &w.ada,
        Method::PUT,
        "/api/v1/contacts/connection",
        Some(json!({"enabled":true})),
    )
    .await?;
    Mock::given(method("GET"))
        .and(path("/v1/contacts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"items":[{"uuid":w.bob.uuid,"name":"Trusted Bob","trust":5}],"total":1}),
        ))
        .mount(&server)
        .await;
    let older = w.folder(&w.bob, "older-matching", None, None).await?;
    w.share(&w.bob, &older, "carbon", &w.ada.id, &["read"])
        .await?;
    let newer = w.folder(&w.cat, "newer-not-matching", None, None).await?;
    w.share(&w.cat, &newer, "carbon", &w.ada.id, &["read"])
        .await?;
    let (status, page) = w
        .call(
            &w.ada,
            Method::GET,
            "/api/v1/shared?contacts=saved&min_trust=5&limit=1",
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"][0]["entry"]["id"], older);
    assert_eq!(page["next_cursor"], Value::Null);
    assert_eq!(page["items"][0]["shared_by"][0]["uuid"], w.bob.uuid);
    let (_, page) = w
        .call(&w.ada, Method::GET, "/api/v1/shared?contacts=unsaved", None)
        .await?;
    assert_eq!(page["items"][0]["entry"]["id"], newer);
    let (_, page) = w
        .call(&w.ada, Method::GET, "/api/v1/shared?min_trust=6", None)
        .await?;
    assert_eq!(page["items"], json!([]));
    let (_, page) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("/api/v1/shared?from={}&q=older", w.bob.uuid),
            None,
        )
        .await?;
    assert_eq!(page["items"][0]["entry"]["id"], older);
    let (_, page) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("/api/v1/shared?from={}&q=newer", w.bob.uuid),
            None,
        )
        .await?;
    assert_eq!(page["items"], json!([]));
    let (status, _) = w
        .call(&w.bob, Method::GET, "/api/v1/shared?contacts=saved", None)
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    Ok(())
}
