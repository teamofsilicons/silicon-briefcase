//! Real-database participant receipts, cleanup barriers and lifecycle replay fences.
use secrecy::SecretString;
use serde_json::json;
use silicon_briefcase::infrastructure::{
    postgres,
    testing::{TestingEnvironmentStore, honeycomb::HoneycombOperation},
};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one ordered lifecycle journey verifies the same durable environment through purge"
)]
async fn retirement_selects_briefcase_waits_for_cleanup_and_allows_reimport() -> anyhow::Result<()>
{
    let (Ok(control), Ok(data)) = (
        std::env::var("BRIEFCASE_TEST_CONTROL_DATABASE_URL"),
        std::env::var("BRIEFCASE_TEST_DATA_DATABASE_URL"),
    ) else {
        eprintln!("skipping: explicit disposable database URLs required");
        return Ok(());
    };
    anyhow::ensure!(control != data, "test databases must differ");
    let control = PgPoolOptions::new()
        .after_connect(|c, _| {
            Box::pin(async move {
                sqlx::query("SET search_path=public").execute(c).await?;
                Ok(())
            })
        })
        .connect(&control)
        .await?;
    let data = PgPoolOptions::new()
        .after_connect(|c, _| {
            Box::pin(async move {
                sqlx::query("SET search_path=public").execute(c).await?;
                Ok(())
            })
        })
        .connect(&data)
        .await?;
    postgres::migrate(&control).await?;
    postgres::migrate(&data).await?;
    let runtime_control = PgPoolOptions::new()
        .after_connect(|c, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE briefcase_api")
                    .execute(&mut *c)
                    .await?;
                sqlx::query("SET search_path=public").execute(c).await?;
                Ok(())
            })
        })
        .connect(&std::env::var("BRIEFCASE_TEST_CONTROL_DATABASE_URL")?)
        .await?;
    let runtime_data = PgPoolOptions::new()
        .after_connect(|c, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE briefcase_api")
                    .execute(&mut *c)
                    .await?;
                sqlx::query("SET search_path=public").execute(c).await?;
                Ok(())
            })
        })
        .connect(&std::env::var("BRIEFCASE_TEST_DATA_DATABASE_URL")?)
        .await?;
    let token = "dedicated-honeycomb-integration-token-12345";
    let store = TestingEnvironmentStore::new(
        runtime_control.clone(),
        runtime_data.clone(),
        &SecretString::from("MDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDA="),
        "storage",
    )?
    .with_honeycomb_token(Some(SecretString::from(token)));
    assert!(store.authenticate_honeycomb(token).is_ok());
    assert!(store.authenticate_honeycomb("wrong").is_err());
    let id = Uuid::now_v7();
    let org = format!("honeycomb-{}", Uuid::new_v4().simple());
    let mut op: HoneycombOperation = serde_json::from_value(
        json!({"operation_id":Uuid::new_v4(),"environment_id":id,"org_id":org,"app_id":"storage","environment_revision":1,"generation":1,"key_version":1,"action":"prepare","testing_key":"0123456789abcdefghijklmnopqrstuv"}),
    )?;
    let mut wrong_app = op.clone();
    wrong_app.app_id = "briefcase".into();
    assert!(store.honeycomb_operation(&wrong_app).await.is_err());
    let persisted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM briefcase.honeycomb_environments WHERE environment_id=$1",
    )
    .bind(id)
    .fetch_one(&control)
    .await?;
    assert_eq!(
        persisted, 0,
        "wrong participant must not create any lifecycle state"
    );
    assert_eq!(store.honeycomb_operation(&op).await?["state"], "completed");
    let reconfigured = TestingEnvironmentStore::new(
        runtime_control,
        runtime_data,
        &SecretString::from("MDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDA="),
        "archive",
    )?;
    assert!(
        reconfigured
            .honeycomb_receipt(&org, id, op.operation_id)
            .await
            .is_err()
    );
    assert!(
        reconfigured.honeycomb_operation(&op).await.is_err(),
        "old identity replay must fail after reconfiguration"
    );
    let mut takeover = op.clone();
    takeover.app_id = "archive".into();
    assert!(
        reconfigured.honeycomb_operation(&takeover).await.is_err(),
        "changing an existing replay's identity must fail"
    );
    takeover.operation_id = Uuid::new_v4();
    takeover.environment_revision = 2;
    assert!(
        reconfigured.honeycomb_operation(&takeover).await.is_err(),
        "new identity cannot take over an existing participant environment"
    );
    let revision: i64 = sqlx::query_scalar(
        "SELECT environment_revision FROM briefcase.honeycomb_environments WHERE environment_id=$1",
    )
    .bind(id)
    .fetch_one(&control)
    .await?;
    assert_eq!(revision, 1);
    // One unavailable participant report must not starve another active environment.
    let mut second = op.clone();
    second.environment_id = Uuid::new_v4();
    second.operation_id = Uuid::new_v4();
    store.honeycomb_operation(&second).await?;
    sqlx::query("UPDATE briefcase.honeycomb_environments SET last_activity_at=clock_timestamp() WHERE environment_id IN ($1,$2)")
        .bind(id).bind(second.environment_id).execute(&control).await?;
    let server = wiremock::MockServer::start().await;
    reconfigured
        .report_honeycomb_activity(&server.uri().parse()?)
        .await?;
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "reconfiguration must not report another participant's activity"
    );
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(format!(
            "/api/v1/environments/{id}/apps/storage/activity"
        )))
        .respond_with(wiremock::ResponseTemplate::new(503))
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(format!(
            "/api/v1/environments/{}/apps/storage/activity",
            second.environment_id
        )))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        store
            .report_honeycomb_activity(&server.uri().parse()?)
            .await
            .is_err()
    );
    let reported: bool = sqlx::query_scalar("SELECT activity_reported_at IS NOT NULL FROM briefcase.honeycomb_environments WHERE environment_id=$1")
        .bind(second.environment_id).fetch_one(&control).await?;
    assert!(
        reported,
        "a failed report cannot starve another environment"
    );
    let reported: bool = sqlx::query_scalar("SELECT activity_reported_at IS NOT NULL FROM briefcase.honeycomb_environments WHERE environment_id=$1")
        .bind(id).fetch_one(&control).await?;
    assert!(!reported, "failed report stays durable");
    sqlx::query("DELETE FROM briefcase.honeycomb_operations WHERE environment_id=$1")
        .bind(second.environment_id)
        .execute(&control)
        .await?;
    sqlx::query("DELETE FROM briefcase.honeycomb_environments WHERE environment_id=$1")
        .bind(second.environment_id)
        .execute(&control)
        .await?;
    let test_org = format!("{id}:retirement");
    let mut setup = data.begin().await?;
    sqlx::query("SELECT set_config('briefcase.testing_environment_id',$1,true)")
        .bind(id.to_string())
        .execute(&mut *setup)
        .await?;
    sqlx::query("INSERT INTO briefcase.organizations(org_id,testing_environment_id) VALUES($1,$2)")
        .bind(&test_org)
        .bind(id)
        .execute(&mut *setup)
        .await?;
    let cleanup = Uuid::new_v4();
    sqlx::query("INSERT INTO briefcase.object_cleanup_jobs(org_id,cleanup_id,cleanup_kind,source_entry_id,source_version_id,storage_backend,bucket_name,storage_region,storage_prefix,storage_encryption_mode,object_key) VALUES($1,$2,'version_delete',$3,$4,'platform','test-bucket','us-east-1','testing','sse_s3','object')")
        .bind(&test_org).bind(cleanup).bind(Uuid::new_v4()).bind(Uuid::new_v4()).execute(&mut *setup).await?;
    setup.commit().await?;
    op.operation_id = Uuid::new_v4();
    op.action = "retire-applications".into();
    op.retired_apps = vec!["other".into()];
    op.environment_revision = 2;
    let receipt = store.honeycomb_operation(&op).await?;
    assert_eq!(receipt["state"], "completed");
    assert_eq!(receipt["retired_apps"], json!(["other"]));
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM briefcase.organizations WHERE testing_environment_id=$1",
    )
    .bind(id)
    .fetch_one(&data)
    .await?;
    assert_eq!(
        remaining, 1,
        "another application's retirement preserves Briefcase data"
    );
    op.operation_id = Uuid::new_v4();
    op.retired_apps = vec!["storage".into()];
    op.environment_revision = 3;
    // A fault scoped to this environment proves failed receipts recover on retry.
    // Dynamic identifiers and values below contain only a generated UUID and fixed text.
    let fault = format!("honeycomb_cleanup_fault_{}", id.simple());
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE FUNCTION briefcase.{fault}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF current_setting('briefcase.testing_environment_id',true)='{id}' THEN RAISE EXCEPTION 'injected cleanup failure'; END IF; RETURN NULL; END $$")))
        .execute(&data).await?;
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE TRIGGER {fault} BEFORE DELETE ON briefcase.organization_member_tags FOR EACH STATEMENT EXECUTE FUNCTION briefcase.{fault}()")))
        .execute(&data).await?;
    let failed = store.honeycomb_operation(&op).await;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP TRIGGER {fault} ON briefcase.organization_member_tags"
    )))
    .execute(&data)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP FUNCTION briefcase.{fault}()"
    )))
    .execute(&data)
    .await?;
    assert!(failed.is_err());
    assert_eq!(
        store.honeycomb_receipt(&org, id, op.operation_id).await?["state"],
        "failed"
    );
    assert_eq!(store.honeycomb_operation(&op).await?["state"], "pending");
    assert_eq!(
        store.honeycomb_receipt(&org, id, op.operation_id).await?["state"],
        "pending"
    );
    assert_eq!(store.honeycomb_operation(&op).await?["state"], "pending");
    sqlx::query("DELETE FROM briefcase.object_cleanup_jobs WHERE org_id=$1 AND cleanup_id=$2")
        .bind(&test_org)
        .bind(cleanup)
        .execute(&data)
        .await?;
    let receipt = store.honeycomb_operation(&op).await?;
    assert_eq!(receipt["state"], "completed");
    assert_eq!(receipt["retired_apps"], json!(["storage"]));
    let state: String = sqlx::query_scalar(
        "SELECT state FROM briefcase.honeycomb_environments WHERE environment_id=$1",
    )
    .bind(id)
    .fetch_one(&control)
    .await?;
    assert_eq!(state, "retired");
    let old_activity_cleared: bool = sqlx::query_scalar("SELECT last_activity_at IS NULL AND activity_reported_at IS NULL FROM briefcase.honeycomb_environments WHERE environment_id=$1")
        .bind(id).fetch_one(&control).await?;
    assert!(
        old_activity_cleared,
        "retirement cannot replay pre-retirement use"
    );
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM briefcase.organizations WHERE testing_environment_id=$1",
    )
    .bind(id)
    .fetch_one(&data)
    .await?;
    assert_eq!(remaining, 0);
    let retirement = op.clone();
    op.operation_id = Uuid::new_v4();
    op.action = "restore".into();
    op.environment_revision = 4;
    op.retired_apps.clear();
    assert!(
        store.honeycomb_operation(&op).await.is_err(),
        "restore cannot revive retired application data"
    );
    op.operation_id = Uuid::new_v4();
    op.action = "import".into();
    // The shared world may be cleaned and its root rotated while Briefcase is absent.
    op.generation = 2;
    op.key_version = 2;
    op.testing_key = "1234567890abcdefghijklmnopqrstuv".into();
    assert_eq!(store.honeycomb_operation(&op).await?["state"], "completed");
    assert_eq!(
        store.honeycomb_operation(&retirement).await?["state"],
        "completed"
    );
    let state: String = sqlx::query_scalar(
        "SELECT state FROM briefcase.honeycomb_environments WHERE environment_id=$1",
    )
    .bind(id)
    .fetch_one(&control)
    .await?;
    assert_eq!(
        state, "active",
        "old retirement replay cannot retire the new import"
    );
    sqlx::query("DELETE FROM briefcase.honeycomb_operations WHERE environment_id=$1")
        .bind(id)
        .execute(&control)
        .await?;
    sqlx::query("DELETE FROM briefcase.honeycomb_environments WHERE environment_id=$1")
        .bind(id)
        .execute(&control)
        .await?;
    Ok(())
}
