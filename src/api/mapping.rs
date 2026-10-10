//! Mapping between transport DTOs and application/domain models.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use url::Url;

use crate::{
    application::service::{
        ActivityEvent, AuthorizedEntryView, EntryListItem, FileVersionView,
        MetadataRepositoryError, MetadataServiceError, Page, SearchResultView,
    },
    domain::{
        actor::{ActorKind, ActorRef, ApplicationId},
        entry::{EntryKind, EntryPath, RootType},
        ids::EntryId,
        media::RenderKind,
        notification::{Notification, NotificationDecision, NotificationInbox, NotificationKind},
        permission::{AccessRight, EffectiveAccess, GrantedAccess, PermissionGrant},
        quota::OrganizationUsage,
    },
    error::AppError,
};

use super::dto::{
    ActivityEventDto, ActivityPageDto, ActorRefDto, ActorTypeDto, DailyUsageMeasureDto,
    EffectiveAccessDto, EffectivePermissionDto, EntryDto, EntryPageDto, EntryTypeDto,
    EntryVisibilityDto, FileVersionDto, FileVersionSourceDto, GrantAccessDto,
    NotificationDecisionDto, NotificationDto, NotificationInboxDto, NotificationKindDto,
    NotificationSubjectDto, OrganizationUsageDto, PermissionGrantDto, PermissionGrantPageDto,
    PermissionInspectionResultDto, RenderKindDto, RootTypeDto, SearchPageDto, SearchResultDto,
    UsageMeasureDto,
};

/// Returns the next midnight UTC, when a daily allowance returns.
fn next_utc_midnight() -> time::OffsetDateTime {
    let now = time::OffsetDateTime::now_utc();
    now.date()
        .next_day()
        .map_or(now, |day| day.midnight().assume_utc())
}

/// Characters that must not survive unencoded in a permanent-URL segment.
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'\\')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

fn with_directory_path(mut base: Url) -> Url {
    if !base.path().ends_with('/') {
        let normalized_path = format!("{}/", base.path());
        base.set_path(&normalized_path);
    }
    base
}

fn encode_segment(value: &str) -> String {
    utf8_percent_encode(value, PATH_SEGMENT).to_string()
}

/// The path shown for an entry: drive-relative, and empty for the drive's
/// root folder.
fn presented_path(path: EntryPath) -> String {
    if path.is_drive_root() {
        String::new()
    } else {
        path.into_inner()
    }
}

#[cfg(test)]
mod url_tests {
    use super::*;

    fn mapper() -> anyhow::Result<ResponseMapper> {
        Ok(ResponseMapper::new(
            &Url::parse("https://api.example/api/v1/")?,
            Url::parse("https://site.example/")?,
        )
        .with_labels(BTreeMap::from([("Ada1".to_owned(), "c:ada".to_owned())])))
    }

    #[test]
    fn share_links_cover_files_folders_inheritance_and_disabled_access() -> anyhow::Result<()> {
        let mapper = mapper()?;
        for (path, encoded) in [
            ("Shared reports", "Shared%20reports"),
            (
                "Shared reports/draft #1.txt",
                "Shared%20reports/draft%20%231.txt",
            ),
        ] {
            for (enabled, inherited_from) in [
                (true, None),
                (false, Some(uuid::Uuid::new_v4())),
                (false, None),
            ] {
                let effective = enabled || inherited_from.is_some();
                let result = mapper.link_access(
                    "Ada1",
                    &crate::infrastructure::postgres::sharing::LinkAccess {
                        path: EntryPath::new(path)?,
                        can_manage: true,
                        enabled,
                        effective,
                        inherited_from,
                        expires_at: None,
                    },
                )?;
                let document = serde_json::to_value(result)?;
                assert_eq!(document["enabled"], enabled);
                assert_eq!(document["effective"], effective);
                if effective {
                    assert_eq!(
                        document["url"],
                        format!("https://site.example/c:ada/{encoded}")
                    );
                } else {
                    assert!(document["url"].is_null());
                }
                assert!(document.get("path").is_none());
            }
        }
        Ok(())
    }

