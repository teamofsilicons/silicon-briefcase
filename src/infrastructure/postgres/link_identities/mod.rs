//! The cutover from organizations to personal drives:
//! `briefcase-migrate link-identities --file mapping.csv [--dry-run]`.
//!
//! Pre-migration rows name Carbons and Silicons by the public id IAM gave them
//! (`c:saket`) and keep their files in organization tenants. This command
//! links each such identity to its Silicon Accounts account, re-keys every row
//! that names it to the account's uuid, and moves each organization's files
//! into personal drives: members' private files to their own drives, shared
//! spaces to the owner's drive with snapshot shares for exactly the members
//! who could reach them, and the files of members without an account to an
//! `unclaimed/` area in the owner's drive. Addresses from before keep
//! resolving through `legacy_path_aliases`.
//!
//! Everything happens in one transaction. A dry run rolls back and only
//! reports. Running again with the same mapping changes nothing; running
//! with a corrected mapping first undoes the run in place (from its ledger)
//! — until Briefcase has been used in the drives it filled. The mapping file:
//!
//! ```text
//! iam_principal_id,accounts_uuid,kind,accounts_id
//! c:saket,zQo,carbon,c:saket
//! 0191a0b2-6c1e-7cc4-9f3e-2b9a6f1d4e10,Pb7,,si:scout
//! org:tos,zQo
//! ```
//!
//! The first column is a legacy public id, an IAM principal UUID, or
//! `org:{org}` (the account whose drive receives that organization's shared
//! spaces, when it has no single active owner). `kind` and `accounts_id` are
//! optional checks and cache seeds.

use std::fmt;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use sqlx::{Connection as _, PgPool};
use uuid::Uuid;

use crate::domain::account::{AccountUuid, PublicId};

/// The program, run as one script in the command's transaction.
const PROGRAM: &str = concat!(
    include_str!("00_setup.sql"),
    include_str!("10_resolve.sql"),
    include_str!("20_revert.sql"),
    include_str!("30_move.sql"),
    include_str!("31_move_subtree.sql"),
    include_str!("40_plan.sql"),
    include_str!("50_after.sql"),
    include_str!("51_after.sql"),
    include_str!("60_finish.sql"),
    include_str!("99_main.sql"),
);

const REQUIRED_COLUMNS: [&str; 2] = ["iam_principal_id", "accounts_uuid"];
const OPTIONAL_COLUMNS: [&str; 2] = ["kind", "accounts_id"];

/// One parsed line of the mapping file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MappingRow {
    /// Line number in the file (1-based), for messages.
    pub line: usize,
    /// A legacy public id (`c:…`/`si:…`), an IAM principal UUID, or `org:{org}`.
    pub key: String,
    /// The Silicon Accounts uuid the key becomes (or that receives `org:`).
    pub accounts_uuid: String,
    /// `carbon` or `silicon`, when the line states it.
    pub kind: Option<String>,
    /// The account's current public id, when the line states it.
    pub accounts_id: Option<String>,
}

/// A validated mapping file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mapping {
    rows: Vec<MappingRow>,
    sha256: String,
}

impl Mapping {
    /// The parsed rows.
    #[must_use]
    pub fn rows(&self) -> &[MappingRow] {
        &self.rows
    }

    /// SHA-256 of the file's bytes (hex): the same file is the same mapping.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// A mapping file that cannot be used, with the line and the reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MappingError {
    /// The line at fault (0 for the file as a whole).
    pub line: usize,
    /// What is wrong and what to write instead.
    pub reason: String,
}

impl fmt::Display for MappingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(formatter, "mapping file: {}", self.reason)
        } else {
            write!(
                formatter,
                "mapping file line {}: {}",
                self.line, self.reason
            )
        }
    }
}

impl std::error::Error for MappingError {}

fn refuse(line: usize, reason: impl Into<String>) -> MappingError {
    MappingError {
        line,
        reason: reason.into(),
    }
}

