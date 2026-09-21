//! Run only against a fresh disposable database selected explicitly for this upgrade test.

use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[tokio::test]
async fn canonical_upgrade_preserves_keys_and_fences_test_worlds() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("BRIEFCASE_CANONICAL_TEST_DATABASE_URL") else {
        return Ok(());
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname='briefcase')")
            .fetch_one(&pool)
            .await?;
    anyhow::ensure!(
        !exists,
        "canonical upgrade test requires a fresh disposable database"
    );
    sqlx::raw_sql("CREATE ROLE briefcase_migrator NOLOGIN NOSUPERUSER NOBYPASSRLS; DO $$ BEGIN EXECUTE format('GRANT CREATE ON DATABASE %I TO briefcase_migrator',current_database()); END $$; CREATE ROLE briefcase_api NOLOGIN NOSUPERUSER NOBYPASSRLS; SET ROLE briefcase_migrator")
        .execute(&pool)
        .await?;
    for migration in MIGRATOR.iter().filter(|migration| migration.version < 45) {
        sqlx::raw_sql(migration.sql.clone()).execute(&pool).await?;
    }
    sqlx::raw_sql("RESET ROLE").execute(&pool).await?;
    let old_actor = Uuid::now_v7();
    let old_member = Uuid::now_v7();
    let world = Uuid::now_v7();
    let world_actor = Uuid::now_v7();
    let world_member = Uuid::now_v7();
    seed(&pool, None, old_actor, old_member).await?;
    seed(&pool, Some(world), world_actor, world_member).await?;
    sqlx::raw_sql("SET ROLE briefcase_migrator")
        .execute(&pool)
        .await?;
    sqlx::raw_sql(include_str!(
        "../migrations/0045_canonical_iam_identity_bindings.sql"
    ))
    .execute(&pool)
    .await?;
    sqlx::raw_sql("RESET ROLE; RESET briefcase.testing_environment_id; SET ROLE briefcase_api")
        .execute(&pool)
        .await?;
    assert_eq!(resolve(&pool, None, "carbon", "person").await?, old_actor);
    assert_eq!(
        resolve(&pool, None, "membership", "person[alpha]").await?,
        old_member
    );
    assert!(resolve(&pool, None, "carbon", "newperson").await.is_err());
    // A caller cannot select a foreign environment through the function argument.
    assert!(
        resolve(&pool, Some(world), "carbon", "person")
            .await
            .is_err()
    );
    sqlx::query("SELECT set_config('briefcase.testing_environment_id',$1,false)")
        .bind(world.to_string())
        .execute(&pool)
        .await?;
    assert_eq!(
        resolve(&pool, Some(world), "carbon", "person").await?,
        world_actor
    );
    assert_eq!(
        resolve(&pool, Some(world), "membership", "person[alpha]").await?,
        world_member
    );
    assert!(resolve(&pool, None, "carbon", "person").await.is_err());
    sqlx::raw_sql("RESET ROLE; UPDATE briefcase.iam_identity_backfill SET verified=true; RESET briefcase.testing_environment_id; SET ROLE briefcase_api").execute(&pool).await?;
    let allocated = resolve(&pool, None, "carbon", "newperson").await?;
    assert_ne!(allocated, old_actor);
    assert_eq!(
        resolve(&pool, None, "carbon", "newperson").await?,
        allocated
    );
    // Runtime may resolve validated IDs but cannot rewrite the mapping or bypass the fence.
    assert!(
        sqlx::query("DELETE FROM briefcase.iam_identity_bindings")
            .execute(&pool)
            .await
            .is_err()
    );
    pool.close().await;
    Ok(())
}

async fn seed(pool: &PgPool, world: Option<Uuid>, actor: Uuid, member: Uuid) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('briefcase.testing_environment_id',$1,true)")
        .bind(world.map_or_else(String::new, |id| id.to_string()))
        .execute(&mut *tx)
        .await?;
    let org = world.map_or_else(|| "alpha".to_owned(), |id| format!("{id}:alpha"));
    sqlx::query("INSERT INTO briefcase.organizations(org_id) VALUES($1)")
        .bind(&org)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO briefcase.organization_members(org_id,actor_type,actor_id,principal_id,membership_id,authorization_epoch) VALUES($1,'carbon','person',$2,$3,1)")
        .bind(org).bind(actor).bind(member).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn resolve(
    pool: &PgPool,
    world: Option<Uuid>,
    kind: &str,
    id: &str,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar("SELECT briefcase.resolve_iam_identity_key($1,$2,$3,$4)")
        .bind(world.unwrap_or(Uuid::nil()))
        .bind(kind)
        .bind(id)
        .bind(Uuid::now_v7())
        .fetch_one(pool)
        .await
}