    #[test]
    fn permanent_urls_name_the_owner_by_id_and_api_urls_by_uuid() -> anyhow::Result<()> {
        let mapper = mapper()?;
        let path = EntryPath::new("notes/plan.md")?;
        assert_eq!(
            mapper.permanent_url("Ada1", &path)?.as_str(),
            "https://site.example/c:ada/notes/plan.md"
        );
        assert_eq!(
            mapper
                .permanent_url("Ada1", &EntryPath::drive_root())?
                .as_str(),
            "https://site.example/c:ada"
        );
        assert_eq!(
            mapper.content_url("Ada1", &path, "inline")?.as_str(),
            "https://api.example/api/v1/paths/Ada1/notes/plan.md?disposition=inline"
        );
        // A drive whose owner's id Briefcase has not learned is named by uuid.
        assert_eq!(
            mapper.permanent_url("Bob1", &path)?.as_str(),
            "https://site.example/Bob1/notes/plan.md"
        );
        assert_eq!(
            mapper
                .api_path_url("c:ada", &path, Some("disposition=inline"))?
                .as_str(),
            "https://api.example/api/v1/paths/c:ada/notes/plan.md?disposition=inline"
        );
        assert_eq!(
            mapper.public_api_url("c:ada", &path, None)?.as_str(),
            "https://api.example/api/v1/public/c:ada/notes/plan.md"
        );
        Ok(())
    }

    #[test]
    fn reserved_characters_remain_in_their_entry_segment() -> anyhow::Result<()> {
        let mapper = mapper()?;
        let name = "résumé 猫 #1 %\\.md";
        let path = EntryPath::new(format!("public/{name}"))?;
        for (url, segment_count) in [
            (mapper.permanent_url("Ada1", &path)?, 3),
            (mapper.content_url("Ada1", &path, "inline")?, 6),
            (mapper.content_url("Ada1", &path, "attachment")?, 6),
        ] {
            let segments = url
                .path_segments()
                .ok_or_else(|| anyhow::anyhow!("URL must have path segments"))?
                .collect::<Vec<_>>();
            assert_eq!(segments.len(), segment_count, "{url}");
            let last = segments[segment_count - 1];
            assert!(last.contains("%5C"));
            assert_eq!(
                percent_encoding::percent_decode_str(last).decode_utf8()?,
                name
            );
        }
        Ok(())
    }
}

/// Builds public response representations with the configured canonical URLs.
///
/// Permanent addresses name a drive by its owner's current `c:`/`si:` id
/// (`https://briefcase.teamofsilicons.com/c:ada/notes/plan.md`); the labels
/// a handler attaches with [`ResponseMapper::with_labels`] say which id each
/// drive key (the owner's uuid) goes by. API content addresses name the drive
/// by uuid, so they stay valid when the owner's id changes.
#[derive(Clone)]
pub(crate) struct ResponseMapper {
    public_api_base: Url,
    public_site_base_url: Url,
    labels: Arc<BTreeMap<String, String>>,
}

impl ResponseMapper {
    pub(crate) fn new(public_base_url: &Url, public_site_base_url: Url) -> Self {
        Self {
            public_api_base: with_directory_path(public_base_url.clone()),
            public_site_base_url: with_directory_path(public_site_base_url),
            labels: Arc::new(BTreeMap::new()),
        }
    }

    /// The same mapper naming drives by the given owner ids (drive key →
    /// current `c:`/`si:` id).
    pub(crate) fn with_labels(&self, labels: BTreeMap<String, String>) -> Self {
        Self {
            labels: Arc::new(labels),
            ..self.clone()
        }
    }

    /// The owner segment of a drive's permanent addresses: the owner's
    /// current id, or its uuid while Briefcase has not learned the id.
    fn label<'a>(&'a self, drive: &'a str) -> &'a str {
        self.labels.get(drive).map_or(drive, String::as_str)
    }

