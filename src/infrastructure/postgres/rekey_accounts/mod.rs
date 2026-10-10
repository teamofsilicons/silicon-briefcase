//! Apply the immutable Accounts export without changing content/resource keys.
use std::collections::HashSet;

use anyhow::{Context as _, ensure};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use sqlx::{Connection as _, PgPool};
use uuid::Uuid;

use crate::domain::account::AccountUuid;

const PROGRAM: &str = concat!(
    include_str!("../link_identities/00_setup.sql"),
    include_str!("rekey.sql"),
);

#[derive(Debug, Serialize)]
struct Row {
    old_uuid: String,
    new_uuid: String,
    kind: String,
}

/// Validated, one-to-one Accounts export.
#[derive(Debug)]
pub struct Mapping {
    rows: Vec<Row>,
    digest: String,
}

/// Read the exact `old_uuid,new_uuid,kind` CSV contract.
///
/// # Errors
/// Refuses ambiguous, repeated, noncanonical and overlapping identifiers.
pub fn parse_mapping(text: &str) -> anyhow::Result<Mapping> {
    let mut lines = text.lines();
    ensure!(
        lines.next() == Some("old_uuid,new_uuid,kind"),
        "expected exact CSV header old_uuid,new_uuid,kind"
    );
    let mut rows = Vec::new();
    let (mut old_ids, mut new_ids) = (HashSet::new(), HashSet::new());
    for (index, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let values: Vec<_> = line.split(',').collect();
        ensure!(
            values.len() == 3,
            "line {} needs exactly three unquoted fields",
            index + 2
        );
        let (old, new, kind) = (values[0], values[1], values[2]);
        AccountUuid::new(old)
            .with_context(|| format!("line {} has an invalid old UUID", index + 2))?;
        let uuid = Uuid::parse_str(new).context("new_uuid must be a canonical lowercase UUIDv4")?;
        ensure!(
            uuid.get_version_num() == 4
                && uuid.get_variant() == uuid::Variant::RFC4122
                && uuid.to_string() == new,
            "new_uuid must be a canonical lowercase UUIDv4"
        );
        ensure!(
            matches!(kind, "carbon" | "silicon"),
            "kind must be carbon or silicon"
        );
        ensure!(old != new, "old and new UUIDs must differ");
        ensure!(
            old_ids.insert(old.to_owned()),
            "old UUID {old} occurs twice"
        );
        ensure!(
            new_ids.insert(new.to_owned()),
            "new UUID {new} occurs twice; identities cannot merge"
        );
        rows.push(Row {
            old_uuid: old.to_owned(),
            new_uuid: new.to_owned(),
            kind: kind.to_owned(),
        });
    }
    ensure!(!rows.is_empty(), "mapping has no rows");
    ensure!(
        old_ids.is_disjoint(&new_ids),
        "old and new UUID sets overlap; chains and swaps are not supported"
    );
    Ok(Mapping {
        rows,
        digest: hex::encode(Sha256::digest(text.as_bytes())),
    })
}

/// Rekey every live account reference in one maintenance transaction.
///
/// # Errors
/// Refuses conflicting exports, target collisions, active work and unknown
/// identity columns. Dry runs validate constraints, then roll back.
pub async fn apply(pool: &PgPool, mapping: &Mapping, dry_run: bool) -> anyhow::Result<Value> {
    let mut connection = pool.acquire().await?.detach();
    let outcome = run(&mut connection, mapping, dry_run).await;
    connection.close().await.ok();
    outcome
}

async fn run(
    connection: &mut sqlx::PgConnection,
    mapping: &Mapping,
    dry_run: bool,
) -> anyhow::Result<Value> {
    let mut tx = connection.begin().await?;
    sqlx::raw_sql("CREATE TEMP TABLE link_settings (run_id uuid,mapping_sha256 text,mapping jsonb) ON COMMIT DROP;
        CREATE TEMP TABLE uuid_input (old_uuid text PRIMARY KEY, new_uuid text UNIQUE NOT NULL, kind text NOT NULL) ON COMMIT DROP;
        CREATE TEMP TABLE uuid_settings (digest text NOT NULL) ON COMMIT DROP;
        CREATE TEMP TABLE uuid_result (report jsonb NOT NULL) ON COMMIT DROP;")
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO uuid_input SELECT * FROM jsonb_to_recordset($1) AS row(old_uuid text,new_uuid text,kind text)")
        .bind(serde_json::to_value(&mapping.rows)?).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO uuid_settings VALUES ($1)")
        .bind(&mapping.digest)
        .execute(&mut *tx)
        .await?;
    sqlx::raw_sql(PROGRAM).execute(&mut *tx).await?;
    let mut report: Value = sqlx::query_scalar("SELECT report FROM uuid_result")
        .fetch_one(&mut *tx)
        .await?;
    if dry_run {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    report["dry_run"] = dry_run.into();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::parse_mapping;
    #[test]
    fn requires_an_unambiguous_canonical_v4_export() {
        let valid = "old_uuid,new_uuid,kind\nAda,12345678-1234-4234-8234-123456789abc,carbon\n";
        assert!(parse_mapping(valid).is_ok());
        for bad in [
            valid.replace("4234", "1234"),
            valid.replace("9abc", "9ABC"),
            valid.replace("carbon", "person"),
            format!("{valid}Bob,12345678-1234-4234-8234-123456789abc,silicon\n"),
            format!("{valid}Ada,22345678-1234-4234-8234-123456789abc,carbon\n"),
            valid.replace("old_uuid,new_uuid", "new_uuid,old_uuid"),
        ] {
            assert!(parse_mapping(&bad).is_err(), "accepted {bad}");
        }
    }
}
