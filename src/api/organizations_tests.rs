//! Live membership changes through the router and production's RLS role.
use super::{
    archive_tests::publish, drive_access_tests::in_drive, drives_tests::World,
    memory_store::MemoryStore,
};
use axum::http::{Method, StatusCode};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn team_shares_follow_live_membership_across_every_read_surface() -> anyhow::Result<()> {
    let store = MemoryStore::default();
    let Some(mut world) = World::with_store(Arc::new(store.clone())).await? else {
        return Ok(());
    };
    let service = MockServer::start().await;
    let joined = Arc::new(AtomicBool::new(false));
    let healthy = Arc::new(AtomicBool::new(true));
    let org = "example";
    let team = format!("{org}/{}", uuid::Uuid::new_v4());
    let target = json!({"type":"team","id":team,"name":"Design","org_id":org});
    for who in [&world.ada, &world.bob, &world.cat] {
        let account = who.uuid.clone();
        let is_owner = who.uuid == world.ada.uuid;
        let is_member = who.uuid == world.bob.uuid;
        let team = team.clone();
        let target = target.clone();
        let joined = joined.clone();
        let healthy = healthy.clone();
        Mock::given(method("GET")).and(path("/v1/integrations/briefcase/me"))
            .and(header("Authorization", format!("Bearer {}", who.token)))
            .respond_with(move |_: &wiremock::Request| {
                if !healthy.load(Ordering::SeqCst) { return ResponseTemplate::new(503); }
                let groups = if is_member && joined.load(Ordering::SeqCst) { vec![format!("organization:{org}"),format!("team:{team}")] } else { vec![] };
                ResponseTemplate::new(200).set_body_json(json!({"account_uuid":account,"organizations":[],"groups":groups,"share_targets":if is_owner { vec![target.clone(),json!({"type":"organization","id":org,"name":"Example","org_id":org})] } else { vec![] }}))
            }).mount(&service).await;
    }
    world.f.with_organizations(&service.uri())?;
    let folder = world.folder(&world.ada, "design", None, None).await?;
    let nested = world
        .folder(&world.ada, "nested", Some(&folder), None)
        .await?;
    let own = in_drive(&world, &world.ada, &world.ada).await?;
    let file = publish(
        &world,
        &store,
        &own,
        &nested,
        "roadmap.txt",
        "text/plain",
        b"Team roadmap",
    )
    .await?;
    let endpoint = format!("/api/v1/entries/{folder}/invitations");
    let body =
        json!({"principal":{"type":"team","id":team},"access":["read","update"],"inherit":true});
    let key = uuid::Uuid::new_v4().to_string();
    let auth = format!("Bearer {}", world.ada.token);
    let (status, grant) = world
        .f
        .send(
            Method::POST,
            &endpoint,
            Some(&auth),
            Some(body.clone()),
            &[("idempotency-key", &key)],
        )
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let (_, replay) = world
        .f
        .send(
            Method::POST,
            &endpoint,
            Some(&auth),
            Some(body),
            &[("idempotency-key", &key)],
        )
        .await?;
    assert_eq!(grant["id"], replay["id"]);
    let (_, listing) = world.call(&world.ada, Method::GET, &endpoint, None).await?;
    assert_eq!(listing["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(listing["items"][0]["principal"]["name"], "Design");

    for allowed in [false, true, false, true] {
        joined.store(allowed, Ordering::SeqCst);
        for route in [
            format!("/api/v1/entries/{file}"),
            format!("/api/v1/entries/{file}/download"),
            format!("/api/v1/entries/{file}/content"),
            format!("/api/v1/paths/{}/design/nested/roadmap.txt", world.ada.uuid),
        ] {
            let (status, answer) = world.call(&world.bob, Method::GET, &route, None).await?;
            assert_eq!(
                status,
                if allowed {
                    StatusCode::OK
                } else {
                    StatusCode::NOT_FOUND
                },
                "{route} {answer}"
            );
        }
        let (status, shared) = world
            .call(&world.bob, Method::GET, "/api/v1/shared", None)
            .await?;
        assert_eq!(status, StatusCode::OK, "{shared}");
        assert_eq!(
            shared["items"].as_array().map(Vec::len),
            Some(usize::from(allowed)),
            "{shared}"
        );
        let (status, search) = world
            .call(&world.bob, Method::GET, "/api/v1/search?q=roadmap", None)
            .await?;
        assert_eq!(status, StatusCode::OK, "{search}");
        assert_eq!(
            search["items"].as_array().map(Vec::len),
            Some(usize::from(allowed)),
            "{search}"
        );
        let (status, _) = world
            .call(
                &world.cat,
                Method::GET,
                &format!("/api/v1/entries/{file}"),
                None,
            )
            .await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    // A group member may edit but cannot delete, re-share or choose arbitrary teams.
    let (status, _) = world
        .call(
            &world.bob,
            Method::DELETE,
            &format!("/api/v1/entries/{file}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = world
        .call(
            &world.ada,
            Method::POST,
            &endpoint,
            Some(json!({"principal":{"type":"team","id":"other/no-such-team"}})),
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = world.call(&world.ada, Method::POST, &endpoint, Some(json!({"principal":{"type":"team","id":team},"expires_in_minutes":5,"access":["read","update"]}))).await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // Service failures cannot fall back to stale memberships.
    healthy.store(false, Ordering::SeqCst);
    let (status, _) = world
        .call(
            &world.bob,
            Method::GET,
            &format!("/api/v1/entries/{file}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    healthy.store(true, Ordering::SeqCst);
    // A separate direct share remains after team access is removed.
    let (status, direct) = world
        .share(&world.ada, &folder, "carbon", &world.bob.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{direct}");
    joined.store(false, Ordering::SeqCst);
    assert_eq!(
        world
            .call(
                &world.bob,
                Method::GET,
                &format!("/api/v1/entries/{file}"),
                None
            )
            .await?
            .0,
        StatusCode::OK
    );
    let (status, _) = world
        .call(
            &world.ada,
            Method::DELETE,
            &format!(
                "{endpoint}/{}",
                grant["id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing grant ID"))?
            ),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let direct_id = direct["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing direct grant"))?;
    assert_eq!(
        world
            .call(
                &world.ada,
                Method::DELETE,
                &format!("{endpoint}/{direct_id}"),
                None
            )
            .await?
            .0,
        StatusCode::NO_CONTENT
    );
    joined.store(true, Ordering::SeqCst);
    // Whole-organization grants use the same inheritance and expiry boundary.
    let (status, expiring) = world.call(&world.ada, Method::POST, &endpoint, Some(json!({"principal":{"type":"organization","id":org},"expires_in_minutes":5,"inherit":true}))).await?;
    assert_eq!(status, StatusCode::CREATED, "{expiring}");
    let group_id = expiring["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing expiring grant"))?;
    let expiry_endpoint = format!("{endpoint}/{group_id}");
    let (status, extended) = world
        .call(
            &world.ada,
            Method::PATCH,
            &expiry_endpoint,
            Some(json!({"expires_in_minutes":60})),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{extended}");
    assert!(extended["expires_at"].as_str() > expiring["expires_at"].as_str());
    assert_eq!(
        world
            .call(
                &world.bob,
                Method::GET,
                &format!("/api/v1/entries/{file}"),
                None
            )
            .await?
            .0,
        StatusCode::OK
    );
    let (status, permanent) = world
        .call(
            &world.ada,
            Method::PATCH,
            &expiry_endpoint,
            Some(json!({"permanent":true})),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{permanent}");
    assert!(permanent["expires_at"].is_null());
    assert_eq!(
        world
            .call(&world.ada, Method::DELETE, &expiry_endpoint, None)
            .await?
            .0,
        StatusCode::NO_CONTENT
    );
    let (_, expiring) = world.call(&world.ada, Method::POST, &endpoint, Some(json!({"principal":{"type":"organization","id":org},"expires_in_minutes":1,"inherit":true}))).await?;
    let expired_id: uuid::Uuid = expiring["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing expiring grant"))?
        .parse()?;
    sqlx::query("UPDATE briefcase.group_permission_grants SET expires_at=clock_timestamp()-interval '1 second' WHERE grant_id=$1").bind(expired_id).execute(&world.f.admin).await?;
    assert_eq!(
        world
            .call(
                &world.bob,
                Method::GET,
                &format!("/api/v1/entries/{file}"),
                None
            )
            .await?
            .0,
        StatusCode::NOT_FOUND
    );
    crate::worker::lifetimes::sweep(&world.f.admin, 1000).await?;
    crate::worker::lifetimes::sweep(&world.f.admin, 1000).await?;
    let expiry_events: i64 = sqlx::query_scalar("SELECT count(*) FROM briefcase.audit_events WHERE action='permission.group_expiring_share_expired.v1' AND metadata->>'grant_id'=$1").bind(expired_id.to_string()).fetch_one(&world.f.admin).await?;
    assert_eq!(expiry_events, 1, "the worker records each expiry once");
    Ok(())
}
