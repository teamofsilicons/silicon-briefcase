//! Repository checks that run against a live PostgreSQL instance.
//!
//! These execute the SQL the repository actually builds — reconciliation, path
//! resolution, the compiled filter language, the notification inbox, and the
//! application folder — so a statement that only type-checks cannot pass for
//! working. The whole file is skipped unless
//! `BRIEFCASE_TEST_DATABASE_URL` names a database whose role may create the
//! `briefcase` schema and write projection rows, because that is exactly what
//! the first request of a new organization does.
//!
//! ```bash
//! docker compose up -d postgres
//! BRIEFCASE_TEST_DATABASE_URL=postgres://briefcase:briefcase-local-only@127.0.0.1:5433/briefcase \
//!   cargo test --test postgres_metadata
//! ```

use std::{num::NonZeroU32, time::Duration};

use secrecy::SecretString;
use silicon_briefcase::{
    application::{
        context::ExecutionContext,
        service::{ListEntriesQuery, MetadataRepository, PageRequest},
    },
    config::DatabaseSettings,
    domain::{
        actor::{
            ActorId, ActorKind, ActorRef, ApplicationId, AuthenticationMode, OrganizationId,
            OrganizationRole, RequestAuthContext,
        },
        entry::EntryPath,
        filter::FilterQuery,
    },
    infrastructure::postgres::{self, PostgresRepository},
};
use uuid::Uuid;

const ACTOR_ID: &str = "cos:tos";
const APPLICATION_ID: &str = "silicon-dm";

fn settings(url: String) -> DatabaseSettings {
    DatabaseSettings {
        url: SecretString::from(url),
        max_connections: NonZeroU32::new(4).unwrap_or(NonZeroU32::MIN),
        min_connections: 0,
        acquire_timeout: Duration::from_secs(10),
        statement_timeout: Duration::from_secs(30),
    }
}

fn authorization(organization: &str, authentication: AuthenticationMode) -> RequestAuthContext {
    let organization_id = OrganizationId::new(organization.to_owned())
        .unwrap_or_else(|error| panic!("test organization: {error}"));
    let actor = ActorRef::new(
        ActorKind::Carbon,
        ActorId::new(ACTOR_ID).unwrap_or_else(|error| panic!("test actor: {error}")),
    );
    RequestAuthContext::new(
        organization_id,
        actor,
        OrganizationRole::Owner,
        Vec::new(),
        authentication,
    )
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn the_repository_serves_paths_filters_and_application_folders() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
        eprintln!("skipping: BRIEFCASE_TEST_DATABASE_URL is not set");
        return Ok(());
    };
    let pool = postgres::connect(&settings(url), "briefcase-tests").await?;
    postgres::migrate(&pool).await?;
    let repository = PostgresRepository::new(pool.clone());

    // A fresh organization per run keeps the test additive: it never has to
    // delete anything to be repeatable.
    let organization = format!("test-{}", Uuid::now_v7().simple());
    let context = ExecutionContext::new(
        authorization(&organization, AuthenticationMode::Bearer),
        "postgres-metadata-test",
    );

    // The first request projects the caller and reconciles the reserved roots.
    let roots = repository
        .list_active_children(
            &context,
            &ListEntriesQuery {
                parent_id: None,
                filter: None,
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    let root_names: Vec<String> = roots
        .items
        .iter()
        .map(|entry| entry.entry.name.as_str().to_owned())
        .collect();
    assert!(
        root_names.contains(&"public".to_owned()) && root_names.contains(&"private".to_owned()),
        "reserved containers use their lowercase URL segments: {root_names:?}"
    );

    // The actor's private folder resolves by the path its permanent URL shows.
    let actor_path = EntryPath::new(format!("private/{ACTOR_ID}"))?;
    let actor_root = repository
        .find_active_entry_by_path(&context, &actor_path)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the caller's private folder must be reconciled"))?;
    assert_eq!(actor_root.entry.path, actor_path);

    // The compiled filter language narrows by kind and location.
    let filtered = repository
        .list_active_children(
            &context,
            &ListEntriesQuery {
                parent_id: None,
                filter: Some(FilterQuery::parse("is:folder location:'private'")?),
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    assert!(
        filtered
            .items
            .iter()
            .all(|entry| entry.entry.path.as_str().starts_with("private")),
        "a location filter selects only that subtree"
    );
    assert!(
        filtered
            .items
            .iter()
            .any(|entry| entry.entry.path == actor_path),
        "the caller's own private folder matches the filter"
    );

    // Every documented predicate compiles into runnable SQL.
    let exhaustive = FilterQuery::parse(
        "last:5 (between:12-06-2020=12-07-2030 or after:20-08-2020) before:01-01-2035 \
         location:'private' (contains:'apple' or has:'cat') name:'q*' \
         from:@{carbon:cos:tos} to:@{carbon:someone} for:@{silicon:agent} \
         -is:image is:md permissions:read sort:oldest",
    )?;
    repository
        .list_active_children(
            &context,
            &ListEntriesQuery {
                parent_id: None,
                filter: Some(exhaustive),
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;

    // Batch resolution mixes identifiers and paths in one snapshot.
    let batch = repository
        .find_active_entries(
            &context,
            std::slice::from_ref(&actor_root.entry.id),
            std::slice::from_ref(&actor_path),
        )
        .await?;
    assert_eq!(batch.len(), 1, "one target resolved twice stays one row");

    // The inbox starts empty, and marking it read is a no-op that still answers.
    let inbox = repository.load_notification_inbox(&context).await?;
    assert_eq!(inbox.unread_count, 0);
    assert!(inbox.items.is_empty());

    // The projection answers the role an application request runs with.
    let membership = repository
        .project_member_authorization(
            context.authorization().organization_id(),
            context.authorization().actor(),
            "postgres-metadata-test",
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("the caller must be projected by the first request"))?;
    assert_eq!(membership.role, OrganizationRole::Owner);

    // An application request materializes its own folder under the actor.
    let application_id = ApplicationId::new(APPLICATION_ID)?;
    let application_context = ExecutionContext::new(
        authorization(
            &organization,
            AuthenticationMode::OnBehalfOf { application_id },
        ),
        "postgres-metadata-test",
    );
    let folder = repository
        .ensure_application_folder(&application_context)
        .await?;
    assert_eq!(
        folder.entry.path.as_str(),
        format!("private/{ACTOR_ID}/apps/{APPLICATION_ID}")
    );
    // Materializing it twice returns the same reserved folder.
    let again = repository
        .ensure_application_folder(&application_context)
        .await?;
    assert_eq!(again.entry.id, folder.entry.id);

    // The activity projection reads back the history the reads just recorded.
    let activity = repository
        .list_entry_activity(&context, actor_root.entry.id)
        .await?;
    assert!(activity.len() <= 100);

    pool.close().await;
    Ok(())
}
