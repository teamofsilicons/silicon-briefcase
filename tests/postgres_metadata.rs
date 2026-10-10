//! Repository checks that run against a live PostgreSQL instance.
//!
//! These execute the SQL the repository actually builds — drive creation,
//! path resolution, the compiled filter language, the notification inbox, and
//! the application folder — so a statement that only type-checks cannot pass
//! for working. The whole file is skipped unless
//! `BRIEFCASE_TEST_DATABASE_URL` names a database whose role may create the
//! `briefcase` schema and write rows, because that is exactly what the first
//! request of a new drive does.
//!
//! ```bash
//! BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5460/briefcase_mig \
//!   cargo test --test postgres_metadata
//! ```

use std::{num::NonZeroU32, sync::Arc, time::Duration};

use secrecy::SecretString;
use silicon_briefcase::{
    application::{
        context::ExecutionContext,
        idempotency::IdempotencyKey,
        service::{
            CreateFolderCommand, EntryListItem, GrantPermissionCommand, ListEntriesQuery,
            MetadataRepository, MetadataRepositoryError, MetadataService, MetadataServiceError,
            MutationMetadata, PageRequest, RestoreBinEntryCommand, RevokePermissionCommand,
            SearchQuery,
        },
    },
    config::DatabaseSettings,
    domain::{
        actor::{
            ActorId, ActorKind, ActorRef, ApplicationId, AuthenticationMode, DriveAccess,
            RequestAuthContext,
        },
        entry::{EntryName, EntryPath},
        filter::FilterQuery,
        ids::EntryId,
        permission::{AccessRight, Capability, GrantedAccess},
    },
    infrastructure::postgres::{self, PostgresRepository},
};
use uuid::Uuid;

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

/// A fresh account uuid: a short prefix and random characters.
fn account_uuid(prefix: &str) -> String {
    format!("{prefix}{}", &Uuid::now_v7().simple().to_string()[12..])
}

fn carbon(uuid: &str) -> ActorRef {
    ActorRef::new(
        ActorKind::Carbon,
        ActorId::new(uuid).unwrap_or_else(|error| panic!("test account: {error}")),
    )
}

fn in_drive(
    owner: &ActorRef,
    caller: &ActorRef,
    access: DriveAccess,
    authentication: AuthenticationMode,
    request: &str,
) -> ExecutionContext {
    ExecutionContext::new(
        RequestAuthContext::for_drive(owner.clone(), caller.clone(), access, authentication)
            .unwrap_or_else(|error| panic!("test context: {error}")),
        request,
    )
}

fn entry_name(item: &EntryListItem) -> &str {
    match item {
        EntryListItem::Full(view) => view.entry.name.as_str(),
        EntryListItem::Traversal(view) => view.name.as_str(),
    }
}

fn key(value: &str, fingerprint: u8) -> anyhow::Result<MutationMetadata> {
    Ok(MutationMetadata::new(
        Some(IdempotencyKey::new(format!("{value}-{}", Uuid::now_v7()))?),
        [fingerprint; 32],
    ))
}

async fn folder(
    metadata: &MetadataService,
    context: &ExecutionContext,
    parent: Option<EntryId>,
    name: &str,
    fingerprint: u8,
) -> anyhow::Result<EntryId> {
    Ok(metadata
        .create_folder(
            context,
            CreateFolderCommand::new(EntryName::new(name)?, parent, Vec::new())?,
            &key(name, fingerprint)?,
        )
        .await?
        .entry
        .id)
}

