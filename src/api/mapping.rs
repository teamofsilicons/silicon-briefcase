//! Mapping between transport DTOs and application/domain models.

use url::Url;

use crate::{
    application::service::{
        AccessRequestView, AuthorizedEntryView, FileVersionView, MetadataRepositoryError,
        MetadataServiceError, Page, SearchResultView,
    },
    domain::{
        access::AccessRequestStatus,
        actor::{ActorKind, ActorRef},
        entry::{EntryKind, RootType},
        permission::{AccessLevel, EffectiveAccess, PermissionGrant},
    },
    error::AppError,
};

use super::dto::{
    AccessRequestDto, AccessRequestStatusDto, ActorRefDto, ActorTypeDto, EffectiveAccessDto,
    EntryDto, EntryPageDto, EntryTypeDto, FileVersionDto, GrantAccessDto, PermissionGrantDto,
    PermissionGrantPageDto, RootTypeDto, SearchPageDto, SearchResultDto,
};

/// Builds public response representations with the configured canonical URL.
#[derive(Clone)]
pub(crate) struct ResponseMapper {
    public_base_url: Url,
}

impl ResponseMapper {
    pub(crate) fn new(mut public_base_url: Url) -> Self {
        if !public_base_url.path().ends_with('/') {
            let normalized_path = format!("{}/", public_base_url.path());
            public_base_url.set_path(&normalized_path);
        }
        Self { public_base_url }
    }

    pub(crate) fn entry(&self, view: AuthorizedEntryView) -> Result<EntryDto, AppError> {
        let entry = view.entry;
        let permanent_url = self
            .public_base_url
            .join(&format!("entries/{}", entry.id))
            .map_err(|_| AppError::Internal {
                category: "public_entry_url",
            })?;

        Ok(EntryDto {
            id: entry.id.as_uuid(),
            org_id: entry.organization_id.as_str().to_owned(),
            entry_type: entry_kind(entry.kind),
            name: entry.name.into_inner(),
            parent_id: entry.parent_id.map(crate::domain::ids::EntryId::as_uuid),
            root_type: root_type(entry.boundary.root_type()),
            tag: entry.boundary.tag().map(|tag| tag.as_str().to_owned()),
            content_type: entry.content_type,
            size: entry.size,
            permanent_url,
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

    pub(crate) fn permission(grant: &PermissionGrant) -> PermissionGrantDto {
        PermissionGrantDto {
            id: grant.id().as_uuid(),
            principal: actor(grant.principal()),
            access: access_levels(grant.access()),
            inherit: grant.inheritance().inherit_flag(),
            granted_by: actor(grant.granted_by()),
            created_at: grant.created_at(),
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
            access: requested_access(request.requested_access),
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

const fn requested_access(access: AccessLevel) -> super::dto::RequestedAccessDto {
    match access {
        AccessLevel::Read => super::dto::RequestedAccessDto::Read,
        AccessLevel::Write => super::dto::RequestedAccessDto::Write,
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
        EffectiveAccess::Delete => EffectiveAccessDto::Delete,
        EffectiveAccess::ManagePermissions => EffectiveAccessDto::ManagePermissions,
    }
}

fn access_levels(value: AccessLevel) -> Vec<GrantAccessDto> {
    match value {
        AccessLevel::Read => vec![GrantAccessDto::Read],
        AccessLevel::Write => vec![GrantAccessDto::Read, GrantAccessDto::Write],
    }
}

const fn access_request_status(value: AccessRequestStatus) -> AccessRequestStatusDto {
    match value {
        AccessRequestStatus::Pending => AccessRequestStatusDto::Pending,
        AccessRequestStatus::Approved => AccessRequestStatusDto::Approved,
        AccessRequestStatus::Denied => AccessRequestStatusDto::Denied,
    }
}
