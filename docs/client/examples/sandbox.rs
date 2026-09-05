//! Read-only, hands-on example for an already prepared paired test environment.
//! Put this in examples/sandbox.rs of a project using briefcase-client 0.1 and
//! tokio with macros + rt-multi-thread, then run `cargo run --example sandbox`.
//! Inject BRIEFCASE_TEST_ROOT and BRIEFCASE_TEST_BEARER from private storage.
//! This program does not create environments, mutate files, or print secrets.

use briefcase_client::{Client, Config, EnvironmentKey, ListEntries};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::var("BRIEFCASE_URL")?;
    let org = std::env::var("BRIEFCASE_ORG")?;
    let root = EnvironmentKey::new(std::env::var("BRIEFCASE_TEST_ROOT")?)?;
    let bearer = std::env::var("BRIEFCASE_TEST_BEARER")?;
    let client = Client::connect(
        Config::new(&base, &org)?
            .with_auto_update(false)
            .with_environment(root)
            .with_token(bearer),
    )
    .await?;

    let selected = client.current_testing_environment().await?;
    let page = client.list_entries(&ListEntries::default()).await?;
    println!("Selected environment: {}", selected.id);
    println!("Visible entries on first page: {}", page.items.len());
    println!("More pages: {}", page.next_cursor.is_some());
    Ok(())
}
