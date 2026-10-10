//! The route families personal drives touch beyond the first drive tests:
//! uploads and the drive they are charged to, versions, content, self-destruct
//! keeps, storage settings, search, permanent and public URLs, apps acting
//! through proofs, account filters, and moves across drives — each checked for
//! the drive's owner, a share, the custodian and an outsider.
//!
//! Uploads go through the content repository with a stored-object receipt, as
//! `tests/postgres_versioning.rs` does, because these tests have no object
//! store; everything else goes through the router.
//!
//! ```bash
//! BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5460/briefcase_mig \
//!   cargo test --lib api::drive_access_tests -- --test-threads=1
//! ```

use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

use super::{
    auth::Action,
    drives,
    drives_tests::{Account, World, code, names},
    extract,
};
use crate::{
    application::{
        content::{ContentRepository as _, Prepared, SmallUploadCommand, StagedContent},
        context::ExecutionContext,
        idempotency::IdempotencyKey,
        ports::{ObjectChecksum, ObjectChecksumAlgorithm, ObjectChecksumType, StoredObject},
    },
    domain::{entry::EntryName, ids::EntryId, lifetime::LifetimeMinutes},
    error::AppError,
    infrastructure::accounts::test_support::valid_proof,
};

/// The context `who` gets for a request in `drive`, built exactly as the
/// API builds it: the verified token, then the drive resolution.
async fn in_drive(w: &World, who: &Account, drive: &Account) -> anyhow::Result<ExecutionContext> {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", who.token))?,
    );
    Box::pin(crate::request_context::scope(
        Uuid::now_v7().to_string(),
        async {
            let context = extract::authenticate(&w.f.state, &headers, Action::UploadFile, "test")
                .await
                .map_err(|error| anyhow::anyhow!("authenticate: {error:?}"))?;
            drives::enter(&w.f.state, context, &drive.uuid)
                .await
                .map_err(|error| anyhow::anyhow!("enter: {error:?}"))
        },
    ))
    .await
}

/// Publishes a small file into `parent` with a stored-object receipt and
/// returns its id, or the refusal.
async fn publish(
    w: &World,
    context: &ExecutionContext,
    parent: &str,
    name: &str,
    bytes: &[u8],
    self_destruct: Option<LifetimeMinutes>,
) -> anyhow::Result<Result<String, AppError>> {
    let staged = std::env::temp_dir().join(format!("briefcase-drive-{}", Uuid::now_v7()));
    tokio::fs::write(&staged, bytes).await?;
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let payload = StagedContent {
        path: staged.as_path(),
        offset: 0,
        size: bytes.len() as u64,
        sha256: digest,
    };
    let command = SmallUploadCommand {
        parent_id: EntryId::from_uuid(Uuid::parse_str(parent)?)?,
        name: EntryName::new(name)?,
        content_type: "text/markdown".to_owned(),
        idempotency_key: IdempotencyKey::new(Uuid::now_v7().to_string())?,
        request_hash: [7; 32],
        self_destruct,
    };
    let repository = &w.f.state.content_adapter;
    let outcome = match repository
        .prepare_small_upload(context, &command, &payload)
        .await
    {
        Ok(Prepared::Acquired(preparation)) => {
            let stored = StoredObject {
                key: preparation.key.clone(),
                etag: Some("\"published\"".to_owned()),
                provider_version_id: None,
                size: payload.size,
                checksum: Some(ObjectChecksum::new(
                    ObjectChecksumAlgorithm::Sha256,
                    ObjectChecksumType::FullObject,
                    STANDARD.encode(digest),
                )?),
            };
            repository
                .commit_small_upload(context, &command, &payload, &preparation, &stored)
                .await
                .map(|entry| entry.to_string())
        }
        Ok(Prepared::Replay(entry)) => Ok(entry.to_string()),
        Err(error) => Err(error),
    };
    tokio::fs::remove_file(&staged).await?;
    Ok(outcome)
}