    pub(crate) fn entry(&self, view: AuthorizedEntryView) -> Result<EntryDto, AppError> {
        let entry = view.entry;
        let organization_id = entry.organization_id.as_str().to_owned();
        let is_file = entry.kind == EntryKind::File;
        let permanent_url = self.permanent_url(&organization_id, &entry.path)?;
        let content_url = is_file
            .then(|| self.content_url(&organization_id, &entry.path, "inline"))
            .transpose()?;
        let download_url = is_file
            .then(|| self.content_url(&organization_id, &entry.path, "attachment"))
            .transpose()?;
        let render = is_file.then(|| {
            render_kind(RenderKind::classify(
                entry.name.as_str(),
                entry.content_type.as_deref(),
            ))
        });

        Ok(EntryDto {
            id: entry.id.as_uuid(),
            org_id: organization_id,
            entry_type: entry_kind(entry.kind),
            visibility: EntryVisibilityDto::Full,
            name: entry.name.into_inner(),
            path: presented_path(entry.path),
            parent_id: entry.parent_id.map(crate::domain::ids::EntryId::as_uuid),
            root_type: root_type(entry.boundary.root_type()),
            tag: entry.boundary.tag().map(|tag| tag.as_str().to_owned()),
            content_type: entry.content_type,
            size: entry.size,
            render,
            permanent_url,
            content_url,
            download_url,
            // A reserved container has no proprietor to name.
            owner: (!entry.reserved).then(|| actor(&entry.owner)),
            origin_app_id: entry
                .origin_application_id
                .map(crate::domain::actor::ApplicationId::into_inner),
            effective_access: view
                .effective_access
                .into_iter()
                .map(effective_access)
                .collect(),
            created_at: Some(entry.created_at),
            updated_at: Some(entry.updated_at),
            deleted_at: entry.deleted_at,
            self_destruct_at: entry.self_destruct_at,
        })
    }

    /// Renders one visible entry, redacting a folder shared only by contents.
    ///
    /// A traversal folder is real to the caller — they can open it and see what
    /// was shared inside — so it keeps its identity, path, and permanent URL,
    /// and discloses nothing else.
    pub(crate) fn entry_item(
        &self,
        organization_id: &str,
        item: EntryListItem,
    ) -> Result<EntryDto, AppError> {
        let view = match item {
            EntryListItem::Full(view) => return self.entry(*view),
            EntryListItem::Traversal(view) => view,
        };
        Ok(EntryDto {
            id: view.id.as_uuid(),
            org_id: organization_id.to_owned(),
            entry_type: EntryTypeDto::Folder,
            visibility: EntryVisibilityDto::Traversal,
            name: view.name.into_inner(),
            permanent_url: self.permanent_url(organization_id, &view.path)?,
            path: presented_path(view.path),
            parent_id: view.parent_id.map(EntryId::as_uuid),
            root_type: root_type(view.root_type),
            tag: None,
            content_type: None,
            size: None,
            render: None,
            content_url: None,
            download_url: None,
            owner: None,
            origin_app_id: None,
            effective_access: Vec::new(),
            created_at: None,
            updated_at: None,
            deleted_at: None,
            self_destruct_at: None,
        })
    }

    pub(crate) fn link_access(
        &self,
        organization_id: &str,
        access: &crate::infrastructure::postgres::sharing::LinkAccess,
    ) -> Result<super::dto::LinkAccessDto, AppError> {
        let url = access
            .effective
            .then(|| self.permanent_url(organization_id, &access.path))
            .transpose()?;
        Ok(super::dto::LinkAccessDto {
            can_manage: access.can_manage,
            enabled: access.enabled,
            effective: access.effective,
            inherited_from: access.inherited_from,
            expires_at: access.expires_at,
            url,
        })
    }

    /// Builds the contracted clean permanent URL for one entry,
    /// `{site}/{owner id}/{path}`.
    ///
    /// The owner segment and every path segment are percent-encoded so a
    /// name containing a URL-significant character cannot change the
    /// resolved location.
    pub(crate) fn permanent_url(&self, drive: &str, path: &EntryPath) -> Result<Url, AppError> {
        Self::entry_url(&self.public_site_base_url, "", self.label(drive), path)
    }

    /// The API address resolving a permanent URL, `{api}/paths/{owner}/{path}`.
    pub(crate) fn api_path_url(
        &self,
        owner: &str,
        path: &EntryPath,
        query: Option<&str>,
    ) -> Result<Url, AppError> {
        let mut url = Self::entry_url(&self.public_api_base, "paths/", owner, path)?;
        url.set_query(query);
        Ok(url)
    }