/// Parses and checks a mapping file.
///
/// Blank lines and lines starting with `#` are ignored. The first other line
/// is the header: `iam_principal_id,accounts_uuid` in that order, then
/// optionally `kind` and `accounts_id`. Fields are separated by commas and
/// never quoted (no value can contain a comma).
///
/// # Errors
///
/// Returns the first line that is not valid, with the exact reason.
pub fn parse_mapping(text: &str) -> Result<Mapping, MappingError> {
    let sha256 = hex::encode(Sha256::digest(text.as_bytes()));
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'));
    let Some((header_line, header)) = lines.next() else {
        return Err(refuse(
            0,
            "the file is empty; it needs the header iam_principal_id,accounts_uuid and one line per identity",
        ));
    };
    let columns: Vec<&str> = header.split(',').map(str::trim).collect();
    if columns.len() < 2 || columns[..2] != REQUIRED_COLUMNS {
        return Err(refuse(
            header_line,
            format!("the header must start with iam_principal_id,accounts_uuid, not `{header}`"),
        ));
    }
    for column in &columns[2..] {
        if !OPTIONAL_COLUMNS.contains(column) {
            return Err(refuse(
                header_line,
                format!("unknown column `{column}`; the optional columns are kind and accounts_id"),
            ));
        }
    }
    let position = |name: &str| columns.iter().position(|column| *column == name);
    let (kind_at, id_at) = (position("kind"), position("accounts_id"));
    let mut rows = Vec::new();
    for (line, content) in lines {
        if content.contains('"') {
            return Err(refuse(line, "values are never quoted; remove the quotes"));
        }
        let fields: Vec<&str> = content.split(',').map(str::trim).collect();
        if fields.len() != columns.len() {
            return Err(refuse(
                line,
                format!("expected {} fields, found {}", columns.len(), fields.len()),
            ));
        }
        let key = fields[0];
        validate_key(line, key)?;
        let accounts_uuid = fields[1];
        AccountUuid::new(accounts_uuid).map_err(|error| {
            refuse(
                line,
                format!("`{accounts_uuid}` is not a Silicon Accounts uuid: {error}"),
            )
        })?;
        let kind = kind_at
            .map(|at| fields[at])
            .filter(|value| !value.is_empty())
            .map(|value| match value {
                "carbon" | "silicon" => Ok(value.to_owned()),
                other => Err(refuse(
                    line,
                    format!("kind must be carbon or silicon, not `{other}`"),
                )),
            })
            .transpose()?;
        let accounts_id = id_at
            .map(|at| fields[at])
            .filter(|value| !value.is_empty())
            .map(|value| {
                PublicId::parse(value)
                    .map(|id| id.as_str().to_owned())
                    .map_err(|error| {
                        refuse(line, format!("`{value}` is not an account id: {error}"))
                    })
            })
            .transpose()?;
        if key.starts_with("org:") && (kind.is_some() || accounts_id.is_some()) {
            return Err(refuse(
                line,
                "an org: line names only the receiving account's uuid",
            ));
        }
        rows.push(MappingRow {
            line,
            key: key.to_owned(),
            accounts_uuid: accounts_uuid.to_owned(),
            kind,
            accounts_id,
        });
    }
    if rows.is_empty() {
        return Err(refuse(0, "the file has a header but no identities"));
    }
    Ok(Mapping { rows, sha256 })
}

fn validate_key(line: usize, key: &str) -> Result<(), MappingError> {
    if let Some(organization) = key.strip_prefix("org:") {
        let valid = (3..=50).contains(&organization.len())
            && organization.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            });
        return if valid {
            Ok(())
        } else {
            Err(refuse(
                line,
                format!("`{key}` does not name an organization (org: and its 3–50 character key)"),
            ))
        };
    }
    let lowered = key.to_ascii_lowercase();
    if lowered.starts_with("c:") || lowered.starts_with("si:") {
        let (_, handle) = key.split_once(':').unwrap_or_default();
        let valid = (key.starts_with("c:") || key.starts_with("si:"))
            && (1..=64).contains(&handle.len())
            && handle.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            });
        return if valid {
            Ok(())
        } else {
            Err(refuse(
                line,
                format!("`{key}` is not a legacy public id such as c:saket or si:scout"),
            ))
        };
    }
    Uuid::parse_str(key).map(drop).map_err(|_| {
        refuse(
            line,
            format!("`{key}` is neither a legacy public id (c:…, si:…), an IAM principal UUID, nor org:{{org}}"),
        )
    })
}

