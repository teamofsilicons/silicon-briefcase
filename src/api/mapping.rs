//! Mapping between transport DTOs and application/domain models.

use std::collections::BTreeSet;

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use url::Url;

use crate::{
    application::service::{
        AccessRequestView, ActivityEvent, AuthorizedEntryView, FileVersionView,
        MetadataRepositoryError, MetadataServiceError, Page, SearchResultView,
    },
    domain::{
        access::AccessRequestStatus,
        actor::{ActorKind, ActorRef, ApplicationId},
        entry::{EntryKind, EntryPath, RootType},
        ids::EntryId,
        media::RenderKind,
        notification::{Notification, NotificationDecision, NotificationInbox, NotificationKind},
        permission::{AccessRight, EffectiveAccess, GrantedAccess, PermissionGrant},
    },
    error::AppError,
};

use super::dto::{
    AccessRequestDto, AccessRequestStatusDto, ActivityEventDto, ActivityPageDto, ActorRefDto,
    ActorTypeDto, EffectiveAccessDto, EffectivePermissionDto, EntryDto, EntryPageDto, EntryTypeDto,
    FileVersionDto, GrantAccessDto, NotificationDecisionDto, NotificationDto, NotificationInboxDto,
    NotificationKindDto, NotificationSubjectDto, PermissionGrantDto, PermissionGrantPageDto,
    PermissionInspectionResultDto, RenderKindDto, RootTypeDto, SearchPageDto, SearchResultDto,
};

/// Characters that must not survive unencoded in a permanent-URL segment.
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
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

/// Builds public response representations with the configured canonical URLs.
#[derive(Clone)]
pub(crate) struct ResponseMapper {
    public_base_url: Url,
    public_site_base_url: Url,
}

impl ResponseMapper {
    pub(crate) fn new(public_base_url: Url, public_site_base_url: Url) -> Self {
        Self {
            public_base_url: with_directory_path(public_base_url),
            public_site_base_url: with_directory_path(public_site_base_url),
        }
    }

    pub(crate) fn entry(&self, view: AuthorizedEntryView) -> Result<EntryDto, AppError> {
        let entry = view.entry;
        let is_file = entry.kind == EntryKind::File;
        let permanent_url = self.permanent_url(entry.organization_id.as_str(), &entry.path)?;
        let content_url = is_file
            .then(|| self.api_url(&format!("entries/{}/content", entry.id)))
            .transpose()?;
        let download_url = is_file
            .then(|| self.api_url(&format!("entries/{}/download", entry.id)))
            .transpose()?;
        let render = is_file.then(|| {
            render_kind(RenderKind::classify(
                entry.name.as_str(),
                entry.content_type.as_deref(),
            ))
        });

        Ok(EntryDto {
            id: entry.id.as_uuid(),
            org_id: entry.organization_id.as_str().to_owned(),
            entry_type: entry_kind(entry.kind),
            name: entry.name.into_inner(),
            path: entry.path.into_inner(),
            parent_id: entry.parent_id.map(crate::domain::ids::EntryId::as_uuid),
            root_type: root_type(entry.boundary.root_type()),
            tag: entry.boundary.tag().map(|tag| tag.as_str().to_owned()),
            content_type: entry.content_type,
            size: entry.size,
            render,
            permanent_url,
            content_url,
            download_url,
            owner: actor(&entry.owner),
            origin_app_id: entry
                .origin_application_id
                .map(crate::domain::actor::ApplicationId::into_inner),
            effective_access: view
                .effective_access
                .into_iter()
                .map(effective_access)
                .collect(),
            created_at: entry.created_at,
            updated_at: entry.updated_at,
            deleted_at: entry.deleted_at,
        })
    }

    /// Builds the contracted clean permanent URL for one entry.
    ///
    /// The organization identifier and every path segment are percent-encoded
    /// so a name containing a URL-significant character cannot change the
    /// resolved location.
    pub(crate) fn permanent_url(
        &self,
        organization_id: &str,
        path: &EntryPath,
    ) -> Result<Url, AppError> {
        let mut target = String::from("org/");
        target.push_str(&encode_segment(organization_id));
        for segment in path.segments() {
            target.push('/');
            target.push_str(&encode_segment(segment));
        }
        target.push('/');
        self.public_site_base_url
            .join(&target)
            .map_err(|_| AppError::Internal {
                category: "permanent_entry_url",
            })
    }

    fn api_url(&self, relative: &str) -> Result<Url, AppError> {
        self.public_base_url
            .join(relative)
            .map_err(|_| AppError::Internal {
                category: "public_entry_url",
            })
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
                Ok(NotificationSubjectDto {
                    entry_id: subject.entry_id.as_uuid(),
                    name: subject.name,
                    entry_type: entry_kind(subject.kind),
                    permanent_url: self.permanent_url(organization_id, &subject.path)?,
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

    pub(crate) fn access_request(request: &AccessRequestView) -> AccessRequestDto {
        AccessRequestDto {
            id: request.id.as_uuid(),
            entry_id: request.entry_id.as_uuid(),
            requested_by: actor(&request.requested_by),
            access: access_rights(request.requested_access),
            status: access_request_status(request.status),
            created_at: request.created_at,
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
                let number =
                    u32::try_from(version.number.get()).map_err(|_| AppError::Internal {
                        category: "version_number",
                    })?;
                Ok(FileVersionDto {
                    id: version.id.as_uuid(),
                    number,
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
        MetadataRepositoryError::Unavailable => AppError::DependencyUnavailable {
            dependency: "database",
        },
        MetadataRepositoryError::Internal(_) => AppError::Internal {
            category: "metadata_repository",
        },
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

const fn access_request_status(value: AccessRequestStatus) -> AccessRequestStatusDto {
    match value {
        AccessRequestStatus::Pending => AccessRequestStatusDto::Pending,
        AccessRequestStatus::Approved => AccessRequestStatusDto::Approved,
        AccessRequestStatus::Denied => AccessRequestStatusDto::Denied,
    }
}
