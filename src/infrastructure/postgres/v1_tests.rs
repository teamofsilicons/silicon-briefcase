//! Behavior checks against a real database, including the live permission view.
use super::{PostgresRepository, TenantContext};
use crate::{
    application::{
        context::ExecutionContext,
        idempotency::IdempotencyKey,
        service::{
            CreateFolderCommand, MetadataRepository, MetadataService, MutationMetadata,
            UpdateEntryCommand,
        },
    },
    domain::{
        actor::{
            ActorId, ActorKind, ActorRef, AuthenticationMode, OrganizationId, OrganizationRole,
            RequestAuthContext,
        },
        entry::{EntryName, EntryPath},
        permission::{AccessRight, Capability, GrantedAccess},
    },
    error::AppError,
};
use std::sync::Arc;
use uuid::Uuid;

fn context(
    org: &str,
    actor: &str,
    role: OrganizationRole,
    mode: AuthenticationMode,
) -> anyhow::Result<ExecutionContext> {
    Ok(ExecutionContext::new(
        RequestAuthContext::new(
            OrganizationId::new(org)?,
            ActorRef::new(ActorKind::Carbon, ActorId::new(actor)?),
            role,
            vec![],
            mode,
        ),
        "v1-integration",
    ))
}
fn mutation() -> anyhow::Result<MutationMetadata> {
    Ok(MutationMetadata::new(
        Some(IdempotencyKey::new(Uuid::new_v4().to_string())?),
        [0; 32],
    ))
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn public_inheritance_protected_roots_and_complete_logs() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
        return Ok(());
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
    let repo = Arc::new(PostgresRepository::new(api_pool.clone()));
    let service = MetadataService::new(repo.clone());
    let org = format!("v1-{}", Uuid::new_v4().simple());
    let owner = context(
        &org,
        "owner:tos",
        OrganizationRole::Owner,
        AuthenticationMode::Bearer,
    )?;
    let viewer = context(
        &org,
        "viewer:tos",
        OrganizationRole::Member,
        AuthenticationMode::Bearer,
    )?;
    let public_root = service
        .get_entry_by_path(&owner, &EntryPath::new("public")?)
        .await?;
    assert!(repo.link_access(&owner, public_root.id()).await?.can_manage);
    assert!(
        !repo
            .link_access(&viewer, public_root.id())
            .await?
            .can_manage
    );
    let private = service
        .get_entry_by_path(&owner, &EntryPath::new("private/owner:tos")?)
        .await?;
    assert!(
        repo.set_link_access(&owner, private.id(), true, &mutation()?)
            .await
            .is_err()
    );
    let container = service
        .get_entry_by_path(&owner, &EntryPath::new("private")?)
        .await?;
    assert!(
        repo.set_link_access(&owner, container.id(), true, &mutation()?)
            .await
            .is_err()
    );
    let folder = service
        .create_folder(
            &owner,
            CreateFolderCommand::new(EntryName::new("shared")?, Some(private.id()), None, vec![])?,
            &mutation()?,
        )
        .await?;
    let child = service
        .create_folder(
            &owner,
            CreateFolderCommand::new(
                EntryName::new("nested")?,
                Some(folder.entry.id),
                None,
                vec![],
            )?,
            &mutation()?,
        )
        .await?;
    let tenant = TenantContext::for_control_service(&org, "anonymous-test");
    assert!(matches!(
        repo.public_entry(&tenant, child.entry.path.as_str()).await,
        Err(AppError::NotFound)
    ));
    repo.set_link_access(&owner, folder.entry.id, true, &mutation()?)
        .await?;
    assert_eq!(
        repo.public_entry(&tenant, child.entry.path.as_str())
            .await?
            .id,
        child.entry.id.as_uuid()
    );
    let access = repo.link_access(&owner, child.entry.id).await?;
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
        "anonymous permission does not widen authenticated member grants"
    );
    assert!(
        repo.set_link_access(&viewer, folder.entry.id, false, &mutation()?)
            .await
            .is_err()
    );
    repo.set_link_access(&owner, folder.entry.id, false, &mutation()?)
        .await?;
    assert!(matches!(
        repo.public_entry(&tenant, child.entry.path.as_str()).await,
        Err(AppError::NotFound)
    ));
    for i in 0..105 {
        repo.set_link_access(&owner, child.entry.id, i % 2 == 0, &mutation()?)
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
    service
        .update_entry(
            &owner,
            &UpdateEntryCommand::new(child.entry.id, None, Some(private.id()))?,
            &mutation()?,
        )
        .await?;
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
async fn delegated_namespace_and_tag_permissions_are_live() -> anyhow::Result<()> {
    use crate::domain::actor::ApplicationId;
    let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
        return Ok(());
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
    let repo = Arc::new(PostgresRepository::new(api_pool.clone()));
    let service = MetadataService::new(repo.clone());
    let org = format!("v1-{}", Uuid::new_v4().simple());
    let owner = context(
        &org,
        "owner:tos",
        OrganizationRole::Owner,
        AuthenticationMode::Bearer,
    )?;
    let app_mode = AuthenticationMode::OnBehalfOf {
        application_id: ApplicationId::new("tos>notes")?,
    };
    let app = context(&org, "owner:tos", OrganizationRole::Owner, app_mode)?;
    let app_root = service.application_folder(&app).await?;
    assert_eq!(
        app_root.entry.path.as_str(),
        "apps/tos>notes/private/owner:tos"
    );
    assert_eq!(
        service.application_folder(&app).await?.entry.id,
        app_root.entry.id,
        "an existing namespace is reused"
    );
    assert!(
        service
            .get_entry_by_path(&app, &EntryPath::new("private/owner:tos")?)
            .await
            .is_err(),
        "even an OBO owner stays inside the calling app"
    );
    let public = service
        .get_entry_by_path(&app, &EntryPath::new("apps/tos>notes/public")?)
        .await?;
    let shared = service
        .create_folder(
            &owner,
            CreateFolderCommand::new(
                EntryName::new("tag-shared")?,
                Some(public.id()),
                None,
                vec![],
            )?,
            &mutation()?,
        )
        .await?;
    // Seed authoritative directory facts; the SQL view must follow revocation
    // without materializing a permanent grant for the recipient.
    let viewer = context(
        &org,
        "viewer:tos",
        OrganizationRole::Member,
        AuthenticationMode::Bearer,
    )?;
    service
        .get_entry_by_path(&viewer, &EntryPath::new("public")?)
        .await?;
    let viewer_app = context(
        &org,
        "viewer:tos",
        OrganizationRole::Member,
        AuthenticationMode::OnBehalfOf {
            application_id: ApplicationId::new("tos>notes")?,
        },
    )?;
    assert_eq!(
        service
            .application_folder(&viewer_app)
            .await?
            .entry
            .path
            .as_str(),
        "apps/tos>notes/private/viewer:tos"
    );
    assert!(
        service
            .get_entry_by_path(&viewer_app, &app_root.entry.path)
            .await
            .is_err(),
        "the app cannot read another member's private root"
    );
    let mut tx = repo.begin(&TenantContext::from_execution(&owner)).await?;
    sqlx::query("INSERT INTO briefcase.organization_tags(org_id,tag_id,name) VALUES($1,'research-id','research')").bind(&org).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO briefcase.organization_member_tags(org_id,actor_type,actor_id,tag_id) VALUES($1,'carbon','viewer:tos','research-id')").bind(&org).execute(&mut *tx).await?;
    tx.commit().await?;
    repo.invite_tag(
        &owner,
        shared.entry.id,
        "research-id",
        GrantedAccess::new([AccessRight::Update]),
        true,
        &mutation()?,
    )
    .await?;
    let tagged = ExecutionContext::new(
        RequestAuthContext::new(
            OrganizationId::new(&org)?,
            viewer.authorization().actor().clone(),
            OrganizationRole::Member,
            vec![crate::domain::actor::TagName::new("research")?],
            AuthenticationMode::Bearer,
        ),
        "v1-tagged-member",
    );
    let authorized = repo
        .find_active_entry(&tagged, shared.entry.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("missing shared entry"))?;
    assert!(
        authorized
            .authorization(tagged.authorization())
            .allows(Capability::UpdateMetadata)
    );
    assert!(
        !authorized
            .authorization(tagged.authorization())
            .allows(Capability::Delete)
    );
    let listed = repo.invitations(&owner, shared.entry.id, None).await?;
    let grant = listed["items"][0]["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing grant"))?
        .parse()?;
    let revoke_key = mutation()?;
    repo.revoke_tag_or_member(&owner, shared.entry.id, grant, &revoke_key)
        .await?;
    repo.revoke_tag_or_member(&owner, shared.entry.id, grant, &revoke_key)
        .await?;
    let inbox = repo.load_notification_inbox(&tagged).await?;
    assert_eq!(
        inbox
            .items
            .iter()
            .filter(|n| n.kind == crate::domain::notification::NotificationKind::AccessRevoked)
            .count(),
        1,
        "an idempotent revocation notifies each member once"
    );
    repo.invite_tag(
        &owner,
        shared.entry.id,
        "research-id",
        GrantedAccess::new([AccessRight::Update]),
        true,
        &mutation()?,
    )
    .await?;
    let mut tx = repo.begin(&TenantContext::from_execution(&owner)).await?;
    sqlx::query("INSERT INTO briefcase.organization_tags(org_id,tag_id,name) SELECT $1,'page-'||n,'page-'||n FROM generate_series(1,105) n").bind(&org).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO briefcase.tag_permission_grants(org_id,entry_id,grant_id,tag_id,access_mask,inherits_to_descendants,granted_by_type,granted_by_id) SELECT $1,$2,gen_random_uuid(),'page-'||n,1,true,'carbon','owner:tos' FROM generate_series(1,105) n").bind(&org).bind(shared.entry.id.as_uuid()).execute(&mut *tx).await?;
    tx.commit().await?;
    let page = repo.invitations(&owner, shared.entry.id, None).await?;
    assert_eq!(page["items"].as_array().map(Vec::len), Some(100));
    let cursor = page["next_cursor"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing page cursor"))?
        .parse()?;
    let last = repo
        .invitations(&owner, shared.entry.id, Some(cursor))
        .await?;
    assert_eq!(last["items"].as_array().map(Vec::len), Some(6));
    assert!(last["next_cursor"].is_null());
    let mut tx = repo.begin(&TenantContext::from_execution(&owner)).await?;
    sqlx::query(
        "DELETE FROM briefcase.organization_member_tags WHERE org_id=$1 AND actor_id='viewer:tos'",
    )
    .bind(&org)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let revoked = repo
        .find_active_entry(&viewer, shared.entry.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("missing public entry"))?;
    assert!(
        !revoked
            .authorization(viewer.authorization())
            .allows(Capability::UpdateMetadata)
    );
    api_pool.close().await;
    pool.close().await;
    Ok(())
}