/// Why the command changed nothing.
#[derive(Debug)]
pub enum LinkError {
    /// The database refused, with the program's exact reason.
    Refused(String),
    /// The database could not be reached or failed unexpectedly.
    Database(sqlx::Error),
}

impl fmt::Display for LinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(reason) => write!(formatter, "nothing was changed: {reason}"),
            Self::Database(error) => {
                write!(formatter, "nothing was changed: database error: {error}")
            }
        }
    }
}

impl std::error::Error for LinkError {}

impl From<sqlx::Error> for LinkError {
    fn from(error: sqlx::Error) -> Self {
        match &error {
            sqlx::Error::Database(database) => Self::Refused(database.message().to_owned()),
            _ => Self::Database(error),
        }
    }
}

/// Runs the cutover program with `mapping` and returns its report.
///
/// The work happens on a connection of its own that is closed afterwards, in
/// one transaction that commits, or rolls back with `dry_run`.
///
/// # Errors
///
/// Returns [`LinkError::Refused`] with the exact reason when the program
/// refuses (nothing changed), or the database error.
pub async fn link_identities(
    pool: &PgPool,
    mapping: &Mapping,
    dry_run: bool,
) -> Result<Value, LinkError> {
    let mut connection = pool.acquire().await?.detach();
    let result = run(&mut connection, mapping, dry_run).await;
    connection.close().await.ok();
    let mut report = result?;
    if let Value::Object(fields) = &mut report {
        fields.insert("dry_run".to_owned(), Value::Bool(dry_run));
    }
    Ok(report)
}

