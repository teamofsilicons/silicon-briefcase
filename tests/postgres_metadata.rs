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

use std::{num::NonZeroU32, sync::Arc, time::Duration};

use secrecy::SecretString;
use silicon_briefcase::{
    application::{
        context::ExecutionContext,
        idempotency::IdempotencyKey,
        service::{
            CreateFolderCommand, EntryListItem, ListEntriesQuery, MetadataRepository,
            MetadataService, MetadataServiceError, MutationMetadata, PageRequest,
            RequestAccessByPathCommand, SearchQuery, TokenAuthorizationQuery,
        },
    },
    config::DatabaseSettings,
    domain::{
        actor::{
            ActorId, ActorKind, ActorRef, ApplicationId, AuthenticationMode, OrganizationId,
            OrganizationRole, RequestAuthContext, TagName,
        },
        entry::{EntryName, EntryPath},
        filter::FilterQuery,
        notification::NotificationKind,
        permission::GrantedAccess,
    },
    infrastructure::postgres::{self, PostgresRepository},
};
use uuid::Uuid;

const ACTOR_ID: &str = "cos:tos";
const APPLICATION_ID: &str = "silicon-dm";
const VIEWER_ID: &str = "cos:filter-viewer";
const PEER_ID: &str = "cos:ancestor-peer";
const OUTSIDER_ID: &str = "cos:ancestor-outsider";

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
    authorization_with_role(
        organization,
        ACTOR_ID,
        OrganizationRole::Owner,
        authentication,
    )
}

fn authorization_with_role(
    organization: &str,
    actor_id: &str,
    role: OrganizationRole,
    authentication: AuthenticationMode,
) -> RequestAuthContext {
    let organization_id = OrganizationId::new(organization.to_owned())
        .unwrap_or_else(|error| panic!("test organization: {error}"));
    let actor = ActorRef::new(
        ActorKind::Carbon,
        ActorId::new(actor_id).unwrap_or_else(|error| panic!("test actor: {error}")),
    );
    RequestAuthContext::new(organization_id, actor, role, Vec::new(), authentication)
}