    /// The anonymous link address, `{api}/public/{owner}/{path}`.
    pub(crate) fn public_api_url(
        &self,
        owner: &str,
        path: &EntryPath,
        query: Option<&str>,
    ) -> Result<Url, AppError> {
        let mut url = Self::entry_url(&self.public_api_base, "public/", owner, path)?;
        url.set_query(query);
        Ok(url)
    }

    fn entry_url(base: &Url, prefix: &str, owner: &str, path: &EntryPath) -> Result<Url, AppError> {
        // Every Briefcase address is scoped to a drive: `{base}{prefix}{owner}`
        // is the drive's own address and the entry path continues it. The
        // leading `./` keeps an owner id such as `c:ada` from reading as a URL
        // scheme.
        let mut target = format!("./{prefix}{}", encode_segment(owner));
        if !path.is_drive_root() {
            for segment in path.segments() {
                target.push('/');
                target.push_str(&encode_segment(segment));
            }
        }
        base.join(&target).map_err(|_| AppError::Internal {
            category: "permanent_entry_url",
        })
    }

    /// Builds the authenticated content URL for one file,
    /// `{api}/paths/{drive uuid}/{path}?disposition=…`.
    ///
    /// The same address serves the entry, its rendered bytes and its
    /// download. It names the drive by uuid, so it keeps working when the
    /// owner's id changes.
    fn content_url(
        &self,
        drive: &str,
        path: &EntryPath,
        disposition: &str,
    ) -> Result<Url, AppError> {
        self.api_path_url(drive, path, Some(&format!("disposition={disposition}")))
    }

    pub(crate) fn entry_page(
        &self,
        page: Page<AuthorizedEntryView>,
    ) -> Result<EntryPageDto, AppError> {
        Ok(EntryPageDto {
            items: page
                .items
                .into_iter()
                .map(|entry| self.entry(entry))
                .collect::<Result<_, _>>()?,
            next_cursor: page.next_cursor,
        })
    }

    /// Renders the notification inbox, including its badge count.
    /// Renders organization consumption as exact byte counts.
    pub(crate) fn usage(usage: &OrganizationUsage) -> OrganizationUsageDto {
        OrganizationUsageDto {
            storage: UsageMeasureDto {
                used_bytes: usage.stored_bytes,
                limit_bytes: usage.storage_allowance(),
                remaining_bytes: usage.storage_remaining(),
            },
            daily_uploads: DailyUsageMeasureDto {
                used_bytes: usage.daily_upload_bytes,
                limit_bytes: usage.daily_upload_allowance(),
                remaining_bytes: usage.daily_upload_remaining(),
                resets_at: next_utc_midnight(),
            },
        }
    }

    pub(crate) fn inbox(
        &self,
        organization_id: &str,
        inbox: NotificationInbox,
    ) -> Result<NotificationInboxDto, AppError> {
        let items = inbox
            .items
            .into_iter()
            .map(|item| self.notification(organization_id, item))
            .collect::<Result<_, _>>()?;
        Ok(NotificationInboxDto {
            items,
            unread_count: inbox.unread_count,
        })
    }

    fn notification(
        &self,
        organization_id: &str,
        notification: Notification,
    ) -> Result<NotificationDto, AppError> {
        let read = notification.is_read();
        let subject = notification
            .subject
            .map(|subject| -> Result<NotificationSubjectDto, AppError> {
                let drive = subject.drive.as_deref().unwrap_or(organization_id);
                Ok(NotificationSubjectDto {
                    entry_id: subject.entry_id.as_uuid(),
                    permanent_url: self.permanent_url(drive, &subject.path)?,
                    drive: subject.drive.clone(),
                    name: subject.name,
                    entry_type: entry_kind(subject.kind),
                    path: subject.path.into_inner(),
                })
            })
            .transpose()?;
        Ok(NotificationDto {
            id: notification.id.as_uuid(),
            kind: notification_kind(notification.kind),
            read,
            actor: notification.actor.as_ref().map(actor),
            subject,
            access: notification.access.map(access_rights),
            access_request_id: notification
                .access_request_id
                .map(crate::domain::ids::AccessRequestId::as_uuid),
            decision: notification.decision.map(notification_decision),
            created_at: notification.created_at,
        })
    }

