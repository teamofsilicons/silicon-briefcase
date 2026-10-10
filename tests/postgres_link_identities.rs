//! The cutover, end to end in PostgreSQL: a database migrated with
//! origin/main's migrations (0001–0047) and filled with an organization is
//! upgraded with the new migrations, then `link-identities` moves it into
//! personal drives — as a non-superuser, row-security-bound migrator, exactly
//! like production.
//!
//! Skipped unless `BRIEFCASE_TEST_DATABASE_URL` names a superuser connection
//! (each test creates and drops its own database).
//!
//! ```bash
//! BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5460/briefcase_mig \
//!   cargo test --test postgres_link_identities -- --test-threads=1
//! ```

use std::{borrow::Cow, str::FromStr as _};

use serde_json::{Value, json};
use silicon_briefcase::infrastructure::postgres::{
    self,
    link_identities::{LinkError, link_identities, parse_mapping},
};
use sqlx::{
    Connection as _, Executor as _, PgConnection, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use uuid::Uuid;

const MIGRATOR: &str = "briefcase_test_migrator";
const MIGRATOR_PASSWORD: &str = "briefcase-test-migrator-local-only";
/// The last migration origin/main shipped before Silicon Accounts.
const ORIGIN_MAIN_LAST_MIGRATION: i64 = 47;

const NOTES: &str = "01900000-0000-7000-8000-000000000203";
const GUIDE: &str = "01900000-0000-7000-8000-000000000202";
const TODO: &str = "01900000-0000-7000-8000-000000000206";

/// Saket by his IAM principal UUID, Bob and Scout by public id, a line that
/// matches nobody; c:gone has no account and c:ex was removed.
const MAPPING: &str = "iam_principal_id,accounts_uuid,kind,accounts_id
01900000-0000-7000-8000-000000000001,Saq1,carbon,c:saket
c:bob,Bob1,,c:bob
si:scout,Sct1,silicon,si:scout
c:nobody,Nob1,,
";

/// The same, after the operator found c:gone's account.
const CORRECTED: &str = "iam_principal_id,accounts_uuid,kind,accounts_id
01900000-0000-7000-8000-000000000001,Saq1,carbon,c:saket
c:bob,Bob1,,c:bob
si:scout,Sct1,silicon,si:scout
c:nobody,Nob1,,
c:gone,Gon1,carbon,c:gone
";

/// A throwaway database owned by a non-superuser, non-BYPASSRLS migrator.
struct World {
    admin: PgPool,
    database: String,
    migrator: PgPool,
}

impl World {
    /// Creates the database, migrates it to origin/main's schema, loads the
    /// legacy organization when `fixture`, then applies the new migrations.
    async fn create(fixture: bool) -> anyhow::Result<Option<Self>> {
        let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
            eprintln!("skipping: BRIEFCASE_TEST_DATABASE_URL is not set");
            return Ok(None);
        };
        let options = PgConnectOptions::from_str(&url)?;
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone())
            .await?;
        let superuser: bool = sqlx::query_scalar(
            "SELECT rolsuper FROM pg_catalog.pg_roles WHERE rolname = current_user",
        )
        .fetch_one(&admin)
        .await?;
        let runtime_roles: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_catalog.pg_roles \
              WHERE rolname IN ('briefcase_api', 'briefcase_worker')",
        )
        .fetch_one(&admin)
        .await?;
        if !superuser || runtime_roles != 2 {
            eprintln!("skipping: needs a superuser connection and the runtime roles");
            return Ok(None);
        }
        admin
            .execute(sqlx::AssertSqlSafe(format!(
                "DO $$ BEGIN \
                   IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = '{MIGRATOR}') THEN \
                     CREATE ROLE {MIGRATOR} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE; \
                   END IF; \
                 END $$"
            )))
            .await?;
        admin
            .execute(sqlx::AssertSqlSafe(format!(
                "ALTER ROLE {MIGRATOR} PASSWORD '{MIGRATOR_PASSWORD}'"
            )))
            .await?;
        let database = format!(
            "briefcase_link_{}",
            &Uuid::now_v7().simple().to_string()[16..]
        );
        admin
            .execute(sqlx::AssertSqlSafe(format!(
                "CREATE DATABASE {database} OWNER {MIGRATOR}"
            )))
            .await?;
        let migrator = PgPoolOptions::new()
            .max_connections(2)
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("SELECT set_config('search_path', 'public', false)")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(
                options
                    .clone()
                    .database(&database)
                    .username(MIGRATOR)
                    .password(MIGRATOR_PASSWORD),
            )
            .await?;
        let world = Self {
            admin,
            database,
            migrator,
        };
        // origin/main's schema, then (optionally) its data, then ours.
        let mut origin_main = sqlx::migrate!("./migrations");
        origin_main.migrations = Cow::Owned(
            origin_main
                .migrations
                .iter()
                .filter(|migration| migration.version <= ORIGIN_MAIN_LAST_MIGRATION)
                .cloned()
                .collect(),
        );
        origin_main.run(&world.migrator).await?;
        if fixture {
            let mut session = world.superuser().await?;
            let mut transaction = session.begin().await?;
            sqlx::raw_sql(include_str!("fixtures/legacy_organization.sql"))
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
        }
        postgres::migrate(&world.migrator).await?;
        Ok(Some(world))
    }

    async fn superuser(&self) -> anyhow::Result<PgConnection> {
        let options = (*self.admin.connect_options())
            .clone()
            .database(&self.database);
        Ok(PgConnection::connect_with(&options).await?)
    }

    async fn link(&self, mapping: &str, dry_run: bool) -> Result<Value, LinkError> {
        let mapping =
            parse_mapping(mapping).map_err(|error| LinkError::Refused(error.to_string()))?;
        link_identities(&self.migrator, &mapping, dry_run).await
    }

    async fn value(&self, sql: &'static str) -> anyhow::Result<Value> {
        let mut session = self.superuser().await?;
        Ok(sqlx::query_scalar::<_, Option<Value>>(sql)
            .fetch_one(&mut session)
            .await?
            .unwrap_or(Value::Null))
    }

    /// Everything the cutover may change, by path rather than by generated
    /// id, so two databases can be compared.
    async fn snapshot(&self) -> anyhow::Result<Value> {
        self.value(include_str!("fixtures/link_snapshot.sql")).await
    }

    async fn drop(self) -> anyhow::Result<()> {
        self.migrator.close().await;
        self.admin
            .execute(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                self.database
            )))
            .await?;
        self.admin.close().await;
        Ok(())
    }
}

