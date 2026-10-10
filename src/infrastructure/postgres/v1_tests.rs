//! Drive behavior against a real database, as the API's own role (row
//! security on), including the live permission view and the cross-drive
//! functions.
use super::{PostgresRepository, TenantContext};
use crate::{
    application::{
        context::ExecutionContext,
        idempotency::IdempotencyKey,
        service::{
            CreateFolderCommand, GrantPermissionCommand, MetadataRepository, MetadataService,
            MetadataServiceError, MutationMetadata, RevokePermissionCommand, UpdateEntryCommand,
        },
    },
    domain::{
        actor::{
            ActorId, ActorKind, ActorRef, ApplicationId, AuthenticationMode, DriveAccess,
            RequestAuthContext,
        },
        entry::{EntryName, EntryPath},
        notification::NotificationKind,
        permission::{AccessRight, Capability, GrantedAccess},
    },
    error::AppError,
};
use std::sync::Arc;
use uuid::Uuid;

fn account_uuid(prefix: &str) -> String {
    format!("{prefix}{}", &Uuid::now_v7().simple().to_string()[12..])
}

fn carbon(uuid: &str) -> anyhow::Result<ActorRef> {
    Ok(ActorRef::new(ActorKind::Carbon, ActorId::new(uuid)?))
}

fn in_drive(
    owner: &ActorRef,
    caller: &ActorRef,
    access: DriveAccess,
    mode: AuthenticationMode,
) -> anyhow::Result<ExecutionContext> {
    Ok(ExecutionContext::new(
        RequestAuthContext::for_drive(owner.clone(), caller.clone(), access, mode)?,
        "v1-integration",
    ))
}

fn mutation() -> anyhow::Result<MutationMetadata> {
    Ok(MutationMetadata::new(
        Some(IdempotencyKey::new(Uuid::new_v4().to_string())?),
        [0; 32],
    ))
}

