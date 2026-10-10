//! Apps acting for an account through proofs: one scope per delegated route,
//! confined apps kept to `apps/{app}/` in the account's own drive, first-party
//! apps reaching what the account can, entries recording the app that made
//! them, trash limited to what the app created, and the byte transfer finding
//! its drive from the reservation.
//!
//! ```bash
//! BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5460/briefcase_mig \
//!   cargo test --lib api::delegation_tests -- --test-threads=1
//! ```

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

use super::{
    drive_access_tests::{in_drive, publish},
    drives_tests::{Account, World, code, names},
};
use crate::infrastructure::accounts::test_support::valid_proof;

const ISSUERS: &str = "briefcase.entries.list=interface|waveform \
     briefcase.files.read=interface|waveform \
     briefcase.folders.create=interface|waveform \
     briefcase.entries.trash=interface|waveform \
     briefcase.uploads.reserve=waveform";

/// Serves `POST /v1/proofs/verify` for one proof token: `app` acting for
/// `who` with `scopes`.
async fn proof(w: &World, token: &str, app: &str, who: &Account, kind: &str, scopes: &[&str]) {
    Mock::given(method("POST"))
        .and(path("/v1/proofs/verify"))
        .and(body_string_contains(token))
        .respond_with(ResponseTemplate::new(200).set_body_json(valid_proof(
            app,
            "briefcase",
            (who.uuid.as_str(), who.id.as_str(), kind),
            scopes,
        )))
        .mount(&w.f.server)
        .await;
}