async fn share(
    metadata: &MetadataService,
    owner: &ExecutionContext,
    entry_id: EntryId,
    recipient: &ActorRef,
    access: GrantedAccess,
    inherit: bool,
) -> anyhow::Result<silicon_briefcase::domain::permission::PermissionGrant> {
    let sharing = owner.clone().with_share_recipients(vec![recipient.clone()]);
    Ok(metadata
        .grant_permission(
            &sharing,
            &GrantPermissionCommand {
                entry_id,
                principal: recipient.clone(),
                access,
                inherits_to_descendants: inherit,
                lifetime: None,
            },
            &key("share", 90)?,
        )
        .await?)
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
    let metadata = MetadataService::new(Arc::new(repository.clone()));

    // A fresh drive per run keeps the test additive: it never has to delete
    // anything to be repeatable.
    let ada = carbon(&account_uuid("Ada"));
    let viewer = carbon(&account_uuid("Vwr"));
    let peer = carbon(&account_uuid("Per"));
    let outsider = carbon(&account_uuid("Out"));
    let context = in_drive(
        &ada,
        &ada,
        DriveAccess::Owner,
        AuthenticationMode::Bearer,
        "owner",
    );
    let viewer_context = in_drive(
        &ada,
        &viewer,
        DriveAccess::Visitor,
        AuthenticationMode::Bearer,
        "viewer",
    );
    let outsider_context = in_drive(
        &ada,
        &outsider,
        DriveAccess::Visitor,
        AuthenticationMode::Bearer,
        "outsider",
    );
    // `for:` resolves accounts through the account cache.
    for (account, kind) in [
        (&viewer, "carbon"),
        (&outsider, "carbon"),
        (&peer, "carbon"),
    ] {
        sqlx::query("INSERT INTO briefcase.accounts (account_uuid, kind) VALUES ($1, $2)")
            .bind(account.id().as_str())
            .bind(kind)
            .execute(&pool)
            .await?;
    }

    // The first request creates the drive; its top lists nothing yet but the
    // folders made below.
    let top = repository
        .list_active_children(
            &context,
            &ListEntriesQuery {
                parent_id: None,
                filter: None,
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    assert!(top.items.is_empty(), "a new drive starts empty");

    // Mixed SQL/policy boolean expressions retain their exact meaning, and a
    // chronological take is applied only after authorization and permission
    // filtering. Five viewer-created folders are deliberately older than nine
    // owner-created ones so `last:5 permissions:delete` must scan past an
    // entire newer, non-matching block.
    let space = folder(&metadata, &context, None, "shared-space", 1).await?;
    share(
        &metadata,
        &context,
        space,
        &viewer,
        GrantedAccess::new([AccessRight::Write]),
        true,
    )
    .await?;
    for index in 0_u8..5 {
        folder(
            &metadata,
            &viewer_context,
            Some(space),
            &format!("viewer-{index}"),
            index.saturating_add(1),
        )
        .await?;
    }
    for index in 0_u8..9 {
        let name = if index == 0 {
            "owner-apple".to_owned()
        } else {
            format!("owner-{index}")
        };
        folder(
            &metadata,
            &context,
            Some(space),
            &name,
            index.saturating_add(16),
        )
        .await?;
    }
    let listing = |filter: &str| -> anyhow::Result<ListEntriesQuery> {
        Ok(ListEntriesQuery {
            parent_id: Some(space),
            filter: Some(FilterQuery::parse(filter)?),
            page: PageRequest::new(None, 100)?,
        })
    };

    let mixed_or = metadata
        .list_entries(
            &viewer_context,
            &listing("name:'owner-apple' or permissions:delete")?,
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
            &listing("not (name:'owner-apple' or permissions:delete)")?,
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
        .list_entries(&viewer_context, &listing("last:5 permissions:delete")?)
        .await?;
    assert_eq!(latest_owned.items.len(), 5);
    assert!(
        latest_owned
            .items
            .iter()
            .all(|entry| entry_name(entry).starts_with("viewer-"))
    );
    let earliest_peer_owned = metadata
        .list_entries(&viewer_context, &listing("first:5 -permissions:delete")?)
        .await?;
    assert_eq!(earliest_peer_owned.items.len(), 5);
    assert!(
        earliest_peer_owned
            .items
            .iter()
            .all(|entry| entry_name(entry).starts_with("owner-"))
    );

    // A folder's creator retains read access to content a collaborator
    // creates below it. Search and `accessible-to` narrow before their result
    // limits, so their SQL must carry the same owned-ancestor rule as policy.
    // The viewer may only add to this folder (no inherited read), so what it
    // sees below is exactly what the owned-ancestor rule gives it.
    let owned_space = folder(&metadata, &context, None, "owned-space", 41).await?;
    share(
        &metadata,
        &context,
        owned_space,
        &viewer,
        GrantedAccess::new([AccessRight::Write]),
        false,
    )
    .await?;
    let owned_folder = folder(
        &metadata,
        &viewer_context,
        Some(owned_space),
        "owned-search-container",
        42,
    )
    .await?;
    let peer_file_id = Uuid::now_v7();
    let peer_version_id = Uuid::now_v7();
    let mut peer_file_transaction = pool.begin().await?;
    sqlx::query("SET CONSTRAINTS briefcase.entries_current_version_fk DEFERRED")
        .execute(&mut *peer_file_transaction)
        .await?;
    sqlx::query(
        "INSERT INTO briefcase.organization_members (org_id, actor_type, actor_id) \
         VALUES ($1, 'carbon', $2)",
    )
    .bind(ada.id().as_str())
    .bind(peer.id().as_str())
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
    .bind(ada.id().as_str())
    .bind(peer_file_id)
    .bind(owned_folder.as_uuid())
    .bind(peer.id().as_str())
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
    .bind(ada.id().as_str())
    .bind(peer_file_id)
    .bind(peer_version_id)
    .bind(format!("audit/{peer_file_id}"))
    .bind("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
    .bind(peer.id().as_str())
    .execute(&mut *peer_file_transaction)
    .await?;
    sqlx::query(
        "INSERT INTO briefcase.search_documents ( \
                org_id, entry_id, filename, extracted_content, extraction_status, indexed_at \
         ) VALUES ($1, $2, 'ancestorneedle.txt', 'owned ancestor search proof', 'indexed', clock_timestamp())",
    )
    .bind(ada.id().as_str())
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
        "a folder's creator must find what others put inside it"
    );
    let outsider_search = metadata
        .search(&outsider_context, &SearchQuery::new("ancestorneedle", 20)?)
        .await?;
    assert!(
        outsider_search.is_empty(),
        "a visitor with no share must not find the descendant"
    );
    let owner_search = metadata
        .search(&context, &SearchQuery::new("ancestorneedle", 20)?)
        .await?;
    assert_eq!(owner_search.len(), 1, "the drive's owner finds everything");
    let hidden_path = EntryPath::new("owned-space/owned-search-container/ancestorneedle.txt")?;
    assert!(matches!(
        metadata
            .get_entry_by_path(&outsider_context, &hidden_path)
            .await,
        Err(MetadataServiceError::NotFound)
    ));

    let accessible = |id: &ActorRef| -> anyhow::Result<ListEntriesQuery> {
        Ok(ListEntriesQuery {
            parent_id: Some(owned_folder),
            filter: Some(FilterQuery::parse(&format!(
                "for:@{{{}}}",
                id.id().as_str()
            ))?),
            page: PageRequest::new(None, 100)?,
        })
    };
    let accessible_to_viewer = metadata
        .list_entries(&context, &accessible(&viewer)?)
        .await?;
    assert_eq!(accessible_to_viewer.items.len(), 1);
    assert_eq!(
        entry_name(&accessible_to_viewer.items[0]),
        "ancestorneedle.txt"
    );
    let accessible_to_outsider = metadata
        .list_entries(&context, &accessible(&outsider)?)
        .await?;
    assert!(accessible_to_outsider.items.is_empty());
    let accessible_to_owner = metadata.list_entries(&context, &accessible(&ada)?).await;
    assert!(
        accessible_to_owner.is_ok(),
        "an account the cache does not hold simply matches nothing"
    );

    // The compiled filter language narrows by kind and location.
    let filtered = repository
        .list_active_children(
            &context,
            &ListEntriesQuery {
                parent_id: None,
                filter: Some(FilterQuery::parse("is:folder location:'shared-space'")?),
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?;
    assert!(
        !filtered.items.is_empty()
            && filtered.items.iter().all(|entry| entry
                .entry
                .path
                .as_str()
                .starts_with("shared-space")),
        "a location filter selects only that subtree, never the drive root"
    );

    // Every documented predicate compiles into runnable SQL.
    let exhaustive = FilterQuery::parse(
        "last:5 (between:12-06-2020=12-07-2030 or after:20-08-2020) before:01-01-2035 \
         location:'shared-space' (contains:'apple' or has:'cat') name:'q*' \
         from:@{c:cos} to:@{c:someone} for:@{si:agent} \
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
    let space_path = EntryPath::new("shared-space")?;
    let batch = repository
        .find_active_entries(
            &context,
            std::slice::from_ref(&space),
            std::slice::from_ref(&space_path),
        )
        .await?;
    assert_eq!(batch.len(), 1, "one target resolved twice stays one row");

    // An application request materializes its own folder in the drive.
    let application_context = in_drive(
        &ada,
        &ada,
        DriveAccess::Owner,
        AuthenticationMode::OnBehalfOf {
            application_id: ApplicationId::new(APPLICATION_ID)?,
        },
        "application",
    );
    let app_folder = repository
        .ensure_application_folder(&application_context)
        .await?;
    assert_eq!(
        app_folder.entry.path.as_str(),
        format!("apps/{APPLICATION_ID}")
    );
    let again = repository
        .ensure_application_folder(&application_context)
        .await?;
    assert_eq!(again.entry.id, app_folder.entry.id);

    // Loading activity is itself metadata access. It is committed before the
    // history query so the response contains the access that produced it.
    let metadata_reads_before = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM briefcase.audit_events \
          WHERE org_id = $1 AND entry_id = $2 AND action = 'entry.metadata_read.v1'",
    )
    .bind(ada.id().as_str())
    .bind(space.as_uuid())
    .fetch_one(&pool)
    .await?;
    let activity = metadata.entry_activity(&context, space).await?;
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
    .bind(ada.id().as_str())
    .bind(space.as_uuid())
    .fetch_one(&pool)
    .await?;
    assert_eq!(metadata_reads_after, metadata_reads_before + 1);

    pool.close().await;
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn bin_restore_replays_require_current_authority_and_the_original_root() -> anyhow::Result<()>
{
    let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
        eprintln!("skipping: BRIEFCASE_TEST_DATABASE_URL is not set");
        return Ok(());
    };
    let pool = postgres::connect(&settings(url), "briefcase-bin-tests").await?;
    postgres::migrate(&pool).await?;
    let repository = PostgresRepository::new(pool.clone());
    let service = MetadataService::new(Arc::new(repository.clone()));
    let ada = carbon(&account_uuid("Ada"));
    let bob = carbon(&account_uuid("Bob"));
    let owner = in_drive(
        &ada,
        &ada,
        DriveAccess::Owner,
        AuthenticationMode::Bearer,
        "bin-owner",
    );
    let member = in_drive(
        &ada,
        &bob,
        DriveAccess::Visitor,
        AuthenticationMode::Bearer,
        "bin-member",
    );
    // Bob may add to the workspace and update the folder that will be
    // deleted and restored.
    let workspace = folder(&service, &owner, None, "workspace", 1).await?;
    share(
        &service,
        &owner,
        workspace,
        &bob,
        GrantedAccess::new([AccessRight::Write]),
        false,
    )
    .await?;
    let restore_root = folder(&service, &owner, Some(workspace), "restore-root", 2).await?;
    let child = folder(&service, &owner, Some(restore_root), "restore-child", 3).await?;
    let grant = share(
        &service,
        &owner,
        restore_root,
        &bob,
        GrantedAccess::new([AccessRight::Update]),
        true,
    )
    .await?;
    service
        .soft_delete_entry(&owner, restore_root, &key("delete-1", 4)?)
        .await?;
    let restore = MutationMetadata::new(
        Some(IdempotencyKey::new(format!(
            "restore-root-{}",
            Uuid::now_v7()
        ))?),
        [5; 32],
    );
    assert!(matches!(
        repository
            .restore_bin_entry(&member, child, &restore, Capability::UpdateMetadata)
            .await,
        Err(MetadataRepositoryError::NotFound)
    ));
    assert!(matches!(
        service
            .restore_bin_entry(
                &member,
                RestoreBinEntryCommand { entry_id: child },
                &restore
            )
            .await,
        Err(MetadataServiceError::NotFound)
    ));

    let command = RestoreBinEntryCommand {
        entry_id: restore_root,
    };
    let first = service
        .restore_bin_entry(&member, command, &restore)
        .await?;
    let replay = service
        .restore_bin_entry(&member, command, &restore)
        .await?;
    assert_eq!(
        first, replay,
        "a completed keyed restore is replayable through the service"
    );
    assert_eq!(first.entry.parent_id, Some(workspace));
    assert!(repository.find_active_entry(&owner, child).await?.is_some());
    let restoration_count = repository
        .list_entry_activity(&owner, restore_root)
        .await?
        .iter()
        .filter(|event| event.action == "entry.subtree_restored.v1")
        .count();
    assert_eq!(
        restoration_count, 1,
        "a replay must not repeat restore/audit effects"
    );
    assert!(matches!(
        service
            .restore_bin_entry(&member, command, &MutationMetadata::new(None, [5; 32]))
            .await,
        Err(MetadataServiceError::NotFound)
    ));
    let changed_intent = MutationMetadata::new(restore.idempotency_key.clone(), [6; 32]);
    assert!(matches!(
        service
            .restore_bin_entry(&member, command, &changed_intent)
            .await,
        Err(MetadataServiceError::Conflict)
    ));
    assert!(
        matches!(
            service.restore_bin_entry(&owner, command, &restore).await,
            Err(MetadataServiceError::Conflict)
        ),
        "another account cannot use the completed claim"
    );
    let application_member = in_drive(
        &ada,
        &bob,
        DriveAccess::Visitor,
        AuthenticationMode::OnBehalfOf {
            application_id: ApplicationId::new(APPLICATION_ID)?,
        },
        "bin-application-member",
    );
    assert!(
        matches!(
            service
                .restore_bin_entry(&application_member, command, &restore)
                .await,
            Err(MetadataServiceError::NotFound)
        ),
        "an app cannot restore an entry outside its folder"
    );
    let other = folder(&service, &member, Some(workspace), "other-root", 7).await?;
    assert!(
        matches!(
            service
                .restore_bin_entry(
                    &member,
                    RestoreBinEntryCommand { entry_id: other },
                    &restore
                )
                .await,
            Err(MetadataServiceError::Conflict)
        ),
        "even an identical supplied fingerprint cannot replay a different resource"
    );

    service
        .soft_delete_entry(&owner, restore_root, &key("delete-2", 8)?)
        .await?;
    assert!(
        matches!(
            service.restore_bin_entry(&member, command, &restore).await,
            Err(MetadataServiceError::Conflict)
        ),
        "an old successful restore must not undo a later deletion"
    );
    assert!(
        repository
            .find_bin_entry(&owner, restore_root)
            .await?
            .is_some()
    );
    let fresh_restore = key("restore-again", 9)?;
    service
        .restore_bin_entry(&member, command, &fresh_restore)
        .await?;
    service
        .revoke_permission(
            &owner,
            RevokePermissionCommand {
                entry_id: restore_root,
                grant_id: grant.id(),
            },
            &key("revoke", 10)?,
        )
        .await?;
    assert!(
        matches!(
            service
                .restore_bin_entry(&member, command, &fresh_restore)
                .await,
            Err(MetadataServiceError::NotFound | MetadataServiceError::Forbidden { .. })
        ),
        "a completed key cannot preserve revoked restore rights"
    );
    assert!(
        matches!(
            repository
                .restore_bin_entry(
                    &member,
                    restore_root,
                    &fresh_restore,
                    Capability::UpdateMetadata
                )
                .await,
            Err(MetadataRepositoryError::Conflict)
        ),
        "the locked repository path must also recheck current rights"
    );

    // When the original folder is gone, an entry comes back at the top of
    // the drive, which only the drive's owner may add to.
    let lonely = folder(&service, &owner, Some(workspace), "lonely", 11).await?;
    service
        .soft_delete_entry(&owner, lonely, &key("delete-lonely", 12)?)
        .await?;
    service
        .soft_delete_entry(&owner, workspace, &key("delete-workspace", 13)?)
        .await?;
    let back = service
        .restore_bin_entry(
            &owner,
            RestoreBinEntryCommand { entry_id: lonely },
            &key("restore-lonely", 14)?,
        )
        .await?;
    let root = service.drive_root(&owner).await?;
    assert_eq!(back.entry.parent_id, Some(root.entry.id));
    assert_eq!(back.entry.path.as_str(), "lonely");
    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn bug_reports_are_retry_safe_and_drive_isolated() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
        eprintln!("skipping: BRIEFCASE_TEST_DATABASE_URL is not set");
        return Ok(());
    };
    let pool = postgres::connect(&settings(url), "briefcase-report-tests").await?;
    postgres::migrate(&pool).await?;
    let repository = PostgresRepository::new(pool.clone());
    let reporter = carbon(&account_uuid("Rep"));
    let context = in_drive(
        &reporter,
        &reporter,
        DriveAccess::Owner,
        AuthenticationMode::Bearer,
        "report-test",
    );
    let metadata = MutationMetadata::new(
        Some(IdempotencyKey::new("report-retry".to_owned())?),
        [42; 32],
    );
    let (first, replay) = tokio::join!(
        repository.submit_report(
            &context,
            "Reproduction steps",
            None,
            Some("planner"),
            &metadata
        ),
        repository.submit_report(
            &context,
            "Reproduction steps",
            None,
            Some("planner"),
            &metadata
        )
    );
    let first = first?;
    assert_eq!(first.id, replay?.id);
    assert!(first.accepted);
    let conflict = MutationMetadata::new(metadata.idempotency_key.clone(), [43; 32]);
    assert!(matches!(
        repository
            .submit_report(&context, "Different report", None, None, &conflict)
            .await,
        Err(MetadataRepositoryError::Conflict)
    ));
    let tenant = postgres::TenantContext::from_execution(&context);
    let mut transaction = repository.begin(&tenant).await?;
    sqlx::query("SET LOCAL ROLE briefcase_api")
        .execute(&mut *transaction)
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM briefcase.bug_reports")
        .fetch_one(&mut *transaction)
        .await?;
    assert_eq!(count, 1);
    sqlx::query("SELECT set_config('briefcase.org_id', 'OtherDrive1', true)")
        .execute(&mut *transaction)
        .await?;
    let hidden: i64 = sqlx::query_scalar("SELECT count(*) FROM briefcase.bug_reports")
        .fetch_one(&mut *transaction)
        .await?;
    assert_eq!(hidden, 0);
    transaction.rollback().await?;
    Ok(())
}