async fn pools() -> anyhow::Result<Option<(sqlx::PgPool, sqlx::PgPool)>> {
    let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
        return Ok(None);
    };
    let pool = sqlx::PgPool::connect_with(
        url.parse::<sqlx::postgres::PgConnectOptions>()?
            .options([("search_path", "public")]),
    )
    .await?;
    super::migrate(&pool).await?;
    let api_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE briefcase_api")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with((*pool.connect_options()).clone())
        .await?;
    Ok(Some((pool, api_pool)))
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn drive_roots_links_and_complete_logs() -> anyhow::Result<()> {
    let Some((pool, api_pool)) = pools().await? else {
        return Ok(());
    };
    let repo = Arc::new(PostgresRepository::new(api_pool.clone()));
    let service = MetadataService::new(repo.clone());
    let ada = carbon(&account_uuid("Ada"))?;
    let bob = carbon(&account_uuid("Bob"))?;
    let owner = in_drive(&ada, &ada, DriveAccess::Owner, AuthenticationMode::Bearer)?;
    let viewer = in_drive(&ada, &bob, DriveAccess::Visitor, AuthenticationMode::Bearer)?;

    // The first request creates the drive and its root folder.
    let root = service.drive_root(&owner).await?;
    assert!(root.entry.path.is_drive_root());
    assert_eq!(root.entry.parent_id, None);
    assert!(
        matches!(
            service.drive_root(&viewer).await,
            Err(MetadataServiceError::NotFound)
        ),
        "only the drive's owner rights reach its root"
    );
    // The top of a drive never gets a link.
    assert!(!repo.link_access(&owner, root.entry.id).await?.can_manage);
    assert!(
        repo.set_link_access(&owner, root.entry.id, true, None, &mutation()?)
            .await
            .is_err()
    );
    // `apps` at the top is kept for apps.
    assert!(
        service
            .create_folder(
                &owner,
                CreateFolderCommand::new(EntryName::new("apps")?, None, vec![])?,
                &mutation()?,
            )
            .await
            .is_err()
    );
    // A visitor cannot add to someone else's drive top.
    assert!(
        service
            .create_folder(
                &viewer,
                CreateFolderCommand::new(EntryName::new("intrusion")?, None, vec![])?,
                &mutation()?,
            )
            .await
            .is_err()
    );

    let folder = service
        .create_folder(
            &owner,
            CreateFolderCommand::new(EntryName::new("shared")?, None, vec![])?,
            &mutation()?,
        )
        .await?;
    assert_eq!(folder.entry.path.as_str(), "shared");
    assert_eq!(folder.entry.parent_id, Some(root.entry.id));
    let child = service
        .create_folder(
            &owner,
            CreateFolderCommand::new(EntryName::new("nested")?, Some(folder.entry.id), vec![])?,
            &mutation()?,
        )
        .await?;
    assert_eq!(child.entry.path.as_str(), "shared/nested");
    let tenant = TenantContext::for_service(ada.id().as_str(), "anonymous-test");
    assert!(matches!(
        repo.public_entry(&tenant, child.entry.path.as_str()).await,
        Err(AppError::NotFound)
    ));
    let shared = repo
        .set_link_access(&owner, folder.entry.id, true, None, &mutation()?)
        .await?;
    assert_eq!(shared.path, folder.entry.path);
    assert_eq!(
        repo.public_entry(&tenant, child.entry.path.as_str())
            .await?
            .id,
        child.entry.id.as_uuid()
    );
    let access = repo.link_access(&owner, child.entry.id).await?;
    assert_eq!(access.path, child.entry.path);
    assert!(
        !access.enabled
            && access.effective
            && access.inherited_from == Some(folder.entry.id.as_uuid())
    );
    assert_eq!(
        repo.public_children(&tenant, folder.entry.path.as_str(), None)
            .await?
            .items
            .len(),
        1
    );
    assert!(
        service
            .get_entry_by_path(&viewer, &child.entry.path)
            .await
            .is_err(),
        "a public link does not widen what a signed-in visitor sees"
    );
    assert!(
        repo.set_link_access(&viewer, folder.entry.id, false, None, &mutation()?)
            .await
            .is_err()
    );
    repo.set_link_access(&owner, folder.entry.id, false, None, &mutation()?)
        .await?;
    assert!(matches!(
        repo.public_entry(&tenant, child.entry.path.as_str()).await,
        Err(AppError::NotFound)
    ));
    for i in 0..105 {
        repo.set_link_access(&owner, child.entry.id, i % 2 == 0, None, &mutation()?)
            .await?;
    }
    let first = repo.logs(&owner, folder.entry.id, None, 100).await?;
    assert_eq!(first.items.len(), 100);
    assert!(first.next_cursor.is_some());
    let second = repo
        .logs(&owner, folder.entry.id, first.next_cursor, 100)
        .await?;
    assert!(
        !second.items.is_empty(),
        "logs must outlive the 100-event activity window"
    );
    assert!(
        first
            .items
            .iter()
            .any(|event| event.action == "child.entry.link_access_changed.v1")
    );
    assert!(
        repo.list_entry_activity(&owner, child.entry.id)
            .await?
            .len()
            <= 100
    );
    // Moving to the top of the drive is a move to its root folder.
    let moved_up = service
        .update_entry(
            &owner,
            &UpdateEntryCommand::new(child.entry.id, None, Some(root.entry.id))?,
            &mutation()?,
        )
        .await?;
    assert_eq!(moved_up.entry.path.as_str(), "nested");
    let moved = repo.logs(&owner, folder.entry.id, None, 100).await?;
    assert!(
        moved
            .items
            .iter()
            .any(|event| event.action == "child.entry.metadata_updated.v1"
                && event.metadata["moved"] == true),
        "the previous parent retains the removal/move event"
    );
    api_pool.close().await;
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn app_folders_and_shares_across_drives_are_live() -> anyhow::Result<()> {
    let Some((pool, api_pool)) = pools().await? else {
        return Ok(());
    };
    let repo = Arc::new(PostgresRepository::new(api_pool.clone()));
    let service = MetadataService::new(repo.clone());
    let ada = carbon(&account_uuid("Ada"))?;
    let bob = carbon(&account_uuid("Bob"))?;
    let owner = in_drive(&ada, &ada, DriveAccess::Owner, AuthenticationMode::Bearer)?;
    let notes = || -> anyhow::Result<AuthenticationMode> {
        Ok(AuthenticationMode::OnBehalfOf {
            application_id: ApplicationId::new("notes")?,
        })
    };
    let app = in_drive(&ada, &ada, DriveAccess::Owner, notes()?)?;
    let app_folder = service.application_folder(&app).await?;
    assert_eq!(app_folder.entry.path.as_str(), "apps/notes");
    assert_eq!(
        service.application_folder(&app).await?.entry.id,
        app_folder.entry.id,
        "an existing app folder is reused"
    );
    let private = service
        .create_folder(
            &owner,
            CreateFolderCommand::new(EntryName::new("private-notes")?, None, vec![])?,
            &mutation()?,
        )
        .await?;
    assert!(
        service
            .get_entry_by_path(&app, &EntryPath::new("private-notes")?)
            .await
            .is_err(),
        "an app stays inside its own folder"
    );
    assert!(
        service
            .get_entry_by_path(&app, &EntryPath::new("apps")?)
            .await
            .is_err(),
        "an app does not see the apps container itself"
    );

    // Bob's app folder lives in Bob's drive; Ada's drive stays out of reach.
    let bob_app = in_drive(&bob, &bob, DriveAccess::Owner, notes()?)?;
    let bob_folder = service.application_folder(&bob_app).await?;
    assert_eq!(bob_folder.entry.path.as_str(), "apps/notes");
    assert_ne!(bob_folder.entry.id, app_folder.entry.id);
    assert!(
        repo.find_active_entry(&bob_app, app_folder.entry.id)
            .await?
            .is_none(),
        "row security keeps Ada's drive out of Bob's"
    );

    // Ada shares a folder with Bob, who acts on it inside Ada's drive.
    let visitor = in_drive(&ada, &bob, DriveAccess::Visitor, AuthenticationMode::Bearer)?;
    let bob_home = in_drive(&bob, &bob, DriveAccess::Owner, AuthenticationMode::Bearer)?;
    assert!(
        repo.find_active_entry(&visitor, private.entry.id)
            .await?
            .is_some_and(
                |entry| entry.authorization(visitor.authorization()).visibility()
                    == crate::domain::permission::EntryVisibility::Hidden
            ),
        "nothing is visible before a share"
    );
    let sharing = owner.clone().with_share_recipients(vec![bob.clone()]);
    let grant = service
        .grant_permission(
            &sharing,
            &GrantPermissionCommand {
                entry_id: private.entry.id,
                principal: bob.clone(),
                access: GrantedAccess::new([AccessRight::Update]),
                inherits_to_descendants: true,
                lifetime: None,
            },
            &mutation()?,
        )
        .await?;
    let authorized = repo
        .find_active_entry(&visitor, private.entry.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("missing shared entry"))?;
    assert!(
        authorized
            .authorization(visitor.authorization())
            .allows(Capability::UpdateMetadata)
    );
    assert!(
        !authorized
            .authorization(visitor.authorization())
            .allows(Capability::Delete),
        "delete is never shared"
    );
    // The share is found from Bob's side through the cross-drive lookup.
    let shared_with_bob = repo.shared_with(&bob, None, 10).await?;
    assert_eq!(shared_with_bob.len(), 1);
    assert_eq!(shared_with_bob[0].drive, ada.id().as_str());
    assert_eq!(shared_with_bob[0].entry_id, private.entry.id.as_uuid());
    assert_eq!(
        repo.shared_drives(&bob, 10).await?,
        vec![ada.id().as_str().to_owned()]
    );
    assert_eq!(
        repo.entry_drive(private.entry.id.as_uuid())
            .await?
            .as_deref(),
        Some(ada.id().as_str())
    );
    assert_eq!(repo.entry_drive(Uuid::now_v7()).await?, None);

    // The notification reached Bob's own inbox, naming Ada's drive.
    let inbox = repo.load_notification_inbox(&bob_home).await?;
    let granted = inbox
        .items
        .iter()
        .find(|item| item.kind == NotificationKind::AccessGranted)
        .ok_or_else(|| anyhow::anyhow!("missing share notification"))?;
    let subject = granted
        .subject
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing notification subject"))?;
    assert_eq!(subject.drive.as_deref(), Some(ada.id().as_str()));
    assert_eq!(subject.path.as_str(), "private-notes");
    assert!(
        repo.load_notification_inbox(&owner).await?.items.is_empty(),
        "nothing landed in Ada's inbox"
    );

    // Revoking is idempotent under its key and notifies once.
    let revoke_key = mutation()?;
    repo.revoke_tag_or_member(&owner, private.entry.id, grant.id().as_uuid(), &revoke_key)
        .await?;
    repo.revoke_tag_or_member(&owner, private.entry.id, grant.id().as_uuid(), &revoke_key)
        .await?;
    let inbox = repo.load_notification_inbox(&bob_home).await?;
    assert_eq!(
        inbox
            .items
            .iter()
            .filter(|n| n.kind == NotificationKind::AccessRevoked)
            .count(),
        1,
        "an idempotent revocation notifies once"
    );
    assert!(repo.shared_with(&bob, None, 10).await?.is_empty());
    let revoked = repo
        .find_active_entry(&visitor, private.entry.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("missing entry"))?;
    assert!(
        !revoked
            .authorization(visitor.authorization())
            .allows(Capability::Read)
    );
    assert!(matches!(
        service
            .revoke_permission(
                &owner,
                RevokePermissionCommand {
                    entry_id: private.entry.id,
                    grant_id: grant.id(),
                },
                &mutation()?,
            )
            .await,
        Err(MetadataServiceError::NotFound)
    ));

    // Share listings page through every live grant.
    let suffix = account_uuid("P");
    let mut tx = repo.begin(&TenantContext::from_execution(&owner)).await?;
    sqlx::query(
        "INSERT INTO briefcase.organization_members (org_id, actor_type, actor_id) \
         SELECT $1, 'carbon', left($2, 20) || n FROM generate_series(1, 105) n",
    )
    .bind(ada.id().as_str())
    .bind(&suffix)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO briefcase.permission_grants (org_id, entry_id, grant_id, principal_type, \
                principal_id, access_mask, inherits_to_descendants, granted_by_type, granted_by_id) \
         SELECT $1, $2, gen_random_uuid(), 'carbon', left($3, 20) || n, 1, true, 'carbon', $1 \
           FROM generate_series(1, 105) n",
    )
    .bind(ada.id().as_str())
    .bind(private.entry.id.as_uuid())
    .bind(&suffix)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let page = repo.invitations(&owner, private.entry.id, None).await?;
    assert_eq!(page["items"].as_array().map(Vec::len), Some(100));
    let cursor = page["next_cursor"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing page cursor"))?
        .parse()?;
    let last = repo
        .invitations(&owner, private.entry.id, Some(cursor))
        .await?;
    assert_eq!(last["items"].as_array().map(Vec::len), Some(5));
    assert!(last["next_cursor"].is_null());
    api_pool.close().await;
    pool.close().await;
    Ok(())
}