async fn delegated(
    w: &World,
    token: &str,
    uri: &str,
    body: Value,
) -> anyhow::Result<(StatusCode, Value)> {
    w.f.send(
        Method::POST,
        uri,
        Some(&format!("Proof {token}")),
        Some(body),
        &[],
    )
    .await
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn first_party_apps_reach_what_the_account_can_and_others_only_their_folder()
-> anyhow::Result<()> {
    let Some(w) = World::with_delegation(ISSUERS, "interface").await? else {
        return Ok(());
    };
    let all = [
        "briefcase.entries.list",
        "briefcase.files.read",
        "briefcase.folders.create",
        "briefcase.entries.trash",
    ];
    proof(&w, "sap_interface_bob", "interface", &w.bob, "carbon", &all).await;
    proof(&w, "sap_waveform_bob", "waveform", &w.bob, "carbon", &all).await;
    proof(&w, "sap_interface_ada", "interface", &w.ada, "carbon", &all).await;

    let plans = w.folder(&w.ada, "plans", None, None).await?;
    let (status, _) = w
        .share(&w.ada, &plans, "carbon", &w.bob.id, &["read", "write"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let ada_own = in_drive(&w, &w.ada, &w.ada).await?;
    publish(&w, &ada_own, &plans, "plan.md", b"plan", None)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let notes = w.folder(&w.bob, "notes", None, None).await?;

    // Interface acting for Bob sees what Bob sees: Ada's shared folder and
    // Bob's own files outside apps/interface/.
    let (status, listing) = delegated(
        &w,
        "sap_interface_bob",
        "/api/v1/obo/entries/list",
        json!({ "parent_id": plans }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(names(&listing), vec!["plan.md"]);
    let (status, listing) = delegated(
        &w,
        "sap_interface_bob",
        "/api/v1/obo/entries/list",
        json!({ "path": "notes" }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{listing}");
    let (status, top) = delegated(
        &w,
        "sap_interface_bob",
        "/api/v1/obo/entries/list",
        json!({ "drive": w.ada.id }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{top}");
    assert_eq!(names(&top), vec!["plans"]);
    // A folder made through Interface in Bob's files records Interface.
    let (status, made) = delegated(
        &w,
        "sap_interface_bob",
        "/api/v1/obo/folders/create",
        json!({ "operation_id": Uuid::now_v7(), "parent_path": "notes", "name": "from-interface" }),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["path"], "notes/from-interface");
    assert_eq!(made["origin_app_id"], "interface");
    assert_eq!(made["owner"]["id"], w.bob.uuid.as_str());

    // Waveform acting for Bob reaches only apps/waveform/ in Bob's drive.
    let (status, empty) = delegated(
        &w,
        "sap_waveform_bob",
        "/api/v1/obo/entries/list",
        json!({ "path": "apps/waveform" }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["items"], json!([]));
    assert_eq!(empty["folder"]["path"], "apps/waveform");
    assert_eq!(empty["folder"]["type"], "folder");
    assert!(empty["folder"]["id"].as_str().is_some());
    for body in [
        json!({ "parent_id": plans }),
        json!({ "parent_id": notes }),
        json!({ "path": "notes" }),
    ] {
        let (status, response) = delegated(
            &w,
            "sap_waveform_bob",
            "/api/v1/obo/entries/list",
            body.clone(),
        )
        .await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}: {response}");
    }
    let (status, refused) = delegated(
        &w,
        "sap_waveform_bob",
        "/api/v1/obo/entries/list",
        json!({ "drive": w.ada.id }),
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(code(&refused), "drive_not_delegated");
    let (status, made) = delegated(
        &w,
        "sap_waveform_bob",
        "/api/v1/obo/folders/create",
        json!({ "operation_id": Uuid::now_v7(), "parent_path": "", "name": "takes" }),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["path"], "apps/waveform/takes");
    assert_eq!(made["origin_app_id"], "waveform");

    // Trash moves only what the acting app created: Interface may not trash
    // Waveform's folder even though Bob could.
    let takes = made["id"].as_str().unwrap_or_default().to_owned();
    let (status, body) = delegated(
        &w,
        "sap_interface_bob",
        "/api/v1/obo/entries/trash",
        json!({ "operation_id": Uuid::now_v7(), "entry_id": takes }),
    )
    .await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "not_created_by_app");
    let (status, body) = delegated(
        &w,
        "sap_waveform_bob",
        "/api/v1/obo/entries/trash",
        json!({ "operation_id": Uuid::now_v7(), "entry_id": takes }),
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // A first-party app acting for a Carbon opens the drives of the Silicons
    // it looks after, like the Carbon itself.
    w.folder(&w.scout, "scout-notes", None, None).await?;
    let (status, scout) = delegated(
        &w,
        "sap_interface_ada",
        "/api/v1/obo/entries/list",
        json!({ "drive": w.scout.id }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{scout}");
    assert_eq!(names(&scout), vec!["scout-notes"]);

    // One scope per route: a proof without the route's scope is refused.
    proof(
        &w,
        "sap_listonly_for_bob",
        "interface",
        &w.bob,
        "carbon",
        &["briefcase.entries.list"],
    )
    .await;
    let (status, refused) = delegated(
        &w,
        "sap_listonly_for_bob",
        "/api/v1/obo/folders/create",
        json!({ "operation_id": Uuid::now_v7(), "parent_path": "notes", "name": "nope" }),
    )
    .await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(code(&refused), "proof_scope_missing");
    Ok(())
}

#[tokio::test]
async fn the_byte_transfer_finds_its_drive_from_the_reservation() -> anyhow::Result<()> {
    let Some(w) = World::with_issuers(ISSUERS).await? else {
        return Ok(());
    };
    proof(
        &w,
        "sap_waveform_bob_upload",
        "waveform",
        &w.bob,
        "carbon",
        &["briefcase.uploads.reserve"],
    )
    .await;
    let bytes = b"take one";
    let digest = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes));
    let (status, reserved) = delegated(
        &w,
        "sap_waveform_bob_upload",
        "/api/v1/obo/uploads/reserve",
        json!({
            "operation_id": Uuid::now_v7(), "parent_path": "", "name": "take.wav",
            "content_type": "audio/wav", "size": bytes.len(), "sha256": digest,
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{reserved}");
    let upload_id: Uuid = reserved["upload_id"].as_str().unwrap_or_default().parse()?;
    let capability = reserved["capability"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert_eq!(
        w.f.state
            .content_adapter
            .metadata_repository()
            .delegated_upload_drive(upload_id)
            .await?
            .as_deref(),
        Some(w.bob.uuid.as_str())
    );
    let transfer = |upload: Uuid, capability: String, length: usize| {
        Request::builder()
            .method(Method::PUT)
            .uri(format!("/api/v1/obo/uploads/{upload}/content"))
            .header("x-briefcase-upload-capability", capability)
            .header(header::CONTENT_LENGTH, length.to_string())
            .body(Body::from(vec![0_u8; length]))
    };
    // Without X-Org-ID the reservation is found; the capability is checked
    // there (a wrong length is then the first refusal).
    let response =
        w.f.router
            .clone()
            .oneshot(transfer(upload_id, capability, bytes.len() + 1)?)
            .await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    // An upload no drive holds answers 404.
    let response =
        w.f.router
            .clone()
            .oneshot(transfer(Uuid::now_v7(), "0".repeat(64), 4)?)
            .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    Ok(())
}

/// A device owner's Extend proof can read a Silicon's explicitly shared
/// screenshot, without inheriting custody, listing siblings or reaching other apps.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn confined_file_read_honors_foreign_shares_without_custodial_powers() -> anyhow::Result<()> {
    let store = super::memory_store::MemoryStore::default();
    let issuers = "briefcase.files.read=extend briefcase.entries.list=extend \
        briefcase.entries.trash=extend briefcase.folders.create=extend";
    let Some(w) = World::build(issuers, "", Some(std::sync::Arc::new(store.clone()))).await? else {
        return Ok(());
    };
    let scopes = [
        "briefcase.files.read",
        "briefcase.entries.list",
        "briefcase.entries.trash",
        "briefcase.folders.create",
    ];
    proof(
        &w,
        "sap_extend_scout",
        "extend",
        &w.scout,
        "silicon",
        &scopes,
    )
    .await;
    proof(&w, "sap_extend_ada", "extend", &w.ada, "carbon", &scopes).await;
    proof(&w, "sap_extend_bob", "extend", &w.bob, "carbon", &scopes).await;
    proof(
        &w,
        "sap_extend_no_read",
        "extend",
        &w.ada,
        "carbon",
        &["briefcase.entries.list"],
    )
    .await;
    let (status, folder) = delegated(
        &w,
        "sap_extend_scout",
        "/api/v1/obo/folders/create",
        json!({"operation_id": Uuid::now_v7(), "parent_path": "", "name": "screenshots"}),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "{folder}");
    let folder = folder["id"].as_str().unwrap_or_default();
    let scout = in_drive(&w, &w.scout, &w.scout).await?;
    let file = super::archive_tests::publish(
        &w,
        &store,
        &scout,
        folder,
        "screenshot.txt",
        "text/plain",
        b"shared screenshot",
    )
    .await?;
    let private = super::archive_tests::publish(
        &w,
        &store,
        &scout,
        folder,
        "private.txt",
        "text/plain",
        b"private screenshot",
    )
    .await?;
    let outside = w.folder(&w.scout, "outside", None, None).await?;
    let outside_file = super::archive_tests::publish(
        &w,
        &store,
        &scout,
        &outside,
        "other.txt",
        "text/plain",
        b"outside app folder",
    )
    .await?;

    // Ada is Scout's custodian, but the confined proof has no inherited power.
    let (status, _) = delegated(
        &w,
        "sap_extend_ada",
        "/api/v1/obo/files/read",
        json!({"entry_id": file}),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, grant) = w
        .share(&w.scout, &file, "carbon", &w.ada.id, &["read", "update"])
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let (status, _) = w
        .share(&w.scout, &outside_file, "carbon", &w.ada.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);

    // Whole bytes, downloads and body-bound ranges all use the same share.
    for (range, download, expected_status, expected) in [
        (None, false, StatusCode::OK, b"shared screenshot".as_slice()),
        (None, true, StatusCode::OK, b"shared screenshot".as_slice()),
        (
            Some("bytes=7-16"),
            false,
            StatusCode::PARTIAL_CONTENT,
            b"screenshot".as_slice(),
        ),
    ] {
        let response = w
            .f
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/obo/files/read")
                    .header(header::AUTHORIZATION, "Proof sap_extend_ada")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"entry_id": file, "range": range, "download": download}).to_string(),
                    ))?,
            )
            .await?;
        assert_eq!(response.status(), expected_status);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1024)
                .await?
                .as_ref(),
            expected
        );
    }
    for (token, entry, expected) in [
        ("sap_extend_ada", &private, StatusCode::NOT_FOUND),
        ("sap_extend_ada", &outside_file, StatusCode::NOT_FOUND),
        ("sap_extend_bob", &file, StatusCode::NOT_FOUND),
        ("sap_extend_no_read", &file, StatusCode::FORBIDDEN),
    ] {
        let (status, body) = delegated(
            &w,
            token,
            "/api/v1/obo/files/read",
            json!({"entry_id": entry}),
        )
        .await?;
        assert_eq!(status, expected, "{token}: {body}");
    }
    for (uri, body) in [
        ("/api/v1/obo/entries/list", json!({"parent_id": folder})),
        (
            "/api/v1/obo/entries/trash",
            json!({"entry_id": file, "operation_id": Uuid::now_v7()}),
        ),
    ] {
        let (status, body) = delegated(&w, "sap_extend_ada", uri, body).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }
    let grant = grant["id"].as_str().unwrap_or_default();
    let (status, body) = w
        .call(
            &w.scout,
            Method::DELETE,
            &format!("/api/v1/entries/{file}/permissions/{grant}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, body) = delegated(
        &w,
        "sap_extend_ada",
        "/api/v1/obo/files/read",
        json!({"entry_id": file}),
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "revocation must apply immediately: {body}"
    );
    Ok(())
}
