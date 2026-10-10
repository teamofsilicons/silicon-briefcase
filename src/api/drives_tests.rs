//! Personal drives through the router: the owner's rights, shares by id, the
//! custodian rule, "Silicons are not open to the world", link rules, bins,
//! usage, and an account's deletion — every route family checked for the
//! owner, a share, the custodian, and an outsider.
//!
//! ```bash
//! BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5460/briefcase_mig \
//!   cargo test --lib api::drives_tests -- --test-threads=1
//! ```

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use time::OffsetDateTime;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{method, path},
};

use super::accounts_tests::{Fixture, fixture_with_store};
use crate::infrastructure::accounts::test_support::{carbon_summary, silicon_summary};

/// One test world: Ada and Bob are Carbons, Cat is a Carbon nobody shares
/// with, Scout is a Silicon Ada looks after, Rover a Silicon Bob looks after.
pub(super) struct World {
    pub(super) f: Fixture,
    pub(super) ada: Account,
    pub(super) bob: Account,
    pub(super) cat: Account,
    pub(super) scout: Account,
    pub(super) rover: Account,
}

pub(super) struct Account {
    pub(super) uuid: String,
    pub(super) id: String,
    pub(super) token: String,
}

impl World {
    pub(super) async fn new() -> anyhow::Result<Option<Self>> {
        Self::with_issuers("").await
    }

    /// The same world with apps allowed to act through proofs, in
    /// `BRIEFCASE_PROOF_ISSUERS` form.
    pub(super) async fn with_issuers(issuers: &str) -> anyhow::Result<Option<Self>> {
        Self::with_delegation(issuers, "").await
    }

    /// [`World::with_issuers`] with first-party apps
    /// (`BRIEFCASE_PROOF_FIRST_PARTY_ISSUERS` form).
    pub(super) async fn with_delegation(
        issuers: &str,
        first_party: &str,
    ) -> anyhow::Result<Option<Self>> {
        Self::build(issuers, first_party, None).await
    }

    /// The world with file bytes served from `store`.
    pub(super) async fn with_store(
        store: std::sync::Arc<dyn crate::application::ports::ObjectStore>,
    ) -> anyhow::Result<Option<Self>> {
        Self::build("", "", Some(store)).await
    }

