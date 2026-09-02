//! Entry browsing and folder metadata mutations.

use crate::{
    application::context::ExecutionContext,
    domain::{
        entry::{EntryKind, EntryPath},
        ids::EntryId,
        permission::{Capability, EntryVisibility},
    },
};

use super::{
    ActivityEvent, AuthorizedEntryView, CreateFolderCommand, CreateFolderMutation, EntryListItem,
    ListEntriesQuery, MetadataService, MetadataServiceError, MutationMetadata, Page,
    UpdateEntryCommand, require_capability, validate_context,
};

impl MetadataService {
    /// Lists visible or traversal-safe children of an organization root or folder.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context or parent is
    /// invalid, the parent is hidden, or repository listing/auditing fails.
    pub async fn list_entries(
        &self,
        context: &ExecutionContext,
        query: &ListEntriesQuery,
    ) -> Result<Page<EntryListItem>, MetadataServiceError> {
        validate_context(context)?;

        if let Some(parent_id) = query.parent_id {
            let parent = self
                .repository
                .find_active_entry(context, parent_id)
                .await?
                .ok_or(MetadataServiceError::NotFound)?;
            if parent.entry.kind != EntryKind::Folder {
                return Err(MetadataServiceError::NotFound);
            }
            let authorization = parent.authorization(context.authorization());
            if authorization.visibility() == EntryVisibility::Hidden {
                return Err(MetadataServiceError::NotFound);
            }
        }

        let candidates = self.repository.list_active_children(context, query).await?;
        // A `permissions:` predicate is not a column: persistence returns the
        // candidates and policy decides them here, against the same effective
        // access the response reports.
        let permission_filter = query
            .filter
            .as_ref()
            .and_then(|filter| filter.expression.as_ref())
            .filter(|expression| expression.requires_policy_evaluation());
        let mut accessed = Vec::with_capacity(candidates.items.len());
        let items = candidates
            .items
            .into_iter()
            .filter_map(|entry| {
                let authorization = entry.authorization(context.authorization());
                if let Some(expression) = permission_filter
                    && !expression.permits(&authorization.capabilities().effective_access())
                {
                    return None;
                }
                let item = entry.clone().into_list_item(authorization);
                if item.is_some() {
                    accessed.push(entry.entry.id);
                }
                item
            })
            .collect();
        if !accessed.is_empty() {
            self.repository
                .record_metadata_access(context, &accessed)
                .await?;
        }
        Ok(Page {
            items,
            next_cursor: candidates.next_cursor,
        })
    }

    /// Creates a user folder after applying root or parent policy.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when validation fails, root or parent
    /// authority is absent, an invitee is not a current member, or persistence
    /// fails.
    pub async fn create_folder(
        &self,
        context: &ExecutionContext,
        command: CreateFolderCommand,
        metadata: &MutationMetadata,
    ) -> Result<AuthorizedEntryView, MetadataServiceError> {
        validate_context(context)?;
        metadata.require_key()?;

        let (boundary, required_parent_capability) = if let Some(parent_id) = command.parent_id {
            let parent = self
                .repository
                .find_active_entry(context, parent_id)
                .await?
                .ok_or(MetadataServiceError::NotFound)?;
            if parent.entry.kind != EntryKind::Folder {
                return Err(MetadataServiceError::NotFound);
            }
            require_capability(&parent, context, Capability::CreateChild)?;
            (parent.entry.boundary, Some(Capability::CreateChild))
        } else {
            if !context.authorization().role().has_administrative_access() {
                return Err(MetadataServiceError::Forbidden {
                    required: Capability::CreateChild,
                });
            }
            let boundary =
                command
                    .root_boundary
                    .clone()
                    .ok_or(MetadataServiceError::Validation(super::ValidationError {
                        field: "root_boundary",
                        message: "is required for a user root",
                    }))?;
            (boundary, None)
        };

        for invitee in &command.invitees {
            if !self
                .repository
                .is_current_member(context, &invitee.principal)
                .await?
            {
                return Err(MetadataServiceError::Validation(super::ValidationError {
                    field: "invitees",
                    message: "every invitee must be a current organization member",
                }));
            }
        }

        let mutation = CreateFolderMutation {
            entry_id: EntryId::new(),
            command,
            boundary,
            owner: context.authorization().actor().clone(),
            origin_application_id: context.authorization().originating_application().cloned(),
        };
        let created = self
            .repository
            .create_folder(context, &mutation, metadata, required_parent_capability)
            .await?;
        let authorization = created.authorization(context.authorization());
        created.into_full_view(authorization)
    }

