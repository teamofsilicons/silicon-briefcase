//! Permanent addresses through the router: drives named by their owner's id
//! or uuid, the top of a drive, ids an owner gave up, links anyone may open,
//! and addresses from before personal drives resolved through the cutover's
//! aliases.
//!
//! ```bash
//! BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5460/briefcase_mig \
//!   cargo test --lib api::locations_tests -- --test-threads=1
//! ```

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tower::ServiceExt as _;
use uuid::Uuid;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{method, path},
};

use super::drives_tests::{World, code};
use crate::infrastructure::accounts::test_support::carbon_summary;

const SITE: &str = "https://briefcase.example";
const API: &str = "https://briefcase.example/api/v1";

/// Sends a request and returns the status, `Location` and JSON body.
async fn fetch(
    w: &World,
    uri: &str,
    token: Option<&str>,
) -> anyhow::Result<(StatusCode, Option<String>, Value)> {
    let mut request = Request::builder().method(Method::GET).uri(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response =
        w.f.router
            .clone()
            .oneshot(request.body(Body::empty())?)
            .await?;
    let status = response.status();
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await?;
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok((status, location, body))
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn permanent_addresses_name_each_drive_by_its_owner() -> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let handbook = w.folder(&w.ada, "handbook", None, None).await?;
    let chapter = w
        .folder(&w.ada, "chapter one", Some(&handbook), None)
        .await?;
    w.folder(&w.ada, "diary", None, None).await?;
    let (status, _) = w
        .share(&w.ada, &handbook, "carbon", &w.bob.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);

    // The entry names its address by the owner's id; API content addresses
    // name the drive by uuid.
    let (status, entry) = w
        .call(
            &w.bob,
            Method::GET,
            &format!("/api/v1/entries/{chapter}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{entry}");
    assert_eq!(
        entry["permanent_url"],
        format!("{SITE}/{}/handbook/chapter%20one", w.ada.id)
    );

    // Ada's id and Ada's uuid lead to the same entry.
    for owner in [&w.ada.id, &w.ada.uuid] {
        let (status, _, body) = fetch(
            &w,
            &format!("/api/v1/paths/{owner}/handbook/chapter%20one"),
            Some(&w.bob.token),
        )
        .await?;
        assert_eq!(status, StatusCode::OK, "{owner}: {body}");
        assert_eq!(body["id"], chapter.as_str());
    }
    // What the caller cannot read, an unknown owner and a malformed owner
    // all answer 404.
    for (who, uri) in [
        (&w.cat, format!("/api/v1/paths/{}/handbook", w.ada.id)),
        (&w.bob, format!("/api/v1/paths/{}/diary", w.ada.id)),
        (&w.bob, "/api/v1/paths/c:nobody-at-all/handbook".to_owned()),
        (&w.bob, "/api/v1/paths/not%20an%20id/handbook".to_owned()),
    ] {
        Mock::given(method("GET"))
            .and(path("/v1/accounts/by-id/c:nobody-at-all"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": { "code": "not_found", "message": "No account has that id." }
            })))
            .mount(&w.f.server)
            .await;
        let (status, _, body) = fetch(&w, &uri, Some(&who.token)).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }

    // The top of a drive: the owner sees it; someone with a share inside
    // sees only that it leads somewhere; an outsider sees nothing; without a
    // token it is refused, never public.
    let (status, _, top) = fetch(
        &w,
        &format!("/api/v1/paths/{}", w.ada.id),
        Some(&w.ada.token),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{top}");
    assert_eq!(top["path"], "");
    assert_eq!(top["permanent_url"], format!("{SITE}/{}", w.ada.id));
    let (status, _, top) = fetch(
        &w,
        &format!("/api/v1/paths/{}", w.ada.id),
        Some(&w.bob.token),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{top}");
    assert_eq!(top["visibility"], "traversal");
    let (status, _, _) = fetch(
        &w,
        &format!("/api/v1/paths/{}", w.ada.id),
        Some(&w.cat.token),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, body) = fetch(&w, &format!("/api/v1/paths/{}", w.ada.id), None).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(code(&body), "authentication_required");

    // Anyone with the link reads a link-shared entry by id or uuid; nothing
    // else is public.
    let (status, link) = w
        .call(
            &w.ada,
            Method::PUT,
            &format!("/api/v1/entries/{handbook}/link-access"),
            Some(json!({ "enabled": true })),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{link}");
    assert_eq!(link["url"], format!("{SITE}/{}/handbook", w.ada.id));
    for owner in [&w.ada.id, &w.ada.uuid] {
        let (status, _, body) =
            fetch(&w, &format!("/api/v1/public/{owner}/handbook"), None).await?;
        assert_eq!(status, StatusCode::OK, "{owner}: {body}");
        assert_eq!(body["id"], handbook.as_str());
        let (status, _, body) = fetch(
            &w,
            &format!("/api/v1/paths/{owner}/handbook/chapter%20one"),
            None,
        )
        .await?;
        assert_eq!(status, StatusCode::OK, "{owner}: {body}");
        let (status, _, _) = fetch(&w, &format!("/api/v1/public/{owner}/diary"), None).await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    Ok(())
}

#[tokio::test]
async fn an_id_the_owner_gave_up_moves_with_the_owner() -> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let handbook = w.folder(&w.ada, "handbook", None, None).await?;
    let (status, _) = w
        .call(
            &w.ada,
            Method::PUT,
            &format!("/api/v1/entries/{handbook}/link-access"),
            Some(json!({ "enabled": true })),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let (old, new) = (w.ada.id.clone(), w.f.id("c:ada-renamed"));
    let data = json!({
        "uuid": w.ada.uuid, "membership_id": format!("briefcase:{}", w.ada.uuid),
        "kind": "carbon", "old_id": old, "new_id": new,
    });
    assert_eq!(
        w.f.deliver("account.id_changed", OffsetDateTime::now_utc(), &data)
            .await?,
        StatusCode::NO_CONTENT
    );
    // Silicon Accounts no longer knows the old id; the new one is Ada's.
    Mock::given(method("GET"))
        .and(path(format!("/v1/accounts/by-id/{old}")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": { "code": "not_found", "message": "No account has that id." }
        })))
        .with_priority(1)
        .mount(&w.f.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v1/accounts/by-id/{new}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(carbon_summary(&w.ada.uuid, &new)))
        .with_priority(1)
        .mount(&w.f.server)
        .await;

    let (status, location, body) = fetch(
        &w,
        &format!("/api/v1/paths/{old}/handbook?disposition=inline"),
        Some(&w.ada.token),
    )
    .await?;
    assert_eq!(status, StatusCode::PERMANENT_REDIRECT, "{body}");
    let moved_to = format!("{API}/paths/{new}/handbook?disposition=inline");
    assert_eq!(location.as_deref(), Some(moved_to.as_str()));
    assert_eq!(body["moved_to"], moved_to);
    assert_eq!(body["permanent_url"], format!("{SITE}/{new}/handbook"));
    assert_eq!(body["owner"]["uuid"], w.ada.uuid.as_str());

    let (status, location, _) = fetch(&w, &format!("/api/v1/public/{old}/handbook"), None).await?;
    assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(location, Some(format!("{API}/public/{new}/handbook")));

    // The new id resolves, and new addresses use it.
    let (status, _, entry) = fetch(
        &w,
        &format!("/api/v1/paths/{new}/handbook"),
        Some(&w.ada.token),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{entry}");
    assert_eq!(entry["permanent_url"], format!("{SITE}/{new}/handbook"));
    Ok(())
}

/// Records one cutover run and the aliases it would have written, as
/// `briefcase-migrate link-identities` does when it moves an organization's
/// files into drives.
async fn record_aliases(
    w: &World,
    organization: &str,
    aliases: &[(&str, &str)],
) -> anyhow::Result<()> {
    let run = Uuid::now_v7();
    // Recorded as reverted, so the shared test database never holds an
    // applied run (only one may exist at a time).
    sqlx::query(
        "INSERT INTO briefcase.identity_link_runs \
             (run_id, status, mapping_sha256, mapping, finished_at, reverted_at) \
         VALUES ($1, 'reverted', repeat('0', 64), '[]'::jsonb, clock_timestamp(), clock_timestamp())",
    )
    .bind(run)
    .execute(&w.f.admin)
    .await?;
    for (legacy_path, entry) in aliases {
        sqlx::query(
            "INSERT INTO briefcase.legacy_path_aliases (legacy_org, legacy_path, drive, entry_id, run_id) \
             VALUES ($1, $2, $3, $4::uuid, $5)",
        )
        .bind(organization)
        .bind(legacy_path)
        .bind(&w.ada.uuid)
        .bind(entry)
        .bind(run)
        .execute(&w.f.admin)
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn addresses_from_before_personal_drives_resolve_through_aliases() -> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let space = w.folder(&w.ada, "tos-public", None, None).await?;
    let handbook = w.folder(&w.ada, "handbook", Some(&space), None).await?;
    let chapter = w.folder(&w.ada, "chapter", Some(&handbook), None).await?;
    let organization = format!("org{}", &Uuid::now_v7().simple().to_string()[20..]);
    record_aliases(
        &w,
        &organization,
        &[
            ("public", space.as_str()),
            ("public/handbook", handbook.as_str()),
        ],
    )
    .await?;
    let (status, _) = w
        .share(&w.ada, &space, "carbon", &w.bob.id, &["read", "write"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);

    // The exact legacy address, and one below a moved folder, lead to the
    // entries' current places; the caller's own access still decides.
    for (uri, expected) in [
        (format!("/org/{organization}/public/handbook"), &handbook),
        (
            format!("/org/{organization}/public/handbook/chapter"),
            &chapter,
        ),
        (format!("/org/{organization}/public"), &space),
    ] {
        let (status, _, body) = fetch(&w, &uri, Some(&w.bob.token)).await?;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        assert_eq!(body["id"], expected.as_str(), "{uri}");
        assert!(
            body["permanent_url"]
                .as_str()
                .is_some_and(|url| url.starts_with(&format!("{SITE}/{}/tos-public", w.ada.id))),
            "{body}"
        );
    }
    for (who, uri) in [
        (&w.cat, format!("/org/{organization}/public/handbook")),
        (&w.bob, format!("/org/{organization}/private/c:bob/notes")),
        (&w.bob, "/org/never-an-org/public/handbook".to_owned()),
    ] {
        let (status, _, body) = fetch(&w, &uri, Some(&who.token)).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }

    // Links anyone may open keep working through the old public address
    // once the entry is link-shared, and only then.
    let legacy_public = format!("/api/v1/public/{organization}/public/handbook");
    let (status, _, _) = fetch(&w, &legacy_public, None).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = w
        .call(
            &w.ada,
            Method::PUT,
            &format!("/api/v1/entries/{handbook}/link-access"),
            Some(json!({ "enabled": true })),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = fetch(&w, &legacy_public, None).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], handbook.as_str());
    let (status, _, body) = fetch(
        &w,
        &format!("/org/{organization}/public/handbook/chapter"),
        None,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], chapter.as_str());
    Ok(())
}
