//! Expiring shares and self-destructing files against a real database.
//!
//! Skipped unless `BRIEFCASE_TEST_DATABASE_URL` names a disposable database
//! whose role may create the `briefcase` schema. Expiry is simulated by moving
//! a deadline into the past as the superuser connection; everything else runs
//! as `briefcase_api`, with row-level security, like production.

use std::{path::PathBuf, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest as _, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::{PostgresContentRepository, PostgresRepository, TenantContext};
use crate::{
    application::{
        content::{ContentRepository, Prepared, SmallUploadCommand, StagedContent},
        context::ExecutionContext,
        idempotency::IdempotencyKey,
        ports::{ObjectChecksum, ObjectChecksumAlgorithm, ObjectChecksumType, StoredObject},
        service::{
            CreateFolderCommand, GrantPermissionCommand, ListBinQuery, ListEntriesQuery,
            MetadataRepository, MetadataService, MutationMetadata, PageRequest,
            RestoreBinEntryCommand,
        },
    },
    config::{S3Encryption, S3Settings},
    domain::{
        actor::{
            ActorId, ActorKind, ActorRef, AuthenticationMode, OrganizationId, OrganizationRole,
            RequestAuthContext, TagName,
        },
        entry::{EntryName, EntryPath},
        filter::FilterQuery,
        ids::EntryId,
        lifetime::LifetimeMinutes,
        notification::NotificationKind,
        permission::{AccessRight, Capability, GrantedAccess},
    },
    error::AppError,
};

fn context(
    org: &str,
    actor: &str,
    role: OrganizationRole,
    tags: Vec<TagName>,
) -> anyhow::Result<ExecutionContext> {
    Ok(ExecutionContext::new(
        RequestAuthContext::new(
            OrganizationId::new(org)?,
            ActorRef::new(ActorKind::Carbon, ActorId::new(actor)?),
            role,
            tags,
            AuthenticationMode::Bearer,
        ),
        "lifetime-integration",
    ))
}

fn mutation() -> anyhow::Result<MutationMetadata> {
    Ok(MutationMetadata::new(
        Some(IdempotencyKey::new(Uuid::new_v4().to_string())?),
        [0; 32],
    ))
}

fn minutes(value: u32) -> anyhow::Result<LifetimeMinutes> {
    Ok(LifetimeMinutes::new(value)?)
}

fn storage_settings() -> S3Settings {
    // No object is stored: publication metadata is what is under test.
    S3Settings {
        region: "us-east-1".to_owned(),
        bucket: "briefcase-tests".to_owned(),
        key_prefix: "orgs".to_owned(),
        endpoint_url: None,
        force_path_style: true,
        encryption: S3Encryption::SseS3,
        temporary_directory: PathBuf::from("/tmp"),
        operation_timeout: Duration::from_secs(10),
    }
}

struct Fixture {
    pool: PgPool,
    api_pool: PgPool,
    repo: Arc<PostgresRepository>,
    service: MetadataService,
    files: PostgresContentRepository,
    org: String,
}

async fn fixture() -> anyhow::Result<Option<Fixture>> {
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
    let repo = Arc::new(PostgresRepository::new(api_pool.clone()));
    let service = MetadataService::new(repo.clone());
    let files = PostgresContentRepository::new((*repo).clone(), storage_settings());
    Ok(Some(Fixture {
        pool,
        api_pool,
        repo,
        service,
        files,
        org: format!("life-{}", Uuid::new_v4().simple()),
    }))
}

impl Fixture {
    async fn close(self) {
        self.api_pool.close().await;
        self.pool.close().await;
    }

    /// Reconciles the member (and their private folder) on first sight.
    async fn member(
        &self,
        actor: &str,
        role: OrganizationRole,
        tags: Vec<TagName>,
    ) -> anyhow::Result<ExecutionContext> {
        let member = context(&self.org, actor, role, tags)?;
        self.service
            .get_entry_by_path(&member, &EntryPath::new("public")?)
            .await?;
        Ok(member)
    }

    async fn private_root(&self, member: &ExecutionContext) -> anyhow::Result<EntryId> {
        let path = EntryPath::new(format!(
            "private/{}",
            member.authorization().actor().id().as_str()
        ))?;
        Ok(self.service.get_entry_by_path(member, &path).await?.id())
    }

    async fn folder(
        &self,
        member: &ExecutionContext,
        parent: EntryId,
        name: &str,
    ) -> anyhow::Result<EntryId> {
        Ok(self
            .service
            .create_folder(
                member,
                CreateFolderCommand::new(EntryName::new(name)?, Some(parent), None, vec![])?,
                &mutation()?,
            )
            .await?
            .entry
            .id)
    }

    /// Publishes bytes as `name` without touching object storage.
    async fn upload(
        &self,
        member: &ExecutionContext,
        parent: EntryId,
        name: &str,
        bytes: &[u8],
        self_destruct: Option<LifetimeMinutes>,
    ) -> Result<EntryId, AppError> {
        let staged = std::env::temp_dir().join(format!("briefcase-lifetime-{}", Uuid::now_v7()));
        tokio::fs::write(&staged, bytes)
            .await
            .map_err(|_| AppError::Internal { category: "test" })?;
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let payload = StagedContent {
            path: staged.as_path(),
            offset: 0,
            size: bytes.len() as u64,
            sha256: digest,
        };
        let command = SmallUploadCommand {
            parent_id: parent,
            name: EntryName::new(name).map_err(|_| AppError::validation("name"))?,
            content_type: "text/plain".to_owned(),
            idempotency_key: IdempotencyKey::new(Uuid::new_v4().to_string())
                .map_err(|_| AppError::validation("key"))?,
            request_hash: [7; 32],
            self_destruct,
        };
        let result = match self
            .files
            .prepare_small_upload(member, &command, &payload)
            .await?
        {
            Prepared::Acquired(preparation) => {
                let stored = StoredObject {
                    key: preparation.key.clone(),
                    etag: Some("\"published\"".to_owned()),
                    provider_version_id: None,
                    size: payload.size,
                    checksum: Some(
                        ObjectChecksum::new(
                            ObjectChecksumAlgorithm::Sha256,
                            ObjectChecksumType::FullObject,
                            STANDARD.encode(digest),
                        )
                        .map_err(|_| AppError::validation("checksum"))?,
                    ),
                };
                self.files
                    .commit_small_upload(member, &command, &payload, &preparation, &stored)
                    .await
            }
            Prepared::Replay(entry_id) => Ok(entry_id),
        };
        let _ = tokio::fs::remove_file(&staged).await;
        result
    }

    async fn can_read(&self, member: &ExecutionContext, id: EntryId) -> anyhow::Result<bool> {
        Ok(self
            .repo
            .find_active_entry(member, id)
            .await?
            .is_some_and(|entry| {
                entry
                    .authorization(member.authorization())
                    .allows(Capability::Read)
            }))
    }

    async fn actions(&self, member: &ExecutionContext, id: EntryId) -> anyhow::Result<Vec<String>> {
        Ok(self
            .repo
            .logs(member, id, None, 100)
            .await?
            .items
            .into_iter()
            .map(|event| event.action)
            .collect())
    }

    /// Moves a stored deadline into the past, as if the time had passed.
    async fn expire(&self, statement: &'static str, id: Uuid) -> anyhow::Result<()> {
        let changed = sqlx::query(statement)
            .bind(&self.org)
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        anyhow::ensure!(
            changed == 1,
            "expected one row to expire, changed {changed}"
        );
        Ok(())
    }

    async fn sweep(&self) -> anyhow::Result<()> {
        crate::worker::lifetimes::sweep(&self.pool, 10_000).await?;
        Ok(())
    }

    async fn listed(
        &self,
        member: &ExecutionContext,
        filter: &str,
    ) -> anyhow::Result<Vec<EntryId>> {
        Ok(self
            .service
            .list_entries(
                member,
                &ListEntriesQuery {
                    parent_id: None,
                    filter: Some(FilterQuery::parse(filter)?),
                    page: PageRequest::new(None, 100)?,
                },
            )
            .await?
            .items
            .iter()
            .map(crate::application::service::EntryListItem::id)
            .collect())
    }
}

const EXPIRE_GRANT: &str = "UPDATE briefcase.permission_grants \
     SET expires_at = clock_timestamp() - interval '1 second' \
     WHERE org_id = $1 AND grant_id = $2";
const EXPIRE_TAG_GRANT: &str = "UPDATE briefcase.tag_permission_grants \
     SET expires_at = clock_timestamp() - interval '1 second' \
     WHERE org_id = $1 AND grant_id = $2";
const EXPIRE_LINK: &str = "UPDATE briefcase.entries \
     SET link_expires_at = clock_timestamp() - interval '1 second' \
     WHERE org_id = $1 AND entry_id = $2";
const EXPIRE_FILE: &str = "UPDATE briefcase.entries \
     SET self_destruct_at = clock_timestamp() - interval '1 second' \
     WHERE org_id = $1 AND entry_id = $2";

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn expiring_shares_are_separate_read_only_grants_that_end_on_time() -> anyhow::Result<()> {
    let Some(f) = fixture().await? else {
        return Ok(());
    };
    let owner = f
        .member("owner:tos", OrganizationRole::Member, vec![])
        .await?;
    let guest = f
        .member("guest:tos", OrganizationRole::Member, vec![])
        .await?;
    let partner = f
        .member("partner:tos", OrganizationRole::Member, vec![])
        .await?;
    let outsider = f
        .member("outsider:tos", OrganizationRole::Member, vec![])
        .await?;
    let root = f.private_root(&owner).await?;
    let file = f.upload(&owner, root, "plan.txt", b"plan", None).await?;
    let expiring =
        |principal: &ExecutionContext, access: GrantedAccess, lifetime| GrantPermissionCommand {
            entry_id: file,
            principal: principal.authorization().actor().clone(),
            access,
            inherits_to_descendants: true,
            lifetime,
        };

    // An expiring share only gives read access.
    assert!(
        f.service
            .grant_permission(
                &owner,
                &expiring(
                    &guest,
                    GrantedAccess::new([AccessRight::Update]),
                    Some(minutes(5)?)
                ),
                &mutation()?,
            )
            .await
            .is_err()
    );

    let guest_expiring = f
        .service
        .grant_permission(
            &owner,
            &expiring(&guest, GrantedAccess::READ_ONLY, Some(minutes(5)?)),
            &mutation()?,
        )
        .await?;
    assert!(guest_expiring.expires_at().is_some());
    assert!(f.can_read(&guest, file).await?);

    // A partner holding a permanent grant keeps it when their expiring share ends.
    let permanent = f
        .service
        .grant_permission(
            &owner,
            &expiring(&partner, GrantedAccess::new([AccessRight::Update]), None),
            &mutation()?,
        )
        .await?;
    let partner_expiring = f
        .service
        .grant_permission(
            &owner,
            &expiring(&partner, GrantedAccess::READ_ONLY, Some(minutes(5)?)),
            &mutation()?,
        )
        .await?;
    assert_ne!(
        permanent.id(),
        partner_expiring.id(),
        "an expiring share never amends a permanent grant"
    );
    let listed = f.repo.invitations(&owner, file, None).await?;
    assert_eq!(listed["items"].as_array().map(Vec::len), Some(3));

    // `is:expiring` finds the share for whoever it gave access to and for the
    // member who manages it, and for nobody else.
    assert_eq!(f.listed(&guest, "is:expiring").await?, vec![file]);
    assert_eq!(f.listed(&owner, "is:expiring").await?, vec![file]);
    assert!(f.listed(&outsider, "is:expiring").await?.is_empty());

    // Expiry is exact: nothing has swept yet, and access is already gone.
    f.expire(EXPIRE_GRANT, guest_expiring.id().as_uuid())
        .await?;
    f.expire(EXPIRE_GRANT, partner_expiring.id().as_uuid())
        .await?;
    assert!(!f.can_read(&guest, file).await?);
    assert!(
        f.can_read(&partner, file).await?,
        "the permanent grant stays"
    );
    assert_eq!(
        f.repo.invitations(&owner, file, None).await?["items"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "expired shares leave the listing"
    );
    assert!(f.listed(&guest, "is:expiring").await?.is_empty());

    // The sweep logs each expiry once and notifies nobody.
    f.sweep().await?;
    f.sweep().await?;
    let actions = f.actions(&owner, file).await?;
    assert_eq!(
        actions
            .iter()
            .filter(|a| *a == "permission.expiring_share_expired.v1")
            .count(),
        2
    );
    let inbox = f.repo.load_notification_inbox(&guest).await?;
    assert!(
        !inbox
            .items
            .iter()
            .any(|n| n.kind == NotificationKind::AccessRevoked)
    );
    f.close().await;
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn expiring_shares_extend_shorten_become_permanent_or_end_early() -> anyhow::Result<()> {
    let Some(f) = fixture().await? else {
        return Ok(());
    };
    let owner = f
        .member("owner:tos", OrganizationRole::Member, vec![])
        .await?;
    let guest = f
        .member("guest:tos", OrganizationRole::Member, vec![])
        .await?;
    let partner = f
        .member("partner:tos", OrganizationRole::Member, vec![])
        .await?;
    let root = f.private_root(&owner).await?;
    let folder = f.folder(&owner, root, "drafts").await?;
    let grant = |principal: &ExecutionContext, lifetime| GrantPermissionCommand {
        entry_id: folder,
        principal: principal.authorization().actor().clone(),
        access: GrantedAccess::READ_ONLY,
        inherits_to_descendants: true,
        lifetime,
    };
    let share = f
        .service
        .grant_permission(&owner, &grant(&guest, Some(minutes(5)?)), &mutation()?)
        .await?;
    let id = share.id().as_uuid();

    let extended = f
        .repo
        .change_expiring_share(&owner, folder, id, Some(minutes(600)?), &mutation()?)
        .await?;
    let shortened = f
        .repo
        .change_expiring_share(&owner, folder, id, Some(minutes(1)?), &mutation()?)
        .await?;
    assert_ne!(extended["expires_at"], shortened["expires_at"]);
    // A member who cannot manage the folder cannot change the share.
    assert!(matches!(
        f.repo
            .change_expiring_share(&guest, folder, id, None, &mutation()?)
            .await,
        Err(AppError::NotFound)
    ));

    let kept = f
        .repo
        .change_expiring_share(&owner, folder, id, None, &mutation()?)
        .await?;
    assert!(kept["expires_at"].is_null());
    assert_eq!(kept["id"].as_str(), Some(id.to_string().as_str()));
    assert!(
        matches!(
            f.repo
                .change_expiring_share(&owner, folder, id, Some(minutes(5)?), &mutation()?)
                .await,
            Err(AppError::Conflict { .. })
        ),
        "a permanent grant is not an expiring share"
    );

    // Making a share permanent folds it into an existing permanent grant.
    let permanent = f
        .service
        .grant_permission(&owner, &grant(&partner, None), &mutation()?)
        .await?;
    let partner_expiring = f
        .service
        .grant_permission(&owner, &grant(&partner, Some(minutes(5)?)), &mutation()?)
        .await?;
    let merged = f
        .repo
        .change_expiring_share(
            &owner,
            folder,
            partner_expiring.id().as_uuid(),
            None,
            &mutation()?,
        )
        .await?;
    assert_eq!(
        merged["id"].as_str(),
        Some(permanent.id().as_uuid().to_string().as_str())
    );

    // Ending an expiring share early revokes it quietly.
    let early = f
        .service
        .grant_permission(&owner, &grant(&partner, Some(minutes(5)?)), &mutation()?)
        .await?;
    f.repo
        .revoke_tag_or_member(&owner, folder, early.id().as_uuid(), &mutation()?)
        .await?;
    let actions = f.actions(&owner, folder).await?;
    for expected in [
        "permission.expiring_share_granted.v1",
        "permission.expiring_share_changed.v1",
        "permission.expiring_share_made_permanent.v1",
        "permission.expiring_share_revoked.v1",
    ] {
        assert!(actions.iter().any(|a| a == expected), "missing {expected}");
    }
    let inbox = f.repo.load_notification_inbox(&partner).await?;
    assert!(
        !inbox
            .items
            .iter()
            .any(|n| n.kind == NotificationKind::AccessRevoked),
        "an expiring share ends without a notification"
    );
    f.close().await;
    Ok(())
}

#[tokio::test]
async fn tag_and_link_expiring_shares_end_on_time() -> anyhow::Result<()> {
    let Some(f) = fixture().await? else {
        return Ok(());
    };
    let owner = f
        .member("owner:tos", OrganizationRole::Member, vec![])
        .await?;
    let root = f.private_root(&owner).await?;
    let folder = f.folder(&owner, root, "launch").await?;
    let file = f
        .upload(&owner, folder, "brief.txt", b"brief", None)
        .await?;

    let mut tx = f.repo.begin(&TenantContext::from_execution(&owner)).await?;
    sqlx::query("INSERT INTO briefcase.organization_tags(org_id,tag_id,name) VALUES($1,'design-id','design')")
        .bind(&f.org)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let designer = f
        .member(
            "designer:tos",
            OrganizationRole::Member,
            vec![TagName::new("design")?],
        )
        .await?;
    // Seeing the member with the tag reconciles their membership of it.

    assert!(matches!(
        f.repo
            .invite_tag(
                &owner,
                folder,
                "design-id",
                GrantedAccess::new([AccessRight::Update]),
                true,
                Some(minutes(5)?),
                &mutation()?,
            )
            .await,
        Err(AppError::Validation { .. })
    ));
    let (tag_grant, expires_at) = f
        .repo
        .invite_tag(
            &owner,
            folder,
            "design-id",
            GrantedAccess::READ_ONLY,
            true,
            Some(minutes(5)?),
            &mutation()?,
        )
        .await?;
    assert!(expires_at.is_some());
    assert!(
        f.can_read(&designer, file).await?,
        "the folder share reaches its files"
    );
    f.expire(EXPIRE_TAG_GRANT, tag_grant).await?;
    assert!(!f.can_read(&designer, file).await?);

    // An expiring link opens the folder to anyone, then closes on time.
    let tenant = TenantContext::for_control_service(&f.org, "anonymous-test");
    let file_path = f.service.get_entry(&owner, file).await?.entry.path;
    let link = f
        .repo
        .set_link_access(&owner, folder, true, Some(minutes(5)?), &mutation()?)
        .await?;
    assert!(link.enabled && link.expires_at.is_some());
    assert!(
        f.repo
            .public_entry(&tenant, file_path.as_str())
            .await
            .is_ok()
    );
    f.expire(EXPIRE_LINK, folder.as_uuid()).await?;
    assert!(matches!(
        f.repo.public_entry(&tenant, file_path.as_str()).await,
        Err(AppError::NotFound)
    ));
    assert!(!f.repo.link_access(&owner, folder).await?.enabled);

    // An expiring link never shortens a permanent one.
    f.repo
        .set_link_access(&owner, folder, true, None, &mutation()?)
        .await?;
    assert!(matches!(
        f.repo
            .set_link_access(&owner, folder, true, Some(minutes(5)?), &mutation()?)
            .await,
        Err(AppError::Conflict { .. })
    ));
    f.repo
        .set_link_access(&owner, folder, false, None, &mutation()?)
        .await?;

    f.sweep().await?;
    let actions = f.actions(&owner, folder).await?;
    for expected in [
        "permission.tag_expiring_share_granted.v1",
        "permission.tag_expiring_share_expired.v1",
        "entry.expiring_link_enabled.v1",
        "entry.expiring_link_expired.v1",
    ] {
        assert!(actions.iter().any(|a| a == expected), "missing {expected}");
    }
    f.close().await;
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn self_destructing_files_never_reach_the_bin() -> anyhow::Result<()> {
    let Some(f) = fixture().await? else {
        return Ok(());
    };
    let owner = f
        .member("owner:tos", OrganizationRole::Member, vec![])
        .await?;
    let admin = f
        .member("admin:tos", OrganizationRole::Admin, vec![])
        .await?;
    let editor = f
        .member("editor:tos", OrganizationRole::Member, vec![])
        .await?;
    let root = f.private_root(&owner).await?;

    let file = f
        .upload(&owner, root, "secret.txt", b"one", Some(minutes(1)?))
        .await?;
    let first = f.service.get_entry(&owner, file).await?;
    let deadline = first
        .entry
        .self_destruct_at
        .ok_or_else(|| anyhow::anyhow!("missing self-destruct deadline"))?;
    assert!(
        f.actions(&owner, file)
            .await?
            .iter()
            .any(|a| a == "entry.self_destruct_set.v1")
    );
    assert_eq!(f.listed(&owner, "is:self-destruct").await?, vec![file]);

    // Self destruct is chosen only for a new file; a new version keeps the timer.
    assert!(matches!(
        f.upload(&owner, root, "secret.txt", b"two", Some(minutes(5)?))
            .await,
        Err(AppError::Conflict { .. })
    ));
    assert_eq!(
        f.upload(&owner, root, "secret.txt", b"two", None).await?,
        file
    );
    assert_eq!(
        f.service
            .get_entry(&owner, file)
            .await?
            .entry
            .self_destruct_at,
        Some(deadline)
    );

    // Only the creator, admins and owners may keep it.
    f.service
        .grant_permission(
            &owner,
            &GrantPermissionCommand {
                entry_id: file,
                principal: editor.authorization().actor().clone(),
                access: GrantedAccess::new([AccessRight::Update]),
                inherits_to_descendants: true,
                lifetime: None,
            },
            &mutation()?,
        )
        .await?;
    assert!(matches!(
        f.repo.make_permanent(&editor, file, &mutation()?).await,
        Err(AppError::Forbidden)
    ));
    f.repo.make_permanent(&admin, file, &mutation()?).await?;
    assert_eq!(
        f.service
            .get_entry(&owner, file)
            .await?
            .entry
            .self_destruct_at,
        None
    );
    assert!(
        f.actions(&owner, file)
            .await?
            .iter()
            .any(|a| a == "entry.made_permanent.v1")
    );
    assert!(matches!(
        f.repo.make_permanent(&owner, file, &mutation()?).await,
        Err(AppError::Conflict { .. })
    ));

    // When the time comes the file is deleted for good, and its folder's log
    // says so.
    let timers = f.folder(&owner, root, "timers").await?;
    let doomed = f
        .upload(&owner, timers, "doomed.txt", b"bye", Some(minutes(1)?))
        .await?;
    f.expire(EXPIRE_FILE, doomed.as_uuid()).await?;
    f.sweep().await?;
    assert!(f.repo.find_active_entry(&owner, doomed).await?.is_none());
    let folder_actions = f.actions(&owner, timers).await?;
    assert!(
        folder_actions
            .iter()
            .any(|a| a == "child.entry.self_destructed.v1")
    );

    // Deleting one by hand skips the bin too, alone or inside a folder.
    let by_hand = f
        .upload(&owner, root, "by-hand.txt", b"x", Some(minutes(60)?))
        .await?;
    f.repo
        .soft_delete_entry(&owner, by_hand, &mutation()?, Capability::Delete)
        .await?;
    let folder = f.folder(&owner, root, "mixed").await?;
    let kept = f.upload(&owner, folder, "kept.txt", b"k", None).await?;
    let inside = f
        .upload(&owner, folder, "inside.txt", b"i", Some(minutes(60)?))
        .await?;
    f.repo
        .soft_delete_entry(&owner, folder, &mutation()?, Capability::Delete)
        .await?;
    let bin: Vec<EntryId> = f
        .repo
        .list_bin_entries(
            &owner,
            &ListBinQuery {
                page: PageRequest::new(None, 100)?,
            },
        )
        .await?
        .items
        .into_iter()
        .map(|entry| entry.entry.id)
        .collect();
    assert_eq!(bin, vec![folder], "only the folder waits in the bin");
    f.service
        .restore_bin_entry(
            &owner,
            RestoreBinEntryCommand { entry_id: folder },
            &mutation()?,
        )
        .await?;
    assert!(f.repo.find_active_entry(&owner, kept).await?.is_some());
    assert!(
        f.repo.find_active_entry(&owner, inside).await?.is_none(),
        "restoring the folder cannot bring back a self-destructing file"
    );
    let due: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM briefcase.entries \
          WHERE org_id = $1 AND entry_id = ANY($2) AND purge_after <= clock_timestamp()",
    )
    .bind(&f.org)
    .bind(vec![doomed.as_uuid(), by_hand.as_uuid(), inside.as_uuid()])
    .fetch_one(&f.pool)
    .await?;
    assert_eq!(due, 3, "each is already due for permanent purge");
    f.close().await;
    Ok(())
}