    /// Returns complete metadata for one directly readable active entry.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid,
    /// the entry is unavailable or unreadable, or repository access/auditing
    /// fails.
    pub async fn get_entry(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
    ) -> Result<AuthorizedEntryView, MetadataServiceError> {
        validate_context(context)?;
        let entry = self
            .repository
            .find_active_entry(context, entry_id)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        let authorization = require_capability(&entry, context, Capability::Read)?;
        self.repository
            .record_metadata_access(context, &[entry_id])
            .await?;
        entry.into_full_view(authorization)
    }

    /// Resolves the entry addressed by a permanent URL path.
    ///
    /// An entry the caller cannot read is indistinguishable from one that does
    /// not exist, which is what the permanent URL must return.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid,
    /// nothing readable exists at the path, or repository access fails.
    pub async fn get_entry_by_path(
        &self,
        context: &ExecutionContext,
        path: &EntryPath,
    ) -> Result<AuthorizedEntryView, MetadataServiceError> {
        validate_context(context)?;
        let entry = self
            .repository
            .find_active_entry_by_path(context, path)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        let authorization = require_capability(&entry, context, Capability::Read)?;
        let entry_id = entry.entry.id;
        self.repository
            .record_metadata_access(context, &[entry_id])
            .await?;
        entry.into_full_view(authorization)
    }

    /// Returns the retained action history of one readable entry.
    ///
    /// The history answers "who did what, and when" for the last hundred
    /// recorded actions, which is exactly what the product contract retains.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid,
    /// the entry is unavailable or unreadable, or repository access fails.
    pub async fn entry_activity(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
    ) -> Result<Vec<ActivityEvent>, MetadataServiceError> {
        validate_context(context)?;
        let entry = self
            .repository
            .find_active_entry(context, entry_id)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        require_capability(&entry, context, Capability::Read)?;
        self.repository
            .list_entry_activity(context, entry_id)
            .await
            .map_err(Into::into)
    }

    /// Renames and/or moves an active entry without crossing permission boundaries.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when validation fails, the source or
    /// destination is unavailable, metadata mutation is unauthorized, the move
    /// crosses an access boundary, or persistence fails.
    pub async fn update_entry(
        &self,
        context: &ExecutionContext,
        command: &UpdateEntryCommand,
        metadata: &MutationMetadata,
    ) -> Result<AuthorizedEntryView, MetadataServiceError> {
        validate_context(context)?;
        metadata.require_key()?;
        let source = self
            .repository
            .find_active_entry(context, command.entry_id)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        require_capability(&source, context, Capability::UpdateMetadata)?;

        if let Some(parent_id) = command.parent_id {
            let destination = self
                .repository
                .find_active_entry(context, parent_id)
                .await?
                .ok_or(MetadataServiceError::NotFound)?;
            if destination.entry.kind != EntryKind::Folder {
                return Err(MetadataServiceError::NotFound);
            }
            require_capability(&destination, context, Capability::CreateChild)?;
            if destination.entry.boundary != source.entry.boundary {
                return Err(MetadataServiceError::Conflict);
            }
        }

        let updated = self
            .repository
            .update_entry(context, command, metadata, Capability::UpdateMetadata)
            .await?;
        let authorization = updated.authorization(context.authorization());
        updated.into_full_view(authorization)
    }

    /// Moves an active entry and its complete subtree to the 45-day bin.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid,
    /// the entry is unavailable, deletion is unauthorized, or persistence
    /// fails.
    pub async fn soft_delete_entry(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
        metadata: &MutationMetadata,
    ) -> Result<(), MetadataServiceError> {
        validate_context(context)?;
        let entry = self
            .repository
            .find_active_entry(context, entry_id)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        require_capability(&entry, context, Capability::Delete)?;
        self.repository
            .soft_delete_entry(context, entry_id, metadata, Capability::Delete)
            .await?;
        Ok(())
    }
}