    pub(crate) fn permission(grant: &PermissionGrant) -> PermissionGrantDto {
        PermissionGrantDto {
            id: grant.id().as_uuid(),
            principal: actor(grant.principal()),
            access: access_rights(grant.access()),
            inherit: grant.inheritance().inherit_flag(),
            granted_by: actor(grant.granted_by()),
            created_at: grant.created_at(),
            expires_at: grant.expires_at(),
        }
    }

    /// Reports effective access per target and echoes what stayed unresolved.
    ///
    /// A requested target is unresolved when it does not exist or is hidden;
    /// the answer deliberately cannot tell those apart.
    pub(crate) fn inspection(
        entry_ids: &[EntryId],
        paths: &[EntryPath],
        visible: Vec<AuthorizedEntryView>,
    ) -> PermissionInspectionResultDto {
        let resolved_ids: BTreeSet<EntryId> = visible.iter().map(|view| view.entry.id).collect();
        let resolved_paths: BTreeSet<&str> = visible
            .iter()
            .map(|view| view.entry.path.as_str())
            .collect();
        let unresolved_entry_ids = entry_ids
            .iter()
            .filter(|id| !resolved_ids.contains(id))
            .map(|id| id.as_uuid())
            .collect();
        let unresolved_paths = paths
            .iter()
            .filter(|path| !resolved_paths.contains(path.as_str()))
            .map(|path| path.as_str().to_owned())
            .collect();
        let items = visible
            .into_iter()
            .map(|view| EffectivePermissionDto {
                entry_id: view.entry.id.as_uuid(),
                path: view.entry.path.into_inner(),
                entry_type: entry_kind(view.entry.kind),
                effective_access: view
                    .effective_access
                    .into_iter()
                    .map(effective_access)
                    .collect(),
            })
            .collect();
        PermissionInspectionResultDto {
            items,
            unresolved_entry_ids,
            unresolved_paths,
        }
    }

    pub(crate) fn permissions(grants: &[PermissionGrant]) -> PermissionGrantPageDto {
        PermissionGrantPageDto {
            items: grants.iter().map(Self::permission).collect(),
        }
    }

