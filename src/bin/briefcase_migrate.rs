//! Silicon Briefcase one-shot database migration process.
//!
//! ```text
//! briefcase-migrate                      apply every pending schema migration
//! briefcase-migrate link-identities --file mapping.csv [--dry-run] [--json]
//!                                        move pre-migration data to Silicon
//!                                        Accounts identities and drives
//! ```

use std::path::PathBuf;

use silicon_briefcase::{
    config::MigrationSettings,
    infrastructure::postgres::{self, link_identities},
    telemetry,
};

const USAGE: &str =
    "usage: briefcase-migrate [link-identities --file MAPPING.csv [--dry-run] [--json]]

Without arguments, applies every pending schema migration.

link-identities links each identity from before Silicon Accounts to its account
and moves each organization's files into personal drives, in one transaction:
  --file PATH   the mapping (header iam_principal_id,accounts_uuid[,kind][,accounts_id])
  --dry-run     do everything, print the report, then roll back
  --json        print the report as JSON instead of a summary
Run it with the API and worker stopped, after briefcase-migrate, and keep a
backup. Running it again with the same mapping changes nothing; with a
corrected mapping it first undoes the run in place.";

enum Command {
    Migrate,
    Link {
        file: PathBuf,
        dry_run: bool,
        json: bool,
    },
}

fn parse(arguments: &[String]) -> Result<Command, String> {
    let Some((first, rest)) = arguments.split_first() else {
        return Ok(Command::Migrate);
    };
    match first.as_str() {
        "-h" | "--help" | "help" => Err(String::new()),
        "link-identities" => {
            let (mut file, mut dry_run, mut json) = (None, false, false);
            let mut rest = rest.iter();
            while let Some(argument) = rest.next() {
                match argument.as_str() {
                    "--file" => {
                        file = Some(PathBuf::from(
                            rest.next()
                                .ok_or("--file needs the path of the mapping file")?,
                        ));
                    }
                    "--dry-run" => dry_run = true,
                    "--json" => json = true,
                    other => return Err(format!("unknown argument `{other}`")),
                }
            }
            let file = file.ok_or("link-identities needs --file with the mapping file")?;
            Ok(Command::Link {
                file,
                dry_run,
                json,
            })
        }
        other => Err(format!("unknown command `{other}`")),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse(&arguments) {
        Ok(command) => command,
        Err(reason) if reason.is_empty() => {
            println!("{USAGE}");
            return Ok(());
        }
        Err(reason) => {
            eprintln!("briefcase-migrate: {reason}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let settings = MigrationSettings::from_env()?;
    telemetry::init_process(settings.environment, &settings.log_filter)?;
    let pool = postgres::connect(&settings.database, "briefcase-migrate").await?;
    let outcome = match command {
        Command::Migrate => postgres::migrate(&pool).await,
        Command::Link {
            file,
            dry_run,
            json,
        } => link(&pool, &file, dry_run, json).await,
    };
    pool.close().await;
    outcome
}

async fn link(
    pool: &sqlx::PgPool,
    file: &std::path::Path,
    dry_run: bool,
    json: bool,
) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(file)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", file.display()))?;
    let mapping = link_identities::parse_mapping(&text)?;
    let report = link_identities::link_identities(pool, &mapping, dry_run).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", link_identities::summary(&report));
    }
    Ok(())
}