fn paths(snapshot: &Value, drive: &str) -> Vec<String> {
    snapshot["entries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| entry[0] == drive)
                .filter_map(|entry| entry[1].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn rows<'a>(snapshot: &'a Value, key: &str) -> Vec<&'a Value> {
    snapshot[key]
        .as_array()
        .map(|rows| rows.iter().collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn migrations_apply_to_an_empty_database_and_upgrade_origin_main() -> anyhow::Result<()> {
    let Some(empty) = World::create(false).await? else {
        return Ok(());
    };
    let applied = empty
        .value("SELECT to_jsonb(count(*)) FROM _sqlx_migrations WHERE success")
        .await?;
    assert_eq!(applied, json!(52));
    empty.drop().await?;

    let Some(upgraded) = World::create(true).await? else {
        return Ok(());
    };
    // The upgrade changes no existing row: the organization, its members and
    // files are all where origin/main left them, and the bucket settings
    // gained the External ID their trust policy already names.
    let state = upgraded
        .value(
            "SELECT jsonb_build_object( \
                 'entries', (SELECT count(*) FROM briefcase.entries WHERE org_id = 'tos'), \
                 'members', (SELECT count(*) FROM briefcase.organization_members WHERE org_id = 'tos'), \
                 'owner', (SELECT owner_account_uuid FROM briefcase.organizations WHERE org_id = 'tos'), \
                 'external_id', (SELECT external_id FROM briefcase.organization_storage_configs), \
                 'runs', (SELECT count(*) FROM briefcase.identity_link_runs))",
        )
        .await?;
    assert_eq!(state["entries"], 23);
    assert_eq!(state["members"], 5);
    assert!(state["owner"].is_null());
    assert_eq!(state["runs"], 0);
    // SHA-256 of "silicon-briefcase:byo-storage:v1\0tos", computed outside.
    assert_eq!(
        state["external_id"],
        "161cc38ed0bde70339b844c277ce421849f5f1b772715662d575f8214ee7fe98"
    );
    upgraded.drop().await?;
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn link_identities_moves_the_organization_into_personal_drives() -> anyhow::Result<()> {
    let Some(w) = World::create(true).await? else {
        return Ok(());
    };
    let report = w.link(MAPPING, false).await?;
    assert_eq!(report["status"], "applied", "{report:#}");
    assert_eq!(report["dry_run"], false);
    let linked: Vec<(i64, &str, &str)> = rows(&report, "linked")
        .into_iter()
        .map(|row| {
            (
                row["line"].as_i64().unwrap_or_default(),
                row["legacy_id"].as_str().unwrap_or_default(),
                row["accounts_uuid"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        linked,
        vec![
            (2, "c:saket", "Saq1"),
            (3, "c:bob", "Bob1"),
            (4, "si:scout", "Sct1")
        ]
    );
    let unmatched = rows(&report, "unmatched");
    assert_eq!(unmatched.len(), 1, "{report:#}");
    assert_eq!(unmatched[0]["line"], 5);
    assert_eq!(unmatched[0]["key"], "c:nobody");
    let unlinked: Vec<&str> = rows(&report, "unlinked")
        .into_iter()
        .filter_map(|row| row["legacy_id"].as_str())
        .collect();
    assert_eq!(unlinked, vec!["c:ex", "c:gone"]);
    let organization = &report["organizations"][0];
    assert_eq!(organization["organization"], "tos");
    assert_eq!(organization["owner"], "Saq1");
    assert_eq!(organization["moved_entries"], 11);
    assert_eq!(organization["link_shared_entries"], 1);
    let spaces: Vec<(String, String, u64)> = rows(organization, "spaces")
        .into_iter()
        .map(|space| {
            (
                space["legacy_path"].as_str().unwrap_or_default().to_owned(),
                space["path"].as_str().unwrap_or_default().to_owned(),
                space["shared_with"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        spaces,
        vec![
            (
                "apps/waveform/public".to_owned(),
                "apps/waveform/tos-public".to_owned(),
                2
            ),
            ("design".to_owned(), "tos-design".to_owned(), 1),
            ("public".to_owned(), "tos-public".to_owned(), 2),
        ]
    );
    assert_eq!(organization["unclaimed"][0]["legacy_id"], "c:gone");
    assert_eq!(organization["unclaimed"][0]["path"], "unclaimed/c:gone");
    assert_eq!(organization["unclaimed"][0]["entries"], 1);
    assert_eq!(
        organization["renamed"][0]["legacy_path"],
        "private/c:saket/apps"
    );
    assert_eq!(organization["renamed"][0]["name"], "apps (from tos)");
    assert_eq!(report["drives_created"], 3);
    assert_eq!(report["notifications_moved"], 1);
    assert_eq!(report["tag_shares_expanded"], 1);
    assert_eq!(report["snapshot_shares"], 5);

    let snapshot = w.snapshot().await?;
    let sorted = |drive: &str| {
        let mut found = paths(&snapshot, drive);
        found.sort();
        found
    };
    assert_eq!(
        sorted("Saq1"),
        vec![
            ".",
            "apps",
            "apps (from tos)",
            "apps/waveform",
            "apps/waveform/tos-public",
            "apps/waveform/tos-public/take.wav",
            "big.bin",
            "notes",
            "tos-design",
            "tos-design/mockups",
            "tos-public",
            "tos-public/handbook",
            "tos-public/handbook/guide.md",
            "unclaimed",
            "unclaimed/c:gone",
            "unclaimed/c:gone/old.txt",
        ]
    );
    assert_eq!(
        sorted("Bob1"),
        vec![
            ".",
            "apps",
            "apps/waveform",
            "apps/waveform/bob-take.wav",
            "todo.txt"
        ]
    );
    assert_eq!(sorted("Sct1"), vec![".", "run.log"]);
    // Only the organization's structural folders stay behind.
    assert_eq!(
        sorted("tos"),
        vec![
            "apps",
            "apps/waveform",
            "apps/waveform/private",
            "apps/waveform/private/c:bob",
            "apps/waveform/public",
            "design",
            "private",
            "private/c:bob",
            "private/c:gone",
            "private/c:saket",
            "private/si:scout",
            "public",
        ]
    );
    let entry = |drive: &str, path: &str| {
        rows(&snapshot, "entries")
            .into_iter()
            .find(|entry| entry[0] == drive && entry[1] == path)
            .cloned()
            .unwrap_or(Value::Null)
    };
    // Creators keep their files; a member without an account keeps its id.
    let guide = entry("Saq1", "tos-public/handbook/guide.md");
    assert_eq!(guide[4], "Bob1");
    assert_eq!(guide[7], "private");
    assert_eq!(guide[9], true, "link sharing survives");
    assert_eq!(entry("Saq1", "unclaimed/c:gone/old.txt")[4], "c:gone");
    assert_eq!(
        entry("Saq1", "apps/waveform/tos-public/take.wav")[10],
        "waveform"
    );
    assert_eq!(entry("Bob1", "apps/waveform")[3], "app_root");
    assert!(
        entry("Saq1", "tos-design/mockups")[8].is_null(),
        "tags are gone"
    );

    let grants: Vec<Value> = rows(&snapshot, "grants")
        .into_iter()
        .filter(|share| share[7] == true)
        .map(|share| json!([share[0], share[1], share[3], share[4], share[5]]))
        .collect();
    assert_eq!(
        grants,
        vec![
            json!(["Saq1", "apps/waveform/tos-public", "Bob1", 3, true]),
            json!(["Saq1", "apps/waveform/tos-public", "Sct1", 3, true]),
            // Saket's share (read) gained what the design tag gave (write).
            json!(["Saq1", "notes", "Bob1", 3, true]),
            json!(["Saq1", "tos-design", "Bob1", 3, true]),
            json!(["Saq1", "tos-public", "Bob1", 3, true]),
            json!(["Saq1", "tos-public", "Sct1", 3, true]),
        ]
    );
    assert_eq!(
        rows(&snapshot, "tag_grants"),
        vec![&json!(["Saq1", "notes", "tag-design", false, "Saq1"])]
    );
    let notification = rows(&snapshot, "notifications")[0].clone();
    assert_eq!(notification[0], "Bob1");
    assert_eq!(notification[1], "Bob1");
    assert_eq!(notification[4]["drive"], "Saq1");
    assert_eq!(notification[4]["path"], "notes");
    assert_eq!(
        rows(&snapshot, "usage"),
        vec![
            &json!(["Bob1", 620, null, null]),
            &json!(["Saq1", 1640, null, 5_000_000]),
            &json!(["Sct1", 30, null, null]),
            &json!(["tos", 0, null, 5_000_000]),
        ]
    );
    let configs: Vec<Value> = rows(&snapshot, "configs")
        .into_iter()
        .map(|config| json!([config[0], config[2], config[3], config[4]]))
        .collect();
    let external = "161cc38ed0bde70339b844c277ce421849f5f1b772715662d575f8214ee7fe98";
    assert_eq!(
        configs,
        vec![
            json!(["Bob1", "superseded", external, "Saq1"]),
            json!(["Saq1", "active", external, "Saq1"]),
            json!(["tos", "active", external, "Saq1"]),
        ]
    );
    assert_eq!(
        rows(&snapshot, "multipart"),
        vec![&json!(["Saq1", ".", "big.bin", "Saq1"])]
    );
    for alias in [
        json!(["tos", "private/c:saket", "Saq1", "."]),
        json!(["tos", "private/c:saket/notes", "Saq1", "notes"]),
        json!(["tos", "private/me:tos/notes", "Saq1", "notes"]),
        json!(["tos", "public", "Saq1", "tos-public"]),
        json!([
            "tos",
            "public/handbook/guide.md",
            "Saq1",
            "tos-public/handbook/guide.md"
        ]),
        json!(["tos", "private/c:gone", "Saq1", "unclaimed/c:gone"]),
        json!([
            "tos",
            "apps/waveform/private/c:bob",
            "Bob1",
            "apps/waveform"
        ]),
        json!([
            "tos",
            "apps/waveform/private/c:bob/bob-take.wav",
            "Bob1",
            "apps/waveform/bob-take.wav"
        ]),
    ] {
        assert!(rows(&snapshot, "aliases").contains(&&alias), "{alias}");
    }
    assert_eq!(
        rows(&snapshot, "accounts"),
        vec![
            &json!(["Bob1", "carbon", "c:bob"]),
            &json!(["Saq1", "carbon", "c:saket"]),
            &json!(["Sct1", "silicon", "si:scout"]),
        ]
    );
    assert_eq!(
        rows(&snapshot, "search"),
        vec![&json!(["Saq1", "tos-public/handbook/guide.md"])]
    );

    // Everything the run created carries a UUIDv7, as Briefcase requires.
    let not_v7 = w
        .value(
            "SELECT to_jsonb(count(*)) FROM ( \
                 SELECT grant_id AS id FROM briefcase.permission_grants \
                 UNION ALL SELECT entry_id FROM briefcase.entries) AS ids \
              WHERE substr(id::text, 15, 1) <> '7'",
        )
        .await?;
    assert_eq!(not_v7, json!(0));

    // The API's role finds it all where the new service looks.
    let mut api = w.superuser().await?;
    api.execute("SET ROLE briefcase_api").await?;
    let located: Vec<(String, String)> = sqlx::query_as(
        "SELECT drive, path FROM briefcase.resolve_legacy_location('tos', 'public/handbook/guide.md') \
         UNION ALL SELECT drive, path FROM briefcase.resolve_legacy_location('tos', 'private/c:saket/notes/2027/plan.md') \
         UNION ALL SELECT drive, path FROM briefcase.resolve_legacy_location('tos', '/private/c:saket/') \
         UNION ALL SELECT drive, path FROM briefcase.resolve_legacy_location('tos', 'private/me:tos/notes') \
         UNION ALL SELECT drive, path FROM briefcase.resolve_legacy_location('tos', 'nothing/here')",
    )
    .fetch_all(&mut api)
    .await?;
    assert_eq!(
        located,
        vec![
            ("Saq1".to_owned(), "tos-public/handbook/guide.md".to_owned()),
            ("Saq1".to_owned(), "notes/2027/plan.md".to_owned()),
            ("Saq1".to_owned(), ".".to_owned()),
            ("Saq1".to_owned(), "notes".to_owned()),
        ]
    );
    let drive: Option<String> = sqlx::query_scalar("SELECT briefcase.entry_drive($1::uuid)")
        .bind(GUIDE)
        .fetch_one(&mut api)
        .await?;
    assert_eq!(drive.as_deref(), Some("Saq1"));
    sqlx::query("SELECT set_config('briefcase.org_id', 'Saq1', false)")
        .execute(&mut api)
        .await?;
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM briefcase.entries")
        .fetch_one(&mut api)
        .await?;
    assert_eq!(visible, 16);
    drop(api);
    w.drop().await?;
    Ok(())
}

#[tokio::test]
async fn a_dry_run_and_the_same_mapping_again_change_nothing() -> anyhow::Result<()> {
    let Some(w) = World::create(true).await? else {
        return Ok(());
    };
    let before = w.snapshot().await?;
    let dry = w.link(MAPPING, true).await?;
    assert_eq!(dry["status"], "applied");
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["organizations"][0]["moved_entries"], 11);
    assert_eq!(w.snapshot().await?, before, "a dry run rolls back");
    assert_eq!(
        w.value("SELECT to_jsonb(count(*)) FROM briefcase.identity_link_runs")
            .await?,
        json!(0)
    );

    let applied = w.link(MAPPING, false).await?;
    let after = w.snapshot().await?;
    assert_ne!(after, before);
    let again = w.link(MAPPING, false).await?;
    assert_eq!(again["status"], "already_applied", "{again:#}");
    assert_eq!(again["applied_run"], applied["run_id"]);
    assert_eq!(w.snapshot().await?, after, "the same mapping is idempotent");
    let dry_again = w.link(MAPPING, true).await?;
    assert_eq!(dry_again["status"], "already_applied");
    w.drop().await?;
    Ok(())
}

#[tokio::test]
async fn a_corrected_mapping_undoes_the_run_in_place_first() -> anyhow::Result<()> {
    let Some(corrected) = World::create(true).await? else {
        return Ok(());
    };
    let first = corrected.link(MAPPING, false).await?;
    let second = corrected.link(CORRECTED, false).await?;
    assert_eq!(second["status"], "applied", "{second:#}");
    assert_eq!(second["reverted_run"], first["run_id"]);
    assert!(second["reverted_changes"].as_u64().unwrap_or_default() > 0);
    assert_eq!(second["organizations"][0]["unclaimed"], json!([]));
    let runs = corrected
        .value(
            "SELECT jsonb_agg(jsonb_build_array(status, reverted_by_run IS NOT NULL) ORDER BY started_at) \
               FROM briefcase.identity_link_runs",
        )
        .await?;
    assert_eq!(runs, json!([["reverted", true], ["applied", false]]));

    let Some(fresh) = World::create(true).await? else {
        return Ok(());
    };
    fresh.link(CORRECTED, false).await?;
    let (undone, direct) = (corrected.snapshot().await?, fresh.snapshot().await?);
    for key in direct
        .as_object()
        .map(|map| map.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default()
    {
        assert_eq!(
            undone[&key], direct[&key],
            "{key} differs after undoing the first run"
        );
    }
    let mut gone = paths(&direct, "Gon1");
    gone.sort();
    assert_eq!(gone, vec![".", "old.txt"]);
    corrected.drop().await?;
    fresh.drop().await?;
    Ok(())
}

#[tokio::test]
async fn link_identities_refuses_with_the_exact_reason_and_changes_nothing() -> anyhow::Result<()> {
    let Some(w) = World::create(true).await? else {
        return Ok(());
    };
    let before = w.snapshot().await?;
    for (mapping, fragment) in [
        (
            "iam_principal_id,accounts_uuid\nc:bob,Bob1\n",
            "the owner c:saket of organization tos is not linked to an account",
        ),
        (
            "iam_principal_id,accounts_uuid\nc:saket,Saq1\nc:bob,Saq1\n",
            "account Saq1 is already linked to c:saket on line 2",
        ),
        (
            "iam_principal_id,accounts_uuid\nc:saket,tos\n",
            "also the key of an organization from before personal drives",
        ),
        (
            "iam_principal_id,accounts_uuid,kind\nc:saket,Saq1,silicon\n",
            "c:saket is a carbon id but the line says silicon",
        ),
    ] {
        match w.link(mapping, false).await {
            Err(LinkError::Refused(reason)) => assert!(reason.contains(fragment), "{reason}"),
            other => panic!("{mapping}: expected a refusal, got {other:?}"),
        }
    }
    let mut session = w.superuser().await?;
    session
        .execute(
            "INSERT INTO briefcase.outbox_events (org_id, event_id, topic, aggregate_type, aggregate_id, payload) \
             VALUES ('tos', gen_random_uuid(), 'briefcase.invitation-email.v1', 'notification', 'x', '{}')",
        )
        .await?;
    match w.link(MAPPING, false).await {
        Err(LinkError::Refused(reason)) => assert!(reason.contains("still in flight"), "{reason}"),
        other => panic!("expected the in-flight refusal, got {other:?}"),
    }
    session
        .execute("DELETE FROM briefcase.outbox_events WHERE aggregate_id = 'x'")
        .await?;
    assert_eq!(w.snapshot().await?, before);

    // Without a linked owner, `org:` names who receives the shared spaces;
    // everyone without an account goes to unclaimed/.
    let report = w
        .link(
            "iam_principal_id,accounts_uuid\nc:bob,Bob1\norg:tos,Bob1\n",
            false,
        )
        .await?;
    assert_eq!(report["organizations"][0]["owner"], "Bob1");
    let unclaimed: Vec<&str> = rows(&report["organizations"][0], "unclaimed")
        .into_iter()
        .filter_map(|area| area["legacy_id"].as_str())
        .collect();
    assert_eq!(unclaimed, vec!["c:gone", "c:saket", "si:scout"]);

    // Once Briefcase has been used in those drives, a corrected mapping can
    // no longer undo the run.
    session
        .execute(sqlx::AssertSqlSafe(format!(
            "INSERT INTO briefcase.audit_events (org_id, audit_id, entry_id, actor_type, actor_id, action, request_id) \
             VALUES ('Bob1', gen_random_uuid(), '{TODO}', 'carbon', 'Bob1', 'entry.accessed.v1', 'after-cutover')"
        )))
        .await?;
    match w.link(MAPPING, false).await {
        Err(LinkError::Refused(reason)) => {
            assert!(reason.contains("can no longer be undone"), "{reason}");
        }
        other => panic!("expected the used-since refusal, got {other:?}"),
    }
    drop(session);
    let _ = NOTES;
    w.drop().await?;
    Ok(())
}

const UUID_MAPPING: &str = "old_uuid,new_uuid,kind
Saq1,12345678-1234-4234-8234-123456789abc,carbon
Bob1,22345678-1234-4234-8234-123456789abc,carbon
Sct1,32345678-1234-4234-8234-123456789abc,silicon
Nob1,42345678-1234-4234-8234-123456789abc,carbon
Del1,52345678-1234-4234-8234-123456789abc,carbon
";

impl World {
    async fn rekey(&self, mapping: &str, dry_run: bool) -> anyhow::Result<Value> {
        let mapping = postgres::rekey_accounts::parse_mapping(mapping)?;
        postgres::rekey_accounts::apply(&self.migrator, &mapping, dry_run).await
    }

    /// Every table, not merely the fields that the migration expects to edit.
    async fn complete_snapshot(&self) -> anyhow::Result<Value> {
        let mut session = self.superuser().await?;
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT tablename FROM pg_tables WHERE schemaname='briefcase' ORDER BY tablename",
        )
        .fetch_all(&mut session)
        .await?;
        let mut result = serde_json::Map::new();
        for name in names {
            let value: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT COALESCE(jsonb_agg(to_jsonb(row) ORDER BY to_jsonb(row)::text),'[]'::jsonb) FROM briefcase.\"{name}\" row"
            ))).fetch_one(&mut session).await?;
            result.insert(name, value);
        }
        result.insert("catalog".into(), self.value("SELECT jsonb_build_object(
            'rls',(SELECT jsonb_agg(jsonb_build_array(relname,relrowsecurity,relforcerowsecurity) ORDER BY relname) FROM pg_class WHERE relnamespace='briefcase'::regnamespace AND relkind='r'),
            'triggers',(SELECT jsonb_agg(jsonb_build_array(tgrelid::regclass::text,tgname,tgenabled) ORDER BY tgrelid::regclass::text,tgname) FROM pg_trigger WHERE NOT tgisinternal AND tgrelid IN (SELECT oid FROM pg_class WHERE relnamespace='briefcase'::regnamespace)),
            'constraints',(SELECT jsonb_agg(jsonb_build_array(conrelid::regclass::text,conname,condeferrable,condeferred) ORDER BY conrelid::regclass::text,conname) FROM pg_constraint WHERE connamespace='briefcase'::regnamespace))").await?);
        Ok(Value::Object(result))
    }

    async fn seed_uuid_references(&self) -> anyhow::Result<()> {
        self.link(MAPPING, false).await?;
        let mut session = self.superuser().await?;
        sqlx::raw_sql("UPDATE briefcase.accounts SET custodian_uuid='Saq1' WHERE account_uuid='Sct1';
            INSERT INTO briefcase.accounts(account_uuid,kind,public_id,status,deleted_at) VALUES('Del1','carbon','c:deleted','deleted',clock_timestamp());
            INSERT INTO briefcase.account_id_history(public_id,account_uuid,replaced_at) VALUES('c:before','Saq1',clock_timestamp());
            INSERT INTO briefcase.silicon_share_allowances(silicon_uuid,allowed_uuid,allowed_by_uuid) VALUES('Sct1','Bob1','Saq1');
            INSERT INTO briefcase.accounts_webhook_receipts(event_id,event_type,account_uuid,outcome) VALUES('uuid-history','account.updated','Saq1','applied');
            UPDATE briefcase.audit_events SET metadata=jsonb_build_object('account_uuid','Saq1','actor',jsonb_build_object('kind','silicon','id','Sct1'),'text','Saq1','object_key','Saq1','entry_id',entry_id);
            INSERT INTO briefcase.honeycomb_environments(environment_id,org_id,environment_revision,generation,key_version,state,operation_id,testing_key_ciphertext,testing_key_nonce)
              VALUES('01900000-0000-7000-8000-000000000999','Saq1',1,1,1,'active','01900000-0000-7000-8000-000000000998',decode('deadbeef53617131','hex'),decode('123456','hex'));
            UPDATE briefcase.outbox_events SET payload=payload || '{\"account_uuid\":\"Saq1\",\"signed_bytes\":\"unchanged\"}'::jsonb;")
            .execute(&mut session).await?;
        Ok(())
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn uuid_backfill_preserves_every_resource_and_is_atomic_and_replay_safe() -> anyhow::Result<()>
{
    let Some(w) = World::create(true).await? else {
        return Ok(());
    };
    w.seed_uuid_references().await?;
    let before = w.complete_snapshot().await?;
    let dry = w.rekey(UUID_MAPPING, true).await?;
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["referenced_accounts"], 4);
    assert_eq!(dry["unreferenced_accounts"], 1);
    assert_eq!(
        before,
        w.complete_snapshot().await?,
        "dry run changed a table or security setting"
    );
    let applied = w.rekey(UUID_MAPPING, false).await?;
    assert_eq!(applied["status"], "applied");
    let after = w.complete_snapshot().await?;
    assert_eq!(before["catalog"], after["catalog"]);
    assert_eq!(before["identity_link_runs"], after["identity_link_runs"]);
    assert_eq!(
        before["identity_link_ledger"],
        after["identity_link_ledger"]
    );
    assert_eq!(
        before["iam_identity_bindings"],
        after["iam_identity_bindings"]
    );
    for table in [
        "entry_versions",
        "organization_storage_configs",
        "multipart_uploads",
        "honeycomb_environments",
    ] {
        let strip = |snapshot: &Value| -> anyhow::Result<Vec<Value>> {
            let mut rows: Vec<_> = snapshot[table]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("snapshot array missing"))?
                .iter()
                .map(|row| {
                    let mut object = row
                        .as_object()
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("snapshot object missing"))?;
                    for key in ["org_id", "created_by_id", "owner_id"] {
                        object.remove(key);
                    }
                    Ok(Value::Object(object))
                })
                .collect::<anyhow::Result<_>>()?;
            rows.sort_by_key(Value::to_string);
            Ok(rows)
        };
        assert_eq!(
            strip(&before)?,
            strip(&after)?,
            "storage or private resource data changed in {table}"
        );
    }
    for old in before["outbox_events"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("snapshot array missing"))?
    {
        let new = after["outbox_events"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("snapshot array missing"))?
            .iter()
            .find(|row| row["event_id"] == old["event_id"])
            .ok_or_else(|| anyhow::anyhow!("historical event disappeared"))?;
        assert_eq!(old["payload"], new["payload"]);
        assert_eq!(new["account_uuid_retired"], true);
    }
    assert_eq!(
        after["silicon_share_allowances"][0]["silicon_uuid"],
        "32345678-1234-4234-8234-123456789abc"
    );
    assert_eq!(
        after["silicon_share_allowances"][0]["allowed_uuid"],
        "22345678-1234-4234-8234-123456789abc"
    );
    assert_eq!(
        after["silicon_share_allowances"][0]["allowed_by_uuid"],
        "12345678-1234-4234-8234-123456789abc"
    );
    assert!(
        after["accounts"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("snapshot array missing"))?
            .iter()
            .any(
                |row| row["account_uuid"] == "52345678-1234-4234-8234-123456789abc"
                    && row["status"] == "deleted"
            )
    );
    for row in after["audit_events"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("snapshot array missing"))?
    {
        assert_eq!(
            row["metadata"]["account_uuid"],
            "12345678-1234-4234-8234-123456789abc"
        );
        assert_eq!(
            row["metadata"]["actor"]["id"],
            "32345678-1234-4234-8234-123456789abc"
        );
        assert_eq!(row["metadata"]["text"], "Saq1");
        assert_eq!(row["metadata"]["object_key"], "Saq1");
    }
    let replay = w.rekey(UUID_MAPPING, false).await?;
    assert_eq!(replay["status"], "already_applied");
    assert_eq!(replay["changed_references"], json!({}));
    assert_eq!(
        after,
        w.complete_snapshot().await?,
        "replay changed persisted rows"
    );
    assert!(
        w.link(MAPPING, false)
            .await
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected migration refusal"))?
            .to_string()
            .contains("sealed")
    );
    let mut session = w.superuser().await?;
    for sql in [
        "INSERT INTO briefcase.accounts(account_uuid,kind) VALUES('Saq1','carbon')",
        "UPDATE briefcase.account_uuid_mappings SET kind='silicon' WHERE old_uuid='Saq1'",
        "DELETE FROM briefcase.account_uuid_mappings WHERE old_uuid='Saq1'",
        "UPDATE briefcase.outbox_events SET status='pending',delivered_at=NULL WHERE account_uuid_retired",
        "UPDATE briefcase.outbox_events SET account_uuid_retired=false WHERE account_uuid_retired",
    ] {
        assert!(
            sqlx::raw_sql(sql).execute(&mut session).await.is_err(),
            "unsafe mutation accepted: {sql}"
        );
    }
    assert_eq!(after, w.complete_snapshot().await?);
    w.drop().await
}

#[tokio::test]
async fn uuid_backfill_refuses_missing_mappings_conflicts_kinds_and_targets() -> anyhow::Result<()>
{
    let Some(w) = World::create(true).await? else {
        return Ok(());
    };
    w.seed_uuid_references().await?;
    let before = w.complete_snapshot().await?;
    for mapping in [
        UUID_MAPPING
            .lines()
            .filter(|line| !line.starts_with("Del1,"))
            .collect::<Vec<_>>()
            .join("\n"),
        UUID_MAPPING.replace(
            "Sct1,32345678-1234-4234-8234-123456789abc,silicon",
            "Sct1,32345678-1234-4234-8234-123456789abc,carbon",
        ),
    ] {
        assert!(w.rekey(&mapping, false).await.is_err());
        assert_eq!(before, w.complete_snapshot().await?);
    }
    let mut session = w.superuser().await?;
    sqlx::raw_sql("INSERT INTO briefcase.accounts(account_uuid,kind) VALUES('12345678-1234-4234-8234-123456789abc','carbon')").execute(&mut session).await?;
    let collision = w.complete_snapshot().await?;
    assert!(
        w.rekey(UUID_MAPPING, false)
            .await
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected migration refusal"))?
            .to_string()
            .contains("refusing identity merge")
    );
    assert_eq!(collision, w.complete_snapshot().await?);
    sqlx::raw_sql(
        "DELETE FROM briefcase.accounts WHERE account_uuid='12345678-1234-4234-8234-123456789abc'",
    )
    .execute(&mut session)
    .await?;
    w.rekey(UUID_MAPPING, false).await?;
    let after = w.complete_snapshot().await?;
    assert!(
        w.rekey(
            &UUID_MAPPING.replace(
                "12345678-1234-4234-8234-123456789abc",
                "92345678-1234-4234-8234-123456789abc"
            ),
            false
        )
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected migration refusal"))?
        .to_string()
        .contains("immutable")
    );
    assert_eq!(after, w.complete_snapshot().await?);
    w.drop().await
}

#[tokio::test]
async fn uuid_backfill_refuses_undrained_deliveries_without_rewriting_signed_payloads()
-> anyhow::Result<()> {
    let Some(w) = World::create(true).await? else {
        return Ok(());
    };
    w.seed_uuid_references().await?;
    let mut session = w.superuser().await?;
    sqlx::raw_sql("UPDATE briefcase.outbox_events SET status='pending',delivered_at=NULL")
        .execute(&mut session)
        .await?;
    let before = w.complete_snapshot().await?;
    assert!(
        w.rekey(UUID_MAPPING, false)
            .await
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected migration refusal"))?
            .to_string()
            .contains("work is still in flight")
    );
    assert_eq!(before, w.complete_snapshot().await?);
    w.drop().await
}