    pub(crate) fn search(&self, results: Vec<SearchResultView>) -> Result<SearchPageDto, AppError> {
        let items = results
            .into_iter()
            .map(|result| {
                if !result.score.is_finite() {
                    return Err(AppError::Internal {
                        category: "search_score",
                    });
                }
                Ok(SearchResultDto {
                    entry: self.entry(result.entry)?,
                    score: result.score,
                    filename_match: result.filename_match,
                    content_hits: result.content_hits,
                    snippets: result.snippets,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(SearchPageDto { items })
    }

    pub(crate) fn activity(events: Vec<ActivityEvent>) -> ActivityPageDto {
        ActivityPageDto {
            items: events
                .into_iter()
                .map(|event| ActivityEventDto {
                    action: event.action,
                    actor: actor(&event.actor),
                    app_id: event.application_id.map(ApplicationId::into_inner),
                    occurred_at: event.occurred_at,
                })
                .collect(),
        }
    }

    pub(crate) fn versions(
        versions: Vec<FileVersionView>,
    ) -> Result<Vec<FileVersionDto>, AppError> {
        versions
            .into_iter()
            .map(|version| {
                Ok(FileVersionDto {
                    sha256: version.sha256,
                    source: match version.source {
                        crate::domain::version::VersionSource::InitialUpload => {
                            FileVersionSourceDto::InitialUpload
                        }
                        crate::domain::version::VersionSource::Upload => {
                            FileVersionSourceDto::Upload
                        }
                        crate::domain::version::VersionSource::Restore { .. } => {
                            FileVersionSourceDto::Restore
                        }
                    },
                    id: version.id.as_uuid(),
                    number: version.number.get(),
                    size: version.size,
                    created_by: actor(&version.created_by),
                    created_at: version.created_at,
                })
            })
            .collect()
    }
}

pub(crate) fn metadata_error(error: MetadataServiceError) -> AppError {
    match error {
        MetadataServiceError::Validation(validation) if validation.field == "reserved_name" => {
            AppError::unprocessable("reserved_name", validation.message)
        }
        MetadataServiceError::Validation(validation) => {
            AppError::validation(format!("invalid_{}", validation.field))
        }
        MetadataServiceError::NotFound => AppError::NotFound,
        MetadataServiceError::Forbidden { .. } => AppError::Forbidden,
        MetadataServiceError::Conflict => AppError::conflict("metadata_conflict"),
        MetadataServiceError::Repository(repository) => repository_error(&repository),
    }
}

fn repository_error(error: &MetadataRepositoryError) -> AppError {
    match error {
        MetadataRepositoryError::NotFound => AppError::NotFound,
        MetadataRepositoryError::Conflict => AppError::conflict("metadata_conflict"),
        // A cursor the caller made up is a request error, not a conflict with
        // the current state of anything, and retrying it will never help.
        MetadataRepositoryError::InvalidCursor => AppError::bad_request("invalid_cursor"),
        MetadataRepositoryError::Unavailable => AppError::DependencyUnavailable {
            dependency: "database",
        },
        MetadataRepositoryError::Internal(source) => {
            // The cause stays in the service's own log; callers get the
            // category only.
            tracing::error!(
                event = "metadata_repository_failed",
                error = %format!("{source:#}"),
                "metadata repository failure"
            );
            AppError::Internal {
                category: "metadata_repository",
            }
        }
    }
}

fn actor(value: &ActorRef) -> ActorRefDto {
    ActorRefDto {
        actor_type: match value.kind() {
            ActorKind::Carbon => ActorTypeDto::Carbon,
            ActorKind::Silicon => ActorTypeDto::Silicon,
        },
        id: value.id().as_str().to_owned(),
    }
}

const fn entry_kind(value: EntryKind) -> EntryTypeDto {
    match value {
        EntryKind::File => EntryTypeDto::File,
        EntryKind::Folder => EntryTypeDto::Folder,
    }
}

const fn notification_kind(value: NotificationKind) -> NotificationKindDto {
    match value {
        NotificationKind::AccessGranted => NotificationKindDto::AccessGranted,
        NotificationKind::AccessRevoked => NotificationKindDto::AccessRevoked,
        NotificationKind::AccessRequested => NotificationKindDto::AccessRequested,
        NotificationKind::AccessRequestDecided => NotificationKindDto::AccessRequestDecided,
    }
}

const fn notification_decision(value: NotificationDecision) -> NotificationDecisionDto {
    match value {
        NotificationDecision::Approved => NotificationDecisionDto::Approved,
        NotificationDecision::Denied => NotificationDecisionDto::Denied,
    }
}

const fn render_kind(value: RenderKind) -> RenderKindDto {
    match value {
        RenderKind::Image => RenderKindDto::Image,
        RenderKind::Video => RenderKindDto::Video,
        RenderKind::Document => RenderKindDto::Document,
        RenderKind::Spreadsheet => RenderKindDto::Spreadsheet,
        RenderKind::Presentation => RenderKindDto::Presentation,
        RenderKind::Audio => RenderKindDto::Audio,
        RenderKind::Archive => RenderKindDto::Archive,
        RenderKind::Code => RenderKindDto::Code,
        RenderKind::Unsupported => RenderKindDto::Unsupported,
    }
}

const fn root_type(value: RootType) -> RootTypeDto {
    match value {
        RootType::Public => RootTypeDto::Public,
        RootType::Private => RootTypeDto::Private,
        RootType::Tag => RootTypeDto::Tag,
    }
}

const fn effective_access(value: EffectiveAccess) -> EffectiveAccessDto {
    match value {
        EffectiveAccess::Read => EffectiveAccessDto::Read,
        EffectiveAccess::Write => EffectiveAccessDto::Write,
        EffectiveAccess::Update => EffectiveAccessDto::Update,
        EffectiveAccess::Delete => EffectiveAccessDto::Delete,
        EffectiveAccess::ManagePermissions => EffectiveAccessDto::ManagePermissions,
    }
}

fn access_rights(value: GrantedAccess) -> Vec<GrantAccessDto> {
    value
        .rights()
        .map(|right| match right {
            AccessRight::Read => GrantAccessDto::Read,
            AccessRight::Write => GrantAccessDto::Write,
            AccessRight::Update => GrantAccessDto::Update,
            AccessRight::Delete => GrantAccessDto::Delete,
        })
        .collect()
}