async fn run(
    connection: &mut sqlx::PgConnection,
    mapping: &Mapping,
    dry_run: bool,
) -> Result<Value, LinkError> {
    let mut transaction = connection.begin().await?;
    sqlx::raw_sql(
        "CREATE TEMP TABLE link_input ( \
             line integer NOT NULL, key text NOT NULL, accounts_uuid text NOT NULL, \
             kind text, accounts_id text) ON COMMIT DROP; \
         CREATE TEMP TABLE link_settings ( \
             run_id uuid NOT NULL, mapping_sha256 text NOT NULL, mapping jsonb NOT NULL) ON COMMIT DROP;",
    )
    .execute(&mut *transaction)
    .await?;
    for row in mapping.rows() {
        sqlx::query("INSERT INTO link_input VALUES ($1, $2, $3, $4, $5)")
            .bind(i32::try_from(row.line).unwrap_or(i32::MAX))
            .bind(&row.key)
            .bind(&row.accounts_uuid)
            .bind(row.kind.as_deref())
            .bind(row.accounts_id.as_deref())
            .execute(&mut *transaction)
            .await?;
    }
    sqlx::query("INSERT INTO link_settings VALUES ($1, $2, $3)")
        .bind(Uuid::now_v7())
        .bind(mapping.sha256())
        .bind(sqlx::types::Json(mapping.rows()))
        .execute(&mut *transaction)
        .await?;
    sqlx::raw_sql(PROGRAM).execute(&mut *transaction).await?;
    let report: Value = sqlx::query_scalar("SELECT report FROM link_result")
        .fetch_one(&mut *transaction)
        .await?;
    if dry_run {
        transaction.rollback().await?;
    } else {
        transaction.commit().await?;
    }
    Ok(report)
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn count(value: &Value) -> u64 {
    value.as_u64().unwrap_or_default()
}

fn items(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

/// The report as the operator reads it: what was linked, what matched
/// nothing, where each organization's files went, who has no account.
#[must_use]
pub fn summary(report: &Value) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let status = text(&report["status"]);
    let dry_run = report["dry_run"].as_bool().unwrap_or(false);
    let _ = writeln!(
        out,
        "Briefcase link-identities: {}{} (run {})",
        status.replace('_', " "),
        if dry_run {
            ", dry run: nothing was changed"
        } else {
            ""
        },
        text(&report["run_id"]),
    );
    if status == "already_applied" {
        let _ = writeln!(
            out,
            "This mapping is already in place (run {}); nothing to do.",
            text(&report["applied_run"])
        );
        return out;
    }
    if let Some(previous) = report["reverted_run"].as_str() {
        let _ = writeln!(
            out,
            "Undid run {previous} first ({} changes).",
            count(&report["reverted_changes"])
        );
    }
    let unmatched = items(&report["unmatched"]);
    let _ = writeln!(
        out,
        "Mapping: {} lines (sha256 {}), {} identities linked, {} lines matched nothing.",
        count(&report["mapping"]["rows"]),
        text(&report["mapping"]["sha256"]),
        items(&report["linked"]).len(),
        unmatched.len(),
    );
    for row in unmatched {
        let _ = writeln!(
            out,
            "  line {}: {}: {}",
            count(&row["line"]),
            text(&row["key"]),
            text(&row["reason"])
        );
    }
    for organization in items(&report["organizations"]) {
        summarize_organization(&mut out, organization);
    }
    let unlinked = items(&report["unlinked"]);
    if !unlinked.is_empty() {
        let _ = writeln!(
            out,
            "Members without an account (their files are in unclaimed/, they get no shares):"
        );
        for member in unlinked {
            let _ = writeln!(
                out,
                "  {} {} ({})",
                text(&member["organization"]),
                text(&member["legacy_id"]),
                text(&member["status"])
            );
        }
    }
    let _ = writeln!(
        out,
        "Drives created: {}; notifications moved: {}; tag shares expanded: {}; snapshot shares: {}; old addresses kept: {}.",
        count(&report["drives_created"]),
        count(&report["notifications_moved"]),
        count(&report["tag_shares_expanded"]),
        count(&report["snapshot_shares"]),
        count(&report["aliases"]),
    );
    out
}

fn summarize_organization(out: &mut String, organization: &Value) {
    use std::fmt::Write as _;

    let _ = writeln!(
        out,
        "Organization {}: shared spaces go to account {}.",
        text(&organization["organization"]),
        text(&organization["owner"])
    );
    let _ = writeln!(
        out,
        "  moved {} entries ({} shared by link) into {} drives",
        count(&organization["moved_entries"]),
        count(&organization["link_shared_entries"]),
        items(&organization["drives"]).len()
    );
    for space in items(&organization["spaces"]) {
        let _ = writeln!(
            out,
            "  {} (was {}) in drive {}: shared with {} accounts",
            text(&space["path"]),
            text(&space["legacy_path"]),
            text(&space["drive"]),
            count(&space["shared_with"])
        );
    }
    for area in items(&organization["unclaimed"]) {
        let _ = writeln!(
            out,
            "  {} in drive {}: {} entries of {}, who has no account",
            text(&area["path"]),
            text(&area["drive"]),
            count(&area["entries"]),
            text(&area["legacy_id"])
        );
    }
    for renamed in items(&organization["renamed"]) {
        let _ = writeln!(
            out,
            "  renamed {} to {} (drive {}): the name was taken",
            text(&renamed["legacy_path"]),
            text(&renamed["name"]),
            text(&renamed["drive"])
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{parse_mapping, summary};

    #[test]
    fn the_summary_tells_the_operator_where_everything_went() {
        let report = json!({
            "run_id": "0192", "status": "applied", "dry_run": true, "reverted_run": null,
            "mapping": {"rows": 4, "sha256": "ab12"},
            "linked": [{"line": 2}, {"line": 3}],
            "unmatched": [{"line": 5, "key": "c:nobody", "reason": "no member names this id"}],
            "unlinked": [{"organization": "tos", "legacy_id": "c:gone", "status": "active"}],
            "organizations": [{
                "organization": "tos", "owner": "Saq1", "moved_entries": 11,
                "link_shared_entries": 1, "drives": ["Saq1", "Bob1"],
                "spaces": [{"path": "tos-public", "legacy_path": "public", "drive": "Saq1", "shared_with": 2}],
                "unclaimed": [{"path": "unclaimed/c:gone", "drive": "Saq1", "entries": 1, "legacy_id": "c:gone"}],
                "renamed": [{"legacy_path": "private/c:saket/apps", "name": "apps (from tos)", "drive": "Saq1"}]
            }],
            "drives_created": 3, "notifications_moved": 1, "tag_shares_expanded": 1,
            "snapshot_shares": 5, "aliases": 31
        });
        let text = summary(&report);
        for line in [
            "Briefcase link-identities: applied, dry run: nothing was changed (run 0192)",
            "Mapping: 4 lines (sha256 ab12), 2 identities linked, 1 lines matched nothing.",
            "  line 5: c:nobody: no member names this id",
            "Organization tos: shared spaces go to account Saq1.",
            "  moved 11 entries (1 shared by link) into 2 drives",
            "  tos-public (was public) in drive Saq1: shared with 2 accounts",
            "  unclaimed/c:gone in drive Saq1: 1 entries of c:gone, who has no account",
            "  renamed private/c:saket/apps to apps (from tos) (drive Saq1): the name was taken",
            "  tos c:gone (active)",
            "Drives created: 3; notifications moved: 1; tag shares expanded: 1; snapshot shares: 5; old addresses kept: 31.",
        ] {
            assert!(text.contains(line), "missing `{line}` in:\n{text}");
        }
        let again =
            summary(&json!({"status": "already_applied", "run_id": "0193", "applied_run": "0192"}));
        assert!(again.contains("already in place (run 0192)"), "{again}");
    }

    #[test]
    fn mapping_files_name_identities_by_public_id_principal_or_organization() {
        let text = "# exported 2026-10-10\n\
                    iam_principal_id,accounts_uuid,kind,accounts_id\n\
                    c:saket,zQo,carbon,c:saket\n\
                    \n\
                    0191a0b2-6c1e-7cc4-9f3e-2b9a6f1d4e10,Pb7,,si:scout\n\
                    org:tos,zQo,,\n";
        let mapping = parse_mapping(text).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(mapping.rows().len(), 3);
        assert_eq!(mapping.rows()[0].line, 3);
        assert_eq!(mapping.rows()[0].kind.as_deref(), Some("carbon"));
        assert_eq!(mapping.rows()[1].kind, None);
        assert_eq!(mapping.rows()[1].accounts_id.as_deref(), Some("si:scout"));
        assert_eq!(mapping.rows()[2].key, "org:tos");
        assert_eq!(mapping.sha256().len(), 64);
        let minimal = parse_mapping("iam_principal_id,accounts_uuid\nsi:scout,Pb7\n")
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(minimal.rows()[0].accounts_id, None);
    }

    #[test]
    fn mapping_files_are_refused_with_the_line_and_reason() {
        for (text, line, fragment) in [
            ("", 0, "empty"),
            (
                "accounts_uuid,iam_principal_id\nc:a,zQo\n",
                1,
                "header must start",
            ),
            (
                "iam_principal_id,accounts_uuid,email\nc:a,zQo,a@b\n",
                1,
                "unknown column `email`",
            ),
            ("iam_principal_id,accounts_uuid\n", 0, "no identities"),
            (
                "iam_principal_id,accounts_uuid\nc:a,zQo,extra\n",
                2,
                "expected 2 fields",
            ),
            (
                "iam_principal_id,accounts_uuid\n\"c:a\",zQo\n",
                2,
                "never quoted",
            ),
            (
                "iam_principal_id,accounts_uuid\nC:Saket,zQo\n",
                2,
                "not a legacy public id",
            ),
            (
                "iam_principal_id,accounts_uuid\nC:saket,zQo\n",
                2,
                "not a legacy public id",
            ),
            (
                "iam_principal_id,accounts_uuid\nsaket,zQo\n",
                2,
                "neither a legacy public id",
            ),
            (
                "iam_principal_id,accounts_uuid\nc:a,z\n",
                2,
                "not a Silicon Accounts uuid",
            ),
            (
                "iam_principal_id,accounts_uuid\nc:a,zq-o\n",
                2,
                "not a Silicon Accounts uuid",
            ),
            (
                "iam_principal_id,accounts_uuid,kind\nc:a,zQo,human\n",
                2,
                "carbon or silicon",
            ),
            (
                "iam_principal_id,accounts_uuid,accounts_id\nc:a,zQo,ada\n",
                2,
                "not an account id",
            ),
            (
                "iam_principal_id,accounts_uuid,kind\norg:tos,zQo,carbon\n",
                2,
                "only the receiving",
            ),
            (
                "iam_principal_id,accounts_uuid\norg:T,zQo\n",
                2,
                "does not name an organization",
            ),
        ] {
            let error = parse_mapping(text)
                .err()
                .unwrap_or_else(|| panic!("accepted: {text}"));
            assert_eq!(error.line, line, "{text}: {error}");
            assert!(error.reason.contains(fragment), "{text}: {error}");
        }
    }
}