/// A drive's raw usage counters (uploaded today, stored), if it has a row.
async fn usage(w: &World, drive: &Account) -> anyhow::Result<Option<(i64, i64)>> {
    Ok(sqlx::query_as::<_, (i64, i64)>(
        "SELECT daily_upload_bytes, stored_bytes FROM briefcase.organization_usage \
          WHERE org_id = $1",
    )
    .bind(&drive.uuid)
    .fetch_optional(&w.f.admin)
    .await?)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn uploads_land_in_and_are_charged_to_the_drive_that_holds_the_folder() -> anyhow::Result<()>
{
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let inbox = w.folder(&w.ada, "inbox", None, None).await?;
    let readonly = w.folder(&w.ada, "readonly", None, None).await?;
    let (status, _) = w
        .share(&w.ada, &inbox, "carbon", &w.bob.id, &["read", "write"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = w
        .share(&w.ada, &readonly, "carbon", &w.cat.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);

    // Bob's upload lands in Ada's drive and spends Ada's allowance, not his.
    let bob_in_ada = in_drive(&w, &w.bob, &w.ada).await?;
    let body = b"notes from bob";
    let file = publish(&w, &bob_in_ada, &inbox, "from-bob.md", body, None)
        .await?
        .map_err(|error| anyhow::anyhow!("Bob may write into the shared folder: {error:?}"))?;
    let size = i64::try_from(body.len())?;
    assert_eq!(usage(&w, &w.ada).await?, Some((size, size)));
    assert_eq!(
        usage(&w, &w.bob).await?.unwrap_or_default(),
        (0, 0),
        "nothing is charged to the uploader's own drive"
    );
    let (status, ada_usage) = w.call(&w.ada, Method::GET, "/api/v1/usage", None).await?;
    assert_eq!(status, StatusCode::OK, "{ada_usage}");
    assert_eq!(ada_usage["storage"]["used_bytes"], json!(size));

    // Read conveys no upload, and nothing outside a share is reachable.
    let cat_in_ada = in_drive(&w, &w.cat, &w.ada).await?;
    let refused = publish(&w, &cat_in_ada, &readonly, "x.md", b"x", None).await?;
    assert!(
        matches!(refused, Err(AppError::Forbidden)),
        "a read share adds nothing: {refused:?}"
    );
    let hidden = publish(&w, &cat_in_ada, &inbox, "x.md", b"x", None).await?;
    assert!(
        matches!(hidden, Err(AppError::NotFound)),
        "an unshared folder is not there: {hidden:?}"
    );

    // The file is Bob's (creator) in Ada's drive; outsiders get 404 for its
    // metadata, versions and bytes alike.
    let (status, entry) = w
        .call(
            &w.bob,
            Method::GET,
            &format!("/api/v1/entries/{file}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{entry}");
    assert_eq!(entry["org_id"], w.ada.uuid.as_str());
    assert_eq!(entry["owner"]["id"], w.bob.uuid.as_str());
    let (status, versions) = w
        .call(
            &w.bob,
            Method::GET,
            &format!("/api/v1/entries/{file}/versions"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{versions}");
    assert_eq!(versions["items"].as_array().map(Vec::len), Some(1));
    for uri in [
        format!("/api/v1/entries/{file}"),
        format!("/api/v1/entries/{file}/versions"),
        format!("/api/v1/entries/{file}/content"),
        format!("/api/v1/entries/{file}/download"),
    ] {
        for who in [&w.cat, &w.scout, &w.rover] {
            let (status, body) = w.call(who, Method::GET, &uri, None).await?;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        }
    }
    // The drive's owner holds every right over what a grantee put there.
    let (status, _) = w
        .call(
            &w.ada,
            Method::DELETE,
            &format!("/api/v1/entries/{file}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn a_self_destruct_is_kept_by_the_creator_the_drive_owner_or_a_custodian()
-> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let soon = Some(LifetimeMinutes::new(60)?);
    let inbox = w.folder(&w.ada, "timers", None, None).await?;
    let (status, _) = w
        .share(&w.ada, &inbox, "carbon", &w.bob.id, &["read", "write"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let bob_in_ada = in_drive(&w, &w.bob, &w.ada).await?;
    let mut timers = Vec::new();
    for name in ["one.md", "two.md"] {
        timers.push(
            publish(&w, &bob_in_ada, &inbox, name, b"tick", soon)
                .await?
                .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        );
    }
    let keep = |entry: &str| format!("/api/v1/entries/{entry}/self-destruct");
    for who in [&w.cat, &w.scout] {
        let (status, body) = w.call(who, Method::DELETE, &keep(&timers[0]), None).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }
    let (status, body) = w
        .call(&w.bob, Method::DELETE, &keep(&timers[0]), None)
        .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "the creator keeps it: {body}"
    );
    let (status, body) = w
        .call(&w.ada, Method::DELETE, &keep(&timers[1]), None)
        .await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "the drive's owner keeps it: {body}"
    );

    // Scout (Ada's Silicon) works in Bob's folder once Ada lets Bob share
    // with it; Ada, its custodian, keeps what Scout set to self-destruct
    // there, though Bob's drive is otherwise closed to her.
    let bobs = w.folder(&w.bob, "bob-timers", None, None).await?;
    let allowance = format!(
        "/api/v1/silicons/{}/allowed-accounts/{}",
        w.scout.id, w.bob.id
    );
    let (status, _) = w.call(&w.ada, Method::PUT, &allowance, None).await?;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = w
        .share(&w.bob, &bobs, "silicon", &w.scout.id, &["read", "write"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let scout_in_bob = in_drive(&w, &w.scout, &w.bob).await?;
    let scouts = publish(&w, &scout_in_bob, &bobs, "scout.md", b"tock", soon)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let (status, body) = w.call(&w.cat, Method::DELETE, &keep(&scouts), None).await?;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = w.call(&w.ada, Method::DELETE, &keep(&scouts), None).await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "the custodian keeps it: {body}"
    );
    Ok(())
}

fn bucket(drive: Option<&str>) -> Value {
    let mut body = json!({
        "bucket_name": "example-briefcase",
        "region": "ap-south-1",
        "role_arn": "arn:aws:iam::123456789012:role/briefcase",
        "prefix": "drives/briefcase",
        "aws_account_id": "123456789012",
        "encryption_mode": "sse_s3",
    });
    if let Some(drive) = drive {
        body["drive"] = json!(drive);
    }
    body
}

#[tokio::test]
async fn storage_settings_belong_to_the_drive_and_its_owner_or_custodian() -> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let uri = "/api/v1/storage/configuration";
    for (who, drive) in [
        (&w.bob, &w.scout.id),
        (&w.cat, &w.ada.id),
        (&w.scout, &w.ada.id),
    ] {
        let (status, body) = w
            .call(who, Method::PUT, uri, Some(bucket(Some(drive))))
            .await?;
        assert_eq!(status, StatusCode::FORBIDDEN, "{drive}: {body}");
    }
    // Ada looks after Scout, so she configures Scout's drive; the bucket
    // cannot be reached from a test, so the attempt is recorded as failed,
    // with the External ID derived from Scout's drive.
    let (status, body) = w
        .call(&w.ada, Method::PUT, uri, Some(bucket(Some(&w.scout.id))))
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "failed", "{body}");
    let recorded: (String, String) = sqlx::query_as(
        "SELECT created_by_id, external_id FROM briefcase.organization_storage_configs \
          WHERE org_id = $1",
    )
    .bind(&w.scout.uuid)
    .fetch_one(&w.f.admin)
    .await?;
    assert_eq!(recorded.0, w.ada.uuid);
    assert_eq!(
        recorded.1,
        crate::infrastructure::s3::organization_storage_external_id(&w.scout.uuid)
    );
    // A Silicon configures its own drive.
    let (status, body) = w
        .call(&w.scout, Method::PUT, uri, Some(bucket(None)))
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    Ok(())
}

fn result_names(results: &Value) -> Vec<String> {
    results["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["entry"]["name"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn search_reaches_what_was_shared_and_nothing_else() -> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let shared = w.folder(&w.ada, "shared-q4", None, None).await?;
    let private = w.folder(&w.ada, "private-q4", None, None).await?;
    let (status, _) = w
        .share(&w.ada, &shared, "carbon", &w.bob.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let ada_own = in_drive(&w, &w.ada, &w.ada).await?;
    for (parent, name) in [(&shared, "q4-roadmap.md"), (&private, "q4-salaries.md")] {
        publish(&w, &ada_own, parent, name, b"q4", None)
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    }
    let search = "/api/v1/search?q=q4";
    let (status, bob) = w.call(&w.bob, Method::GET, search, None).await?;
    assert_eq!(status, StatusCode::OK, "{bob}");
    assert_eq!(result_names(&bob), vec!["q4-roadmap.md"], "{bob}");
    let (_, cat) = w.call(&w.cat, Method::GET, search, None).await?;
    assert!(result_names(&cat).is_empty(), "{cat}");
    let (_, ada) = w.call(&w.ada, Method::GET, search, None).await?;
    let mut found = result_names(&ada);
    found.sort();
    assert_eq!(found, vec!["q4-roadmap.md", "q4-salaries.md"]);
    // The custodian searches its Silicon's drive too.
    let scout_own = in_drive(&w, &w.scout, &w.scout).await?;
    let scout_folder = w.folder(&w.scout, "scout-q4", None, None).await?;
    publish(&w, &scout_own, &scout_folder, "q4-scout.md", b"q4", None)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let (_, ada) = w.call(&w.ada, Method::GET, search, None).await?;
    assert!(
        result_names(&ada).contains(&"q4-scout.md".to_owned()),
        "{ada}"
    );
    let (_, bob) = w.call(&w.bob, Method::GET, search, None).await?;
    assert!(!result_names(&bob).contains(&"q4-scout.md".to_owned()));
    Ok(())
}

#[tokio::test]
async fn permanent_and_public_urls_resolve_inside_the_drive() -> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let handbook = w.folder(&w.ada, "handbook", None, None).await?;
    w.folder(&w.ada, "diary", None, None).await?;
    let (status, _) = w
        .share(&w.ada, &handbook, "carbon", &w.bob.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let permanent = |path: &str| format!("/org/{}/{path}", w.ada.uuid);
    let (status, body) = w
        .call(&w.bob, Method::GET, &permanent("handbook"), None)
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], handbook.as_str());
    for (who, path) in [(&w.cat, "handbook"), (&w.bob, "diary")] {
        let (status, body) = w.call(who, Method::GET, &permanent(path), None).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
    }
    // Anyone with the link reads a link-public entry; nothing else is public.
    let (status, link) = w
        .call(
            &w.ada,
            Method::PUT,
            &format!("/api/v1/entries/{handbook}/link-access"),
            Some(json!({ "enabled": true })),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{link}");
    let public = |path: &str| format!("/api/v1/public/{}/{path}", w.ada.uuid);
    let (status, body) =
        w.f.send(Method::GET, &public("handbook"), None, None, &[])
            .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) =
        w.f.send(Method::GET, &public("diary"), None, None, &[])
            .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn apps_act_only_in_the_drive_of_the_account_they_act_for() -> anyhow::Result<()> {
    let Some(w) =
        World::with_issuers("briefcase.entries.list=interface briefcase.files.read=interface")
            .await?
    else {
        return Ok(());
    };
    let plans = w.folder(&w.ada, "plans", None, None).await?;
    let (status, _) = w
        .share(&w.ada, &plans, "carbon", &w.bob.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let ada_own = in_drive(&w, &w.ada, &w.ada).await?;
    let file = publish(&w, &ada_own, &plans, "plan.md", b"plan", None)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    Mock::given(method("POST"))
        .and(path("/v1/proofs/verify"))
        .and(body_string_contains("sap_interface_for_bob"))
        .respond_with(ResponseTemplate::new(200).set_body_json(valid_proof(
            "interface",
            "briefcase",
            (w.bob.uuid.as_str(), w.bob.id.as_str(), "carbon"),
            &["briefcase.entries.list", "briefcase.files.read"],
        )))
        .mount(&w.f.server)
        .await;
    let proof = Some("Proof sap_interface_for_bob");
    // Bob himself reads Ada's shared folder; an app acting for Bob does not.
    let (status, _) = w
        .call(
            &w.bob,
            Method::GET,
            &format!("/api/v1/entries/{file}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    for (uri, body) in [
        ("/api/v1/obo/entries/list", json!({ "parent_id": plans })),
        ("/api/v1/obo/files/read", json!({ "entry_id": file })),
    ] {
        let (status, response) = w.f.send(Method::POST, uri, proof, Some(body), &[]).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {response}");
    }
    let (status, listing) =
        w.f.send(
            Method::POST,
            "/api/v1/obo/entries/list",
            proof,
            Some(json!({})),
            &[],
        )
        .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "Bob's own drive is open to it: {listing}"
    );
    Ok(())
}

#[tokio::test]
async fn filters_name_accounts_by_their_current_id() -> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let team = w.folder(&w.ada, "team", None, None).await?;
    let (status, _) = w
        .share(&w.ada, &team, "carbon", &w.bob.id, &["read", "write"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    w.folder(&w.bob, "bob-notes", Some(&team), None).await?;
    w.folder(&w.ada, "ada-notes", Some(&team), None).await?;
    let filtered = |filter: &str| {
        format!(
            "/api/v1/entries?filter={}",
            percent_encoding::utf8_percent_encode(filter, percent_encoding::NON_ALPHANUMERIC)
        )
    };
    let (status, listing) = w
        .call(
            &w.ada,
            Method::GET,
            &filtered(&format!("from:@{}", w.bob.id)),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(names(&listing), vec!["bob-notes"]);
    let (status, body) = w
        .call(
            &w.ada,
            Method::GET,
            &filtered("from:@c:nobody-at-all"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(code(&body), "unknown_account");
    Ok(())
}

#[tokio::test]
async fn entries_move_only_within_the_drive_that_holds_them() -> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let mine = w.folder(&w.ada, "mine", None, None).await?;
    let open = w.folder(&w.bob, "open", None, None).await?;
    let closed = w.folder(&w.bob, "closed", None, None).await?;
    let (status, _) = w
        .share(&w.bob, &open, "carbon", &w.ada.id, &["read", "write"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let patch = format!("/api/v1/entries/{mine}");
    let (status, body) = w
        .call(
            &w.ada,
            Method::PATCH,
            &patch,
            Some(json!({ "parent_id": open })),
        )
        .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(code(&body), "cross_drive_move");
    let (status, body) = w
        .call(
            &w.ada,
            Method::PATCH,
            &patch,
            Some(json!({ "parent_id": closed })),
        )
        .await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an unseen folder is not there: {body}"
    );
    // Within one drive a move works as before.
    let inner = w.folder(&w.ada, "inner", None, None).await?;
    let (status, moved) = w
        .call(
            &w.ada,
            Method::PATCH,
            &format!("/api/v1/entries/{inner}"),
            Some(json!({ "parent_id": mine })),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["path"], "mine/inner");
    Ok(())
}
