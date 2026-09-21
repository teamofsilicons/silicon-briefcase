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
async fn lifecycle_is_durable_fenced_and_waits_for_provider_cleanup() -> anyhow::Result<()> {
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
        runtime_control,
        runtime_data,
        &SecretString::from("MDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDA="),
        "tos>briefcase",
    )?
    .with_honeycomb_token(Some(SecretString::from(token)));
    assert!(store.authenticate_honeycomb(token).is_ok());
    assert!(store.authenticate_honeycomb("wrong").is_err());
    let id = Uuid::now_v7();
    let org = format!("honeycomb-{}", Uuid::new_v4().simple());
    let mut op: HoneycombOperation = serde_json::from_value(
        json!({"operation_id":Uuid::new_v4(),"environment_id":id,"org_id":org,"app_id":"tos>briefcase","environment_revision":1,"generation":1,"key_version":1,"action":"prepare","testing_key":"0123456789abcdefghijklmnopqrstuv"}),
    )?;
    let first = store.honeycomb_operation(&op).await?;
    assert_eq!(first["state"], "completed");
    assert!(!first.to_string().contains(&op.testing_key));
    assert_eq!(store.honeycomb_operation(&op).await?, first);
    assert_eq!(
        store.honeycomb_receipt(&org, id, op.operation_id).await?,
        first
    );
    assert!(
        store
            .honeycomb_receipt("other", id, op.operation_id)
            .await
            .is_err()
    );
    let mut altered = op.clone();
    altered.reason = "changed payload".into();
    assert!(store.honeycomb_operation(&altered).await.is_err());
    let mut context: silicon_iam_client::models::ApplicationTestingContext =
        serde_json::from_value(json!({
        "environment_id":id,"environment":{"environment_id":id,"org_id":org,"name":"Honeycomb test","description":null,"version":1,"key_generation":1,"cleaned_at":null,"created_at":"2026-09-16T00:00:00Z","creator_type":"carbon","creator_id":"owner"},
        "application":{"app_id":"tos>briefcase","base_url":"https://briefcase.example.test","app_scope":{"iam":[],"external":[]},"webhook_scope":[],"testing_idle_days":30}}))?;
    let secret = SecretString::from(format!("ask_{}{}", Uuid::new_v4().simple(), "x".repeat(11)));
    let access = store
        .discover_at_revision(
            &context,
            &secret,
            store.honeycomb_discovery_revision(id).await?,
        )
        .await?;
    // Activity is sent from a durable outbox with the root key and exact generation.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(format!(
            "/api/v1/environments/{id}/apps/tos%3Ebriefcase/activity"
        )))
        .and(wiremock::matchers::header(
            "x-testing-environment-key",
            op.testing_key.as_str(),
        ))
        .and(wiremock::matchers::body_json(
            json!({"generation":1,"key_version":1}),
        ))
        .respond_with(wiremock::ResponseTemplate::new(503))
        .mount(&server)
        .await;
    store.touch(&access).await?;
    assert!(
        store
            .report_honeycomb_activity(&server.uri().parse()?)
            .await
            .is_err()
    );
    let first_activity = server
        .received_requests()
        .await
        .ok_or_else(|| anyhow::anyhow!("requests"))?;
    server.reset().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(json!({"environment_id":id,"app_id":"tos>briefcase"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    store
        .report_honeycomb_activity(&server.uri().parse()?)
        .await?;
    store
        .report_honeycomb_activity(&server.uri().parse()?)
        .await?;
    let retried = server
        .received_requests()
        .await
        .ok_or_else(|| anyhow::anyhow!("requests"))?;
    assert_eq!(
        first_activity[0].headers["idempotency-key"],
        retried[0].headers["idempotency-key"]
    );
    // An outstanding descriptor for another projected organization must also block completion.
    let test_org = format!("{id}:another-org");
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
    let cleanup_id = Uuid::new_v4();
    sqlx::query("INSERT INTO briefcase.object_cleanup_jobs(org_id,cleanup_id,cleanup_kind,source_entry_id,source_version_id,storage_backend,bucket_name,storage_region,storage_prefix,storage_encryption_mode,object_key) VALUES($1,$2,'version_delete',$3,$4,'platform','test-bucket','us-east-1','testing','sse_s3','object')")
        .bind(&test_org).bind(cleanup_id).bind(Uuid::new_v4()).bind(Uuid::new_v4()).execute(&mut *setup).await?;
    setup.commit().await?;
    op.operation_id = Uuid::new_v4();
    op.action = "clean".into();
    op.environment_revision = 2;
    op.generation = 2;
    assert_eq!(store.honeycomb_operation(&op).await?["state"], "pending");
    assert!(store.resolve_root_key(&secret).await.is_err());
    assert!(store.acquire_use_fence(&access).await.is_err());
    assert!(
        store
            .discover_at_revision(
                &context,
                &secret,
                store.honeycomb_discovery_revision(id).await?
            )
            .await
            .is_err()
    );
    assert_eq!(store.honeycomb_operation(&op).await?["state"], "pending");
    // Simulate the provider worker's confirmed deletion acknowledgement.
    sqlx::query("DELETE FROM briefcase.object_cleanup_jobs WHERE org_id=$1 AND cleanup_id=$2")
        .bind(&test_org)
        .bind(cleanup_id)
        .execute(&data)
        .await?;
    assert_eq!(store.honeycomb_operation(&op).await?["state"], "completed");
    assert!(
        store
            .discover_at_revision(
                &context,
                &secret,
                store.honeycomb_discovery_revision(id).await?
            )
            .await
            .is_err(),
        "old IAM generation cannot repopulate cleaned data"
    );
    let meta = context
        .environment
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("metadata"))?;
    meta.version = 2;
    meta.cleaned_at = Some(time::OffsetDateTime::now_utc());
    store
        .discover_at_revision(
            &context,
            &secret,
            store.honeycomb_discovery_revision(id).await?,
        )
        .await?;
    // A response started in generation two cannot cross a second clean, even
    // if no application discovery happened between the two clean operations.
    let before_second_clean = store.honeycomb_discovery_revision(id).await?;
    let first_clean_context = context.clone();
    let mut next_clean = op.clone();
    next_clean.operation_id = Uuid::new_v4();
    next_clean.environment_revision += 1;
    next_clean.generation += 1;
    store.honeycomb_operation(&next_clean).await?;
    assert!(
        store
            .discover_at_revision(&first_clean_context, &secret, before_second_clean)
            .await
            .is_err()
    );
    context
        .environment
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("metadata"))?
        .cleaned_at = Some(time::OffsetDateTime::now_utc());
    store
        .discover_at_revision(
            &context,
            &secret,
            store.honeycomb_discovery_revision(id).await?,
        )
        .await?;
    op = next_clean;
    let old = op.clone();
    op.operation_id = Uuid::new_v4();
    op.action = "disable".into();
    op.environment_revision = 4;
    assert_eq!(store.honeycomb_operation(&op).await?["state"], "completed");
    assert!(
        store
            .discover_at_revision(
                &context,
                &secret,
                store.honeycomb_discovery_revision(id).await?
            )
            .await
            .is_err()
    );
    // Old completed receipts may be read, but must not reactivate the environment.
    assert_eq!(store.honeycomb_operation(&old).await?["state"], "completed");
    assert!(store.resolve_root_key(&secret).await.is_err());
    op.operation_id = Uuid::new_v4();
    op.action = "restore".into();
    op.environment_revision = 5;
    store.honeycomb_operation(&op).await?;
    store
        .discover_at_revision(
            &context,
            &secret,
            store.honeycomb_discovery_revision(id).await?,
        )
        .await?;
    op.operation_id = Uuid::new_v4();
    op.action = "rotate-key".into();
    op.environment_revision = 6;
    op.key_version = 2;
    op.testing_key = "1234567890abcdefghijklmnopqrstuv".into();
    store.honeycomb_operation(&op).await?;
    assert!(
        store
            .discover_at_revision(
                &context,
                &secret,
                store.honeycomb_discovery_revision(id).await?
            )
            .await
            .is_err()
    );
    context
        .environment
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("metadata"))?
        .key_generation = 2;
    store
        .discover_at_revision(
            &context,
            &secret,
            store.honeycomb_discovery_revision(id).await?,
        )
        .await?;
    op.operation_id = Uuid::new_v4();
    op.action = "purge".into();
    op.environment_revision = 7;
    assert_eq!(store.honeycomb_operation(&op).await?["state"], "completed");
    assert!(
        store
            .discover_at_revision(
                &context,
                &secret,
                store.honeycomb_discovery_revision(id).await?
            )
            .await
            .is_err()
    );
    op.operation_id = Uuid::new_v4();
    op.action = "restore".into();
    op.environment_revision = 8;
    assert!(store.honeycomb_operation(&op).await.is_err());
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM briefcase.organizations WHERE testing_environment_id=$1",
    )
    .bind(id)
    .fetch_one(&data)
    .await?;
    assert_eq!(retained, 0);
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