fn entry_name(item: &EntryListItem) -> &str {
    match item {
        EntryListItem::Full(view) => view.entry.name.as_str(),
        EntryListItem::Traversal(view) => view.name.as_str(),
    }
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

    // Mixed SQL/policy boolean expressions retain their exact meaning, and a
    // chronological take is applied only after authorization and permission
    // filtering. Five member-owned folders are deliberately older than nine
    // peer-owned folders so `last:5 permissions:delete` must scan past an
    // entire newer, non-matching block.
    let public_root_id = roots
        .items
        .iter()
        .find(|entry| entry.entry.name.as_str() == "public")
        .map(|entry| entry.entry.id)
        .ok_or_else(|| anyhow::anyhow!("public root must exist"))?;
    sqlx::query(
        "INSERT INTO briefcase.organization_members \
             (org_id, actor_type, actor_id, org_role, membership_status, iam_version) \
         VALUES ($1, 'carbon', $2, 'member', 'active', 1)",
    )
    .bind(&organization)
    .bind(VIEWER_ID)
    .execute(&pool)
    .await?;
    let viewer_context = ExecutionContext::new(
        authorization_with_role(
            &organization,
            VIEWER_ID,
            OrganizationRole::Member,
            AuthenticationMode::Bearer,
        ),
        "postgres-filter-viewer",
    );
    let metadata = MetadataService::new(Arc::new(repository.clone()));
    for index in 0_u8..5 {
        metadata
            .create_folder(
                &viewer_context,
                CreateFolderCommand::new(
                    EntryName::new(format!("viewer-{index}"))?,
                    Some(public_root_id),
                    None,
                    Vec::new(),
                )?,
                &MutationMetadata::new(
                    Some(IdempotencyKey::new(format!("filter-viewer-{index}"))?),
                    [index.saturating_add(1); 32],
                ),
            )
            .await?;
    }
    for index in 0_u8..9 {
        let name = if index == 0 {
            "owner-apple".to_owned()
        } else {
            format!("owner-{index}")
        };
        metadata
            .create_folder(
                &context,
                CreateFolderCommand::new(
                    EntryName::new(name)?,
                    Some(public_root_id),
                    None,
                    Vec::new(),
                )?,
                &MutationMetadata::new(
                    Some(IdempotencyKey::new(format!("filter-owner-{index}"))?),
                    [index.saturating_add(16); 32],
                ),
            )
            .await?;
    }

    let mixed_or = metadata
        .list_entries(
            &viewer_context,
            &ListEntriesQuery {
                parent_id: Some(public_root_id),
                filter: Some(FilterQuery::parse(
                    "name:'owner-apple' or permissions:delete",
                )?),
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    assert_eq!(mixed_or.items.len(), 6);
    assert!(
        mixed_or
            .items
            .iter()
            .any(|entry| entry_name(entry) == "owner-apple")
    );
    assert_eq!(
        mixed_or
            .items
            .iter()
            .filter(|entry| entry_name(entry).starts_with("viewer-"))
            .count(),
        5
    );

    let mixed_not = metadata
        .list_entries(
            &viewer_context,
            &ListEntriesQuery {
                parent_id: Some(public_root_id),
                filter: Some(FilterQuery::parse(
                    "not (name:'owner-apple' or permissions:delete)",
                )?),
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    assert_eq!(mixed_not.items.len(), 8);
    assert!(
        mixed_not
            .items
            .iter()
            .all(|entry| entry_name(entry).starts_with("owner-")
                && entry_name(entry) != "owner-apple")
    );

    let latest_owned = metadata
        .list_entries(
            &viewer_context,
            &ListEntriesQuery {
                parent_id: Some(public_root_id),
                filter: Some(FilterQuery::parse("last:5 permissions:delete")?),
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    assert_eq!(latest_owned.items.len(), 5);
    assert!(
        latest_owned
            .items
            .iter()
            .all(|entry| entry_name(entry).starts_with("viewer-"))
    );

    let earliest_peer_owned = metadata
        .list_entries(
            &viewer_context,
            &ListEntriesQuery {
                parent_id: Some(public_root_id),
                filter: Some(FilterQuery::parse("first:5 -permissions:delete")?),
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    assert_eq!(earliest_peer_owned.items.len(), 5);
    assert!(
        earliest_peer_owned
            .items
            .iter()
            .all(|entry| entry_name(entry).starts_with("owner-"))
    );

    // A folder owner retains read access to content a collaborator creates
    // below it. Search and `accessible-to` narrow before their result limits,
    // so their SQL must carry the same owned-ancestor rule as domain policy.
    sqlx::query(
        "INSERT INTO briefcase.organization_members \
             (org_id, actor_type, actor_id, org_role, membership_status, iam_version) \
         VALUES ($1, 'carbon', $2, 'member', 'active', 1), \
                ($1, 'carbon', $3, 'member', 'active', 1)",
    )
    .bind(&organization)
    .bind(PEER_ID)
    .bind(OUTSIDER_ID)
    .execute(&pool)
    .await?;
    let viewer_root = repository
        .find_active_entry_by_path(
            &viewer_context,
            &EntryPath::new(format!("private/{VIEWER_ID}"))?,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("the viewer private root must be reconciled"))?;
    let owned_folder = metadata
        .create_folder(
            &viewer_context,
            CreateFolderCommand::new(
                EntryName::new("owned-search-container")?,
                Some(viewer_root.entry.id),
                None,
                Vec::new(),
            )?,
            &MutationMetadata::new(
                Some(IdempotencyKey::new("owned-search-container")?),
                [42; 32],
            ),
        )
        .await?;
    let peer_file_id = Uuid::now_v7();
    let peer_version_id = Uuid::now_v7();
    let mut peer_file_transaction = pool.begin().await?;
    sqlx::query("SET CONSTRAINTS briefcase.entries_current_version_fk DEFERRED")
        .execute(&mut *peer_file_transaction)
        .await?;
    sqlx::query(
        "INSERT INTO briefcase.entries ( \
                org_id, entry_id, parent_id, entry_type, name, root_type, \
                owner_type, owner_id, content_type, size_bytes, current_version_id, \
                created_by_type, created_by_id, updated_by_type, updated_by_id \
         ) VALUES ( \
                $1, $2, $3, 'file', 'ancestorneedle.txt', 'private', \
                'carbon', $4, 'text/plain', 0, $5, \
                'carbon', $4, 'carbon', $4 \
         )",
    )
    .bind(&organization)
    .bind(peer_file_id)
    .bind(owned_folder.entry.id.as_uuid())
    .bind(PEER_ID)
    .bind(peer_version_id)
    .execute(&mut *peer_file_transaction)
    .await?;
    sqlx::query(
        "INSERT INTO briefcase.entry_versions ( \
                org_id, entry_id, version_id, version_number, source, storage_backend, \
                bucket_name, storage_region, storage_prefix, storage_encryption_mode, \
                object_key, checksum_algorithm, checksum_type, checksum_value, size_bytes, \
                content_type, created_by_type, created_by_id \
         ) VALUES ( \
                $1, $2, $3, 1, 'upload', 'platform', \
                'test-bucket', 'us-east-1', '', 'sse_s3', \
                $4, 'sha256', 'full_object', $5, 0, \
                'text/plain', 'carbon', $6 \
         )",
    )
    .bind(&organization)
    .bind(peer_file_id)
    .bind(peer_version_id)
    .bind(format!("audit/{peer_file_id}"))
    .bind("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
    .bind(PEER_ID)
    .execute(&mut *peer_file_transaction)
    .await?;
    sqlx::query(
        "INSERT INTO briefcase.search_documents ( \
                org_id, entry_id, filename, extracted_content, extraction_status, indexed_at \
         ) VALUES ($1, $2, 'ancestorneedle.txt', 'owned ancestor search proof', 'indexed', clock_timestamp())",
    )
    .bind(&organization)
    .bind(peer_file_id)
    .execute(&mut *peer_file_transaction)
    .await?;
    peer_file_transaction.commit().await?;

    let viewer_search = metadata
        .search(&viewer_context, &SearchQuery::new("ancestorneedle", 20)?)
        .await?;
    assert!(
        viewer_search
            .iter()
            .any(|result| result.entry.entry.id.as_uuid() == peer_file_id),
        "a private folder owner must find peer-created descendants"
    );
    let outsider_context = ExecutionContext::new(
        authorization_with_role(
            &organization,
            OUTSIDER_ID,
            OrganizationRole::Member,
            AuthenticationMode::Bearer,
        ),
        "postgres-filter-outsider",
    );
    let outsider_search = metadata
        .search(&outsider_context, &SearchQuery::new("ancestorneedle", 20)?)
        .await?;
    assert!(
        outsider_search.is_empty(),
        "an unrelated peer must not find the private descendant"
    );

    let hidden_path = EntryPath::new(format!(
        "private/{VIEWER_ID}/owned-search-container/ancestorneedle.txt"
    ))?;
    let hidden_request = RequestAccessByPathCommand::new(
        hidden_path.clone(),
        GrantedAccess::READ_ONLY,
        Some("permanent URL access".to_owned()),
    )?;
    let hidden_request_metadata = MutationMetadata::new(
        Some(IdempotencyKey::new("path-access-request-proof")?),
        [71; 32],
    );
    let created_request = metadata
        .request_access_by_path(&outsider_context, &hidden_request, &hidden_request_metadata)
        .await?;
    assert_eq!(created_request.entry_id.as_uuid(), peer_file_id);
    let replayed_request = metadata
        .request_access_by_path(&outsider_context, &hidden_request, &hidden_request_metadata)
        .await?;
    assert_eq!(replayed_request.id, created_request.id);

    let missing_request = RequestAccessByPathCommand::new(
        EntryPath::new(format!(
            "private/{VIEWER_ID}/owned-search-container/not-there.txt"
        ))?,
        GrantedAccess::READ_ONLY,
        None,
    )?;
    assert!(matches!(
        metadata
            .request_access_by_path(
                &outsider_context,
                &missing_request,
                &MutationMetadata::new(None, [72; 32]),
            )
            .await,
        Err(MetadataServiceError::NotFound)
    ));

    // A real path in another tenant is indistinguishable from the missing
    // path above, even when the public actor identifier is the same.
    let foreign_organization = format!("foreign-{}", Uuid::now_v7().simple());
    let foreign_context = ExecutionContext::new(
        authorization_with_role(
            &foreign_organization,
            OUTSIDER_ID,
            OrganizationRole::Member,
            AuthenticationMode::Bearer,
        ),
        "postgres-foreign-path-owner",
    );
    repository
        .list_active_children(
            &foreign_context,
            &ListEntriesQuery {
                parent_id: None,
                filter: None,
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    let foreign_root_path = EntryPath::new(format!("private/{OUTSIDER_ID}"))?;
    let foreign_root = repository
        .find_active_entry_by_path(&foreign_context, &foreign_root_path)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the foreign actor root must exist"))?;
    metadata
        .create_folder(
            &foreign_context,
            CreateFolderCommand::new(
                EntryName::new("foreign-only")?,
                Some(foreign_root.entry.id),
                None,
                Vec::new(),
            )?,
            &MutationMetadata::new(
                Some(IdempotencyKey::new("foreign-path-folder-proof")?),
                [73; 32],
            ),
        )
        .await?;
    let foreign_only_request = RequestAccessByPathCommand::new(
        EntryPath::new(format!("private/{OUTSIDER_ID}/foreign-only"))?,
        GrantedAccess::READ_ONLY,
        None,
    )?;
    assert!(matches!(
        metadata
            .request_access_by_path(
                &outsider_context,
                &foreign_only_request,
                &MutationMetadata::new(None, [74; 32]),
            )
            .await,
        Err(MetadataServiceError::NotFound)
    ));

    let accessible_to_owner = metadata
        .list_entries(
            &context,
            &ListEntriesQuery {
                parent_id: Some(owned_folder.entry.id),
                filter: Some(FilterQuery::parse(&format!("for:@{{carbon:{VIEWER_ID}}}"))?),
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    assert_eq!(accessible_to_owner.items.len(), 1);
    assert_eq!(
        entry_name(&accessible_to_owner.items[0]),
        "ancestorneedle.txt"
    );

    let accessible_to_outsider = metadata
        .list_entries(
            &context,
            &ListEntriesQuery {
                parent_id: Some(owned_folder.entry.id),
                filter: Some(FilterQuery::parse(&format!(
                    "for:@{{carbon:{OUTSIDER_ID}}}"
                ))?),
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    assert!(accessible_to_outsider.items.is_empty());

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

    // The path-addressed request notifies organization decision-makers without
    // requiring the requester to resolve hidden metadata first.
    let inbox = repository.load_notification_inbox(&context).await?;
    assert_eq!(inbox.unread_count, 1);
    assert_eq!(inbox.items.len(), 1);
    assert_eq!(inbox.items[0].kind, NotificationKind::AccessRequested);
    assert_eq!(inbox.items[0].access_request_id, Some(created_request.id));

    // A webhook snapshot adds IAM's immutable identifiers. Bearer authority is
    // returned only for the exact principal, membership, and epoch tuple.
    let principal_id = Uuid::now_v7();
    let membership_id = Uuid::now_v7();
    let updated = sqlx::query(
        "UPDATE briefcase.organization_members \
            SET principal_id = $3, membership_id = $4, authorization_epoch = 7 \
          WHERE org_id = $1 AND actor_type = 'carbon' AND actor_id = $2",
    )
    .bind(&organization)
    .bind(ACTOR_ID)
    .bind(principal_id)
    .bind(membership_id)
    .execute(&pool)
    .await?;
    assert_eq!(updated.rows_affected(), 1);
    let projection_query = TokenAuthorizationQuery {
        organization_id: context.authorization().organization_id(),
        actor_kind: ActorKind::Carbon,
        principal_id,
        membership_id,
        authorization_epoch: 7,
        request_id: "postgres-metadata-test",
        testing_environment: None,
    };
    let projected = repository
        .project_token_authorization(&projection_query)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the exact IAM identity tuple must resolve"))?;
    assert_eq!(projected.actor.id().as_str(), ACTOR_ID);
    assert_eq!(projected.role, OrganizationRole::Owner);
    assert!(
        repository
            .project_token_authorization(&TokenAuthorizationQuery {
                authorization_epoch: 8,
                ..projection_query
            })
            .await?
            .is_none(),
        "a stale authorization epoch must fail closed"
    );

    // An application request materializes its own folder under the actor.
    let tagged_owner = ExecutionContext::new(
        RequestAuthContext::new(
            context.authorization().organization_id().clone(),
            context.authorization().actor().clone(),
            OrganizationRole::Owner,
            vec![TagName::new("research")?],
            AuthenticationMode::Bearer,
        ),
        "postgres-tagged-owner",
    );
    repository
        .find_active_entry_by_path(&tagged_owner, &actor_path)
        .await?;
    sqlx::query("UPDATE briefcase.organization_members SET iam_version = 1 WHERE org_id = $1 AND actor_id = $2")
        .bind(&organization).bind(ACTOR_ID).execute(&pool).await?;
    let application_id = ApplicationId::new(APPLICATION_ID)?;
    let application_context = ExecutionContext::new(
        authorization_with_role(
            &organization,
            ACTOR_ID,
            OrganizationRole::Member,
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
    let preserved = repository
        .project_token_authorization(&projection_query)
        .await?
        .ok_or_else(|| anyhow::anyhow!("OBO must preserve signed identity"))?;
    assert_eq!(
        preserved.role,
        OrganizationRole::Owner,
        "OBO must not downgrade the owner's projection"
    );
    assert!(
        preserved.tags.contains(&TagName::new("research")?),
        "OBO must not erase projected tags"
    );
    // A request authenticated before a signed demotion must not overwrite the
    // newer projection when its repository transaction begins afterward.
    sqlx::query("UPDATE briefcase.organization_members SET org_role = 'member', iam_version = 2, authorization_epoch = 8 WHERE org_id = $1 AND actor_id = $2")
        .bind(&organization).bind(ACTOR_ID).execute(&pool).await?;
    repository
        .find_active_entry_by_path(&context, &actor_path)
        .await?;
    let demoted = repository
        .project_token_authorization(&TokenAuthorizationQuery {
            authorization_epoch: 8,
            ..projection_query
        })
        .await?
        .ok_or_else(|| anyhow::anyhow!("new signed identity must remain"))?;
    assert_eq!(
        demoted.role,
        OrganizationRole::Member,
        "stale bearer must not undo a signed demotion"
    );
    assert!(demoted.tags.contains(&TagName::new("research")?));

    // Loading activity is itself metadata access. It is committed before the
    // history query so the response contains the access that produced it.
    let metadata_reads_before = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM briefcase.audit_events \
          WHERE org_id = $1 AND entry_id = $2 AND action = 'entry.metadata_read.v1'",
    )
    .bind(&organization)
    .bind(actor_root.entry.id.as_uuid())
    .fetch_one(&pool)
    .await?;
    let activity = metadata
        .entry_activity(&context, actor_root.entry.id)
        .await?;
    assert!(activity.len() <= 100);
    assert_eq!(
        activity.first().map(|event| event.action.as_str()),
        Some("entry.metadata_read.v1"),
        "the current activity-list access is visible in its own result"
    );
    let metadata_reads_after = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM briefcase.audit_events \
          WHERE org_id = $1 AND entry_id = $2 AND action = 'entry.metadata_read.v1'",
    )
    .bind(&organization)
    .bind(actor_root.entry.id.as_uuid())
    .fetch_one(&pool)
    .await?;
    assert_eq!(metadata_reads_after, metadata_reads_before + 1);

    pool.close().await;
    Ok(())
}
