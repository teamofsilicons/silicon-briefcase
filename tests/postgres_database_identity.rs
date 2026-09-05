//! Live PostgreSQL checks for production/testing database identity fencing.
//!
//! ```bash
//! docker compose up -d postgres postgres-test
//! BRIEFCASE_TEST_CONTROL_DATABASE_URL=postgres://briefcase:briefcase-local-only@127.0.0.1:5433/briefcase \
//! BRIEFCASE_TEST_DATA_DATABASE_URL=postgres://briefcase:briefcase-local-only@127.0.0.1:5434/briefcase_test \
//!   cargo test --test postgres_database_identity
//! ```

use std::{num::NonZeroU32, time::Duration};

use secrecy::SecretString;
use silicon_briefcase::{config::DatabaseSettings, infrastructure::postgres};

fn settings(url: String) -> DatabaseSettings {
    DatabaseSettings {
        url: SecretString::from(url),
        max_connections: NonZeroU32::new(2).unwrap_or(NonZeroU32::MIN),
        min_connections: 0,
        acquire_timeout: Duration::from_secs(10),
        statement_timeout: Duration::from_secs(30),
    }
}

fn equivalent_dsn(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("postgres://") {
        format!("postgresql://{rest}")
    } else if let Some(rest) = url.strip_prefix("postgresql://") {
        format!("postgres://{rest}")
    } else {
        url.to_owned()
    }
}

#[tokio::test]
async fn actual_database_identity_rejects_an_equivalent_dsn() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("BRIEFCASE_TEST_CONTROL_DATABASE_URL") else {
        eprintln!("skipping: BRIEFCASE_TEST_CONTROL_DATABASE_URL is not set");
        return Ok(());
    };

    let production = postgres::connect(&settings(url.clone()), "database-identity-primary").await?;
    postgres::migrate(&production).await?;
    let alias =
        postgres::connect(&settings(equivalent_dsn(&url)), "database-identity-alias").await?;

    let Err(error) = postgres::verify_distinct_databases(&production, &alias).await else {
        anyhow::bail!("two DSNs for one database were accepted");
    };
    assert!(
        error
            .to_string()
            .contains("must resolve to different PostgreSQL databases")
    );

    if let Ok(test_url) = std::env::var("BRIEFCASE_TEST_DATA_DATABASE_URL") {
        let testing = postgres::connect(&settings(test_url), "database-identity-testing").await?;
        postgres::migrate(&testing).await?;
        postgres::verify_distinct_databases(&production, &testing).await?;
        testing.close().await;
    }

    alias.close().await;
    production.close().await;
    Ok(())
}