    pub(super) async fn build(
        issuers: &str,
        first_party: &str,
        store: Option<std::sync::Arc<dyn crate::application::ports::ObjectStore>>,
    ) -> anyhow::Result<Option<Self>> {
        let Some(f) = fixture_with_store(issuers, first_party, store).await? else {
            return Ok(None);
        };
        let account = |prefix: &str, id: &str, kind: &str| Account {
            uuid: f.uuid(prefix),
            id: f.id(id),
            token: f.issuer.token(&f.uuid(prefix), kind, &f.id(id)),
        };
        let ada = account("Ada", "c:ada", "carbon");
        let bob = account("Bob", "c:bob", "carbon");
        let cat = account("Cat", "c:cat", "carbon");
        let scout = account("Sct", "si:scout", "silicon");
        let rover = account("Rvr", "si:rover", "silicon");
        for carbon in [&ada, &bob, &cat] {
            f.stub_account(
                &carbon.uuid,
                &carbon.id,
                carbon_summary(&carbon.uuid, &carbon.id),
            )
            .await;
        }
        f.stub_account(
            &scout.uuid,
            &scout.id,
            silicon_summary(&scout.uuid, &scout.id, (&ada.uuid, &ada.id)),
        )
        .await;
        f.stub_account(
            &rover.uuid,
            &rover.id,
            silicon_summary(&rover.uuid, &rover.id, (&bob.uuid, &bob.id)),
        )
        .await;
        // Sensitive changes introspect; every token here is still active.
        Mock::given(method("POST"))
            .and(path("/v1/oauth/introspect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "active": true })))
            .mount(&f.server)
            .await;
        let world = Self {
            f,
            ada,
            bob,
            cat,
            scout,
            rover,
        };
        // Each account signs in once, so the cache knows the Silicons'
        // custodians the way a real first request teaches it.
        for who in [
            &world.ada,
            &world.bob,
            &world.cat,
            &world.scout,
            &world.rover,
        ] {
            let (status, body) = world.f.me(&who.token).await?;
            anyhow::ensure!(status == StatusCode::OK, "{body}");
        }
        Ok(Some(world))
    }

    pub(super) async fn call(
        &self,
        who: &Account,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> anyhow::Result<(StatusCode, Value)> {
        self.f.bearer(method, uri, &who.token, body).await
    }

    /// Creates a folder (top of `drive`, or in `parent`) and returns its id.
    pub(super) async fn folder(
        &self,
        who: &Account,
        name: &str,
        parent: Option<&str>,
        drive: Option<&str>,
    ) -> anyhow::Result<String> {
        let mut body = json!({ "name": name });
        if let Some(parent) = parent {
            body["parent_id"] = json!(parent);
        }
        if let Some(drive) = drive {
            body["drive"] = json!(drive);
        }
        let (status, created) = self
            .call(who, Method::POST, "/api/v1/entries", Some(body))
            .await?;
        anyhow::ensure!(status == StatusCode::CREATED, "{status}: {created}");
        Ok(created["id"].as_str().unwrap_or_default().to_owned())
    }

    pub(super) async fn share(
        &self,
        who: &Account,
        entry: &str,
        kind: &str,
        recipient: &str,
        access: &[&str],
    ) -> anyhow::Result<(StatusCode, Value)> {
        self.call(
            who,
            Method::POST,
            &format!("/api/v1/entries/{entry}/invitations"),
            Some(json!({
                "principal": { "type": kind, "id": recipient },
                "access": access,
                "inherit": true,
            })),
        )
        .await
    }
}

pub(super) fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or_default()
}

pub(super) fn names(listing: &Value) -> Vec<String> {
    listing["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["name"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn each_account_owns_one_drive_and_custodians_open_their_silicons_drives()
-> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    // My files: a folder without a parent goes at the top of the caller's
    // drive; `apps` there is kept for apps.
    let notes = w.folder(&w.ada, "notes", None, None).await?;
    let (status, body) = w
        .call(
            &w.ada,
            Method::POST,
            "/api/v1/entries",
            Some(json!({ "name": "apps" })),
        )
        .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(code(&body), "reserved_name");
    for (root_type, expected) in [("public", "public_space_retired"), ("tag", "tags_retired")] {
        let (status, body) = w
            .call(
                &w.ada,
                Method::POST,
                "/api/v1/entries",
                Some(json!({ "name": "x", "root_type": root_type })),
            )
            .await?;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(code(&body), expected);
    }

    // The drives Ada opens with the owner's rights: hers and Scout's.
    let (status, drives) = w.call(&w.ada, Method::GET, "/api/v1/drives", None).await?;
    assert_eq!(status, StatusCode::OK, "{drives}");
    let items = drives["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(items.len(), 2, "{drives}");
    assert_eq!(items[0]["relation"], "own");
    assert_eq!(
        items[0]["storage_external_id"],
        crate::infrastructure::s3::organization_storage_external_id(&w.ada.uuid)
    );
    assert_eq!(items[0]["owner"]["uuid"], w.ada.uuid.as_str());
    assert_eq!(items[1]["relation"], "custodial");
    assert_eq!(items[1]["owner"]["id"], w.scout.id.as_str());
    let ada_root = items[0]["root_id"].as_str().unwrap_or_default().to_owned();
    let (status, root) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("/api/v1/entries/{ada_root}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{root}");
    assert_eq!(root["path"], "");
    assert!(root["parent_id"].is_null());
    let (status, bob_drives) = w.call(&w.bob, Method::GET, "/api/v1/drives", None).await?;
    assert_eq!(status, StatusCode::OK);
    let bob_items = bob_drives["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(bob_items.len(), 2, "Bob looks after Rover: {bob_drives}");
    let (_, scout_drives) = w
        .call(&w.scout, Method::GET, "/api/v1/drives", None)
        .await?;
    assert_eq!(
        scout_drives["items"].as_array().map(Vec::len),
        Some(1),
        "a Silicon has only its own drive: {scout_drives}"
    );

    // Scout keeps its own files; its custodian sees and manages them.
    let scout_work = w.folder(&w.scout, "work", None, None).await?;
    let scout_drive = format!("/api/v1/entries?drive={}", w.scout.id);
    let (status, listing) = w.call(&w.ada, Method::GET, &scout_drive, None).await?;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(names(&listing), vec!["work"]);
    let (status, renamed) = w
        .call(
            &w.ada,
            Method::PATCH,
            &format!("/api/v1/entries/{scout_work}"),
            Some(json!({ "name": "work-reviewed" })),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["path"], "work-reviewed");
    // A custodian may add at the top of its Silicon's drive, recorded as
    // itself, never as the Silicon.
    let (status, added) = w
        .call(
            &w.ada,
            Method::POST,
            "/api/v1/entries",
            Some(json!({ "name": "from-custodian", "drive": w.scout.id })),
        )
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{added}");
    assert_eq!(added["org_id"], w.scout.uuid.as_str());
    assert_eq!(added["owner"]["id"], w.ada.uuid.as_str());
    let (status, usage) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("/api/v1/usage?drive={}", w.scout.id),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{usage}");

    // Nobody else reaches either drive.
    for (who, uri) in [
        (&w.bob, format!("/api/v1/entries/{scout_work}")),
        (&w.bob, format!("/api/v1/entries/{notes}")),
        (&w.cat, format!("/api/v1/entries/{notes}/permissions")),
        (&w.cat, format!("/api/v1/entries/{notes}/invitations")),
        (&w.cat, format!("/api/v1/entries/{notes}/link-access")),
        (&w.cat, format!("/api/v1/entries/{notes}/logs")),
        (&w.cat, format!("/api/v1/entries/{notes}/activity")),
        (&w.cat, format!("/api/v1/usage?drive={}", w.ada.id)),
        (&w.scout, format!("/api/v1/entries/{notes}")),
        (&w.scout, format!("/api/v1/usage?drive={}", w.ada.id)),
    ] {
        let (status, body) = w.call(who, Method::GET, &uri, None).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }
    for (method, uri, body) in [
        (
            Method::PATCH,
            format!("/api/v1/entries/{notes}"),
            Some(json!({ "name": "taken" })),
        ),
        (Method::DELETE, format!("/api/v1/entries/{notes}"), None),
        (
            Method::PUT,
            format!("/api/v1/entries/{notes}/link-access"),
            Some(json!({ "enabled": true })),
        ),
    ] {
        let (status, response) = w.call(&w.cat, method, &uri, body).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {response}");
    }
    let (status, listing) = w
        .call(
            &w.cat,
            Method::GET,
            &format!("/api/v1/entries?drive={}", w.ada.id),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert!(names(&listing).is_empty(), "nothing shared, nothing shown");
    let (status, body) = w
        .call(
            &w.cat,
            Method::POST,
            "/api/v1/entries",
            Some(json!({ "name": "intrusion", "drive": w.ada.id })),
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn shares_by_id_reach_across_drives_and_show_in_shared_with_me() -> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let plans = w.folder(&w.ada, "plans", None, None).await?;
    let secret = w.folder(&w.ada, "secret", None, None).await?;

    // Ada shares by id; the grant names Bob's uuid.
    let (status, grant) = w
        .share(&w.ada, &plans, "carbon", &w.bob.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    assert_eq!(grant["principal"]["id"], w.bob.uuid.as_str());
    let (status, invitations) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("/api/v1/entries/{plans}/invitations"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        invitations["items"][0]["principal"]["id"],
        w.bob.uuid.as_str()
    );
    assert_eq!(
        invitations["items"][0]["principal"]["public_id"],
        w.bob.id.as_str()
    );
    let (status, _) = w
        .call(
            &w.cat,
            Method::GET,
            &format!("/api/v1/entries/{plans}/invitations"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Bob opens it by id, in Ada's drive, and nothing else of hers.
    let (status, entry) = w
        .call(
            &w.bob,
            Method::GET,
            &format!("/api/v1/entries/{plans}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{entry}");
    assert_eq!(entry["org_id"], w.ada.uuid.as_str());
    assert_eq!(entry["effective_access"], json!(["read"]));
    let (status, _) = w
        .call(
            &w.bob,
            Method::GET,
            &format!("/api/v1/entries/{secret}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Shared with me lists it with the drive's owner.
    let (status, shared) = w.call(&w.bob, Method::GET, "/api/v1/shared", None).await?;
    assert_eq!(status, StatusCode::OK, "{shared}");
    let items = shared["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(items.len(), 1, "{shared}");
    assert_eq!(items[0]["owner"]["uuid"], w.ada.uuid.as_str());
    assert_eq!(items[0]["owner"]["id"], w.ada.id.as_str());
    assert_eq!(items[0]["entry"]["id"], plans.as_str());
    let (_, cat_shared) = w.call(&w.cat, Method::GET, "/api/v1/shared", None).await?;
    assert_eq!(cat_shared["items"].as_array().map(Vec::len), Some(0));

    // Read does not add: Bob cannot create in it until Ada adds write.
    let (status, body) = w
        .call(
            &w.bob,
            Method::POST,
            "/api/v1/entries",
            Some(json!({ "name": "draft", "parent_id": plans })),
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = w
        .share(&w.ada, &plans, "carbon", &w.bob.id, &["read", "write"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let draft = w.folder(&w.bob, "draft", Some(&plans), None).await?;
    let (status, draft_entry) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("/api/v1/entries/{draft}"),
            None,
        )
        .await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "the drive's owner sees what Bob created"
    );
    assert_eq!(draft_entry["path"], "plans/draft");
    assert_eq!(draft_entry["owner"]["id"], w.bob.uuid.as_str());
    // Bob, as the creator, may share it onwards and turn its link on; he
    // may not change who reaches Ada's folder.
    let (status, link) = w
        .call(
            &w.bob,
            Method::PUT,
            &format!("/api/v1/entries/{draft}/link-access"),
            Some(json!({ "enabled": true })),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{link}");
    assert_eq!(link["enabled"], true);
    let (status, _) = w
        .call(
            &w.bob,
            Method::PUT,
            &format!("/api/v1/entries/{plans}/link-access"),
            Some(json!({ "enabled": true })),
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = w
        .share(&w.bob, &plans, "carbon", &w.cat.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // The top of a drive never gets a link, even from its owner.
    let (_, drives) = w.call(&w.ada, Method::GET, "/api/v1/drives", None).await?;
    let root = drives["items"][0]["root_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let (status, body) = w
        .call(
            &w.ada,
            Method::PUT,
            &format!("/api/v1/entries/{root}/link-access"),
            Some(json!({ "enabled": true })),
        )
        .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(code(&body), "protected_folder");

    // The drive's owner deletes what anyone created in it; Bob deletes his
    // own, never Ada's.
    let (status, _) = w
        .call(
            &w.bob,
            Method::DELETE,
            &format!("/api/v1/entries/{plans}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = w
        .call(
            &w.bob,
            Method::DELETE,
            &format!("/api/v1/entries/{draft}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // Bins are per drive: the creator and the drive's owner see it there,
    // an outsider does not.
    let bin = format!("/api/v1/bin?drive={}", w.ada.id);
    let (status, bob_bin) = w.call(&w.bob, Method::GET, &bin, None).await?;
    assert_eq!(status, StatusCode::OK, "{bob_bin}");
    assert_eq!(names(&bob_bin), vec!["draft"]);
    let (_, ada_bin) = w.call(&w.ada, Method::GET, "/api/v1/bin", None).await?;
    assert_eq!(names(&ada_bin), vec!["draft"]);
    let (_, cat_bin) = w.call(&w.cat, Method::GET, &bin, None).await?;
    assert!(names(&cat_bin).is_empty());
    let (status, restored) = w
        .call(
            &w.bob,
            Method::POST,
            &format!("/api/v1/bin/{draft}/restore"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{restored}");

    // Bob's own inbox, in his own drive, holds the share with Ada's drive.
    let (status, inbox) = w
        .call(&w.bob, Method::GET, "/api/v1/notifications", None)
        .await?;
    assert_eq!(status, StatusCode::OK, "{inbox}");
    let granted = inbox["items"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["kind"] == "access_granted"))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(granted["subject"]["drive"], w.ada.uuid.as_str(), "{inbox}");
    assert!(
        granted["subject"]["permanent_url"]
            .as_str()
            .is_some_and(|url| url.ends_with(&format!("/{}/plans", w.ada.id))),
        "{granted}"
    );

    // Permission inspection spans drives: Bob's own folder and Ada's share.
    let mine = w.folder(&w.bob, "mine", None, None).await?;
    let (status, inspected) = w
        .call(
            &w.bob,
            Method::POST,
            "/api/v1/permissions/effective",
            Some(json!({ "entry_ids": [mine, plans, secret] })),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{inspected}");
    let found: Vec<&str> = inspected["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["entry_id"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        found.contains(&mine.as_str()) && found.contains(&plans.as_str()),
        "{inspected}"
    );
    assert!(!found.contains(&secret.as_str()), "{inspected}");

    // Search covers the drives shared with Bob.
    let (status, results) = w
        .call(&w.bob, Method::GET, "/api/v1/search?q=plans", None)
        .await?;
    assert_eq!(status, StatusCode::OK, "{results}");
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn silicons_accept_shares_from_their_circle_or_accounts_they_allowed() -> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let ada_folder = w.folder(&w.ada, "for-scout", None, None).await?;
    let bob_folder = w.folder(&w.bob, "for-scout", None, None).await?;
    let bob_other = w.folder(&w.bob, "bob-only", None, None).await?;

    // Ada looks after Scout: her share goes through.
    let (status, body) = w
        .share(
            &w.ada,
            &ada_folder,
            "silicon",
            &w.scout.id,
            &["read", "write"],
        )
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // Bob is outside Scout's circle: refused until Scout or Ada allows him.
    let (status, body) = w
        .share(
            &w.bob,
            &bob_folder,
            "silicon",
            &w.scout.id,
            &["read", "write"],
        )
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "silicon_not_open");
    // A Carbon can be reached by anyone signed in.
    let (status, _) = w
        .share(&w.bob, &bob_folder, "carbon", &w.cat.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    // Rover is Bob's own Silicon.
    let (status, _) = w
        .share(&w.bob, &bob_folder, "silicon", &w.rover.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);

    // Only Scout or its custodian manages who may share with it.
    let allowance = format!(
        "/api/v1/silicons/{}/allowed-accounts/{}",
        w.scout.id, w.bob.id
    );
    let (status, body) = w.call(&w.bob, Method::PUT, &allowance, None).await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "not_silicons_custodian");
    let (status, _) = w.call(&w.ada, Method::PUT, &allowance, None).await?;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = w.call(&w.scout, Method::PUT, &allowance, None).await?;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "allowing again changes nothing"
    );
    let (status, list) = w
        .call(
            &w.scout,
            Method::GET,
            &format!("/api/v1/silicons/{}/allowed-accounts", w.scout.id),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"][0]["account"]["uuid"], w.bob.uuid.as_str());
    let (status, body) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("/api/v1/silicons/{}/allowed-accounts", w.ada.id),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(code(&body), "not_a_silicon");

    let (status, body) = w
        .share(
            &w.bob,
            &bob_folder,
            "silicon",
            &w.scout.id,
            &["read", "write"],
        )
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Scout works in Bob's folder. What Scout creates there carries creator
    // rights for Ada, its custodian; the rest of Bob's drive stays closed.
    let report = w
        .folder(&w.scout, "scout-report", Some(&bob_folder), None)
        .await?;
    let (status, seen) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("/api/v1/entries/{report}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!(seen["org_id"], w.bob.uuid.as_str());
    // Bob's folder shows only as the way to Scout's report: its name and
    // place, nothing else; Bob's other files stay out of reach.
    let (status, traversed) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("/api/v1/entries/{bob_folder}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{traversed}");
    assert_eq!(traversed["visibility"], "traversal", "{traversed}");
    assert_eq!(traversed["effective_access"], json!([]));
    assert!(traversed["owner"].is_null());
    let (status, _) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("/api/v1/entries/{bob_other}"),
            None,
        )
        .await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the custodian has no share in Bob's drive"
    );
    let (status, _) = w
        .call(
            &w.ada,
            Method::DELETE,
            &format!("/api/v1/entries/{report}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Removing the allowance keeps existing shares; new ones are refused.
    let (status, _) = w.call(&w.scout, Method::DELETE, &allowance, None).await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = w.call(&w.scout, Method::DELETE, &allowance, None).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let another = w.folder(&w.bob, "another", None, None).await?;
    let (status, body) = w
        .share(&w.bob, &another, "silicon", &w.scout.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = w
        .call(
            &w.scout,
            Method::GET,
            &format!("/api/v1/entries/{bob_folder}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "the earlier share stays");
    Ok(())
}

#[tokio::test]
async fn a_deleted_accounts_drive_is_erased_and_its_shares_end() -> anyhow::Result<()> {
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    let bob_files = w.folder(&w.bob, "bob-files", None, None).await?;
    let ada_folder = w.folder(&w.ada, "for-bob", None, None).await?;
    let (status, _) = w
        .share(&w.ada, &ada_folder, "carbon", &w.bob.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = w
        .share(&w.bob, &bob_files, "carbon", &w.cat.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);

    let status =
        w.f.deliver(
            "account.deleted",
            OffsetDateTime::now_utc(),
            &json!({ "uuid": w.bob.uuid, "kind": "carbon" }),
        )
        .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Bob's drive is gone for good (purged by the worker), his share in
    // Ada's drive ended, and the share he made ended with his drive.
    let erased: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE deleted_at IS NULL), \
                count(*) FILTER (WHERE purge_after <= clock_timestamp()) \
           FROM briefcase.entries WHERE org_id = $1",
    )
    .bind(&w.bob.uuid)
    .fetch_one(&w.f.admin)
    .await?;
    assert_eq!(erased.0, 0, "nothing in Bob's drive is active");
    assert!(erased.1 >= 2, "his root and folder are due for purge");
    let live_to_bob: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM briefcase.permission_grants \
          WHERE principal_id = $1 AND revoked_at IS NULL",
    )
    .bind(&w.bob.uuid)
    .fetch_one(&w.f.admin)
    .await?;
    assert_eq!(live_to_bob, 0);
    let (status, _) = w
        .call(
            &w.cat,
            Method::GET,
            &format!("/api/v1/entries/{bob_files}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = w.f.me(&w.bob.token).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(code(&body), "account_deleted");
    // Ada's folder is untouched.
    let (status, _) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("/api/v1/entries/{ada_folder}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn custodians_find_silicons_the_cutover_recorded_before_they_signed_in() -> anyhow::Result<()>
{
    let Some(w) = World::new().await? else {
        return Ok(());
    };
    // link-identities records each linked Silicon with its kind and id but
    // without its custodian, and never as freshly confirmed.
    let (uuid, id) = (w.f.uuid("Rec"), w.f.id("si:recorded"));
    w.f.stub_account(
        &uuid,
        &id,
        silicon_summary(&uuid, &id, (&w.ada.uuid, &w.ada.id)),
    )
    .await;
    sqlx::query(
        "INSERT INTO briefcase.accounts (account_uuid, kind, public_id, refreshed_at) \
         VALUES ($1, 'silicon', $2, 'epoch')",
    )
    .bind(&uuid)
    .bind(&id)
    .execute(&w.f.admin)
    .await?;
    let (status, drives) = w.call(&w.ada, Method::GET, "/api/v1/drives", None).await?;
    assert_eq!(status, StatusCode::OK, "{drives}");
    let custodial: Vec<&str> = drives["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| item["relation"] == "custodial")
                .filter_map(|item| item["owner"]["uuid"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(custodial.contains(&uuid.as_str()), "{drives}");
    assert_eq!(
        w.f.account_row(&uuid).await?["custodian_uuid"],
        w.ada.uuid.as_str()
    );
    Ok(())
}
