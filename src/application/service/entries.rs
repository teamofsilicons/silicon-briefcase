//! Entry browsing and folder metadata mutations.

use std::future::Future;

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
    ListEntriesQuery, MetadataRepositoryError, MetadataService, MetadataServiceError,
    MutationMetadata, Page, PageRequest, UpdateEntryCommand, require_capability, validate_context,
};

/// How many extra keyset reads one listing may spend refilling a page.
///
/// A folder whose entries are almost all invisible to the caller would
/// otherwise scan to its end inside a single request. Past this bound the
/// listing answers with what it has and its cursor, which is still complete
/// for a client that follows the cursor.
const MAX_PAGE_REFILLS: usize = 8;

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
        self.validate_listing_parent(context, query.parent_id)
            .await?;

        // A `permissions:` predicate is not a column. For an expression that
        // mixes one with persisted facts, the repository returns each atomic
        // database predicate result and policy evaluates the original boolean
        // tree here without weakening `or` or `not`.
        let policy_filter = query
            .filter
            .as_ref()
            .and_then(|filter| filter.expression.as_ref())
            .filter(|expression| expression.requires_policy_evaluation());
        let take = query.filter.as_ref().and_then(|filter| filter.take);
        let wanted =
            usize::from(take.map_or(query.page.limit, |take| take.count.min(query.page.limit)));
        let mut items: Vec<EntryListItem> = Vec::with_capacity(wanted);
        let mut accessed = Vec::with_capacity(wanted);
        let mut page = query.page.clone();
        let mut next_cursor: Option<String>;
        let mut refills = 0_usize;

        // Candidates the caller may not see are dropped after the keyset page
        // is read, so a page can arrive with room left in it. Refill from the
        // next position rather than answering short: a client that stops when
        // a page is smaller than the limit would otherwise miss the rest of
        // the folder. Each round asks only for what is still missing, so no
        // entry is fetched twice and the cursor keeps meaning what it says.
        loop {
            let candidates = self
                .repository
                .list_active_children(
                    context,
                    &ListEntriesQuery {
                        parent_id: query.parent_id,
                        filter: query.filter.clone(),
                        page: page.clone(),
                    },
                )
                .await?;
            next_cursor = candidates.next_cursor;
            for entry in candidates.items {
                let authorization = entry.authorization(context.authorization());
                let effective_access = authorization.capabilities().effective_access();
                if let Some(expression) = policy_filter {
                    let matches = expression
                        .matches(&effective_access, &entry.database_filter_matches)
                        .ok_or_else(|| {
                            MetadataServiceError::Repository(MetadataRepositoryError::Internal(
                                anyhow::anyhow!(
                                    "database filter projection does not match its expression"
                                ),
                            ))
                        })?;
                    if !matches {
                        continue;
                    }
                }
                if let Some(item) = entry.clone().into_list_item(authorization) {
                    accessed.push(entry.entry.id);
                    items.push(item);
                    if items.len() == wanted {
                        break;
                    }
                }
            }

            let remaining = wanted.saturating_sub(items.len());
            if remaining == 0 {
                if take.is_some() {
                    next_cursor = None;
                }
                break;
            }
            let Some(cursor) = next_cursor.clone() else {
                break;
            };
            // Ordinary cursor listings have a fixed scan budget and return the
            // raw keyset cursor when sparse authorization exhausts it. A
            // chronological take is terminal rather than paginated, so it must
            // continue until N authorized matches or true exhaustion.
            if take.is_none() && refills >= MAX_PAGE_REFILLS {
                break;
            }
            refills += 1;
            let limit = if take.is_some() {
                query.page.limit
            } else {
                let Ok(limit) = u16::try_from(remaining) else {
                    break;
                };
                limit
            };
            page = PageRequest {
                cursor: Some(cursor),
                limit,
            };
        }

        if take.is_some()
            && query
                .filter
                .as_ref()
                .is_some_and(crate::domain::filter::FilterQuery::requires_reversal)
        {
            items.reverse();
            accessed.reverse();
        }

        if !accessed.is_empty() {
            self.repository
                .record_metadata_access(context, &accessed)
                .await?;
        }
        Ok(Page { items, next_cursor })
    }

    async fn validate_listing_parent(
        &self,
        context: &ExecutionContext,
        parent_id: Option<EntryId>,
    ) -> Result<(), MetadataServiceError> {
        let Some(parent_id) = parent_id else {
            return Ok(());
        };
        let parent = self
            .repository
            .find_active_entry(context, parent_id)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        if parent.entry.kind != EntryKind::Folder
            || parent.authorization(context.authorization()).visibility() == EntryVisibility::Hidden
        {
            return Err(MetadataServiceError::NotFound);
        }
        Ok(())
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

        let mut command = command;
        let (boundary, parent_id) = if let Some(parent_id) = command.parent_id {
            let parent = self
                .repository
                .find_active_entry(context, parent_id)
                .await?
                .ok_or(MetadataServiceError::NotFound)?;
            if parent.entry.kind != EntryKind::Folder {
                return Err(MetadataServiceError::NotFound);
            }
            require_capability(&parent, context, Capability::CreateChild)?;
            (parent.entry.boundary, parent_id)
        } else {
            // The organization base holds exactly the reserved containers:
            // Public, Private, and one per tag. Declaring a kind of folder at
            // that level chooses which container it goes into — Public, the
            // caller's own folder inside Private, or that tag's folder — so a
            // member's material always sits somewhere the contract describes.
            let boundary =
                command
                    .root_boundary
                    .clone()
                    .ok_or(MetadataServiceError::Validation(super::ValidationError {
                        field: "root_boundary",
                        message: "is required for a user root",
                    }))?;
            let container = self
                .repository
                .find_boundary_container(context, &boundary)
                .await?
                .ok_or(MetadataServiceError::NotFound)?;
            // A tag folder the caller does not carry is not visible to them,
            // so this reports it exactly as a container that is not there.
            require_capability(&container, context, Capability::CreateChild)?;
            (container.entry.boundary.clone(), container.entry.id)
        };
        command.parent_id = Some(parent_id);
        let required_parent_capability = Some(Capability::CreateChild);

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
    /// A folder that is shared only through its contents resolves as a
    /// traversal view: the caller opens it and sees exactly the entries they
    /// were given. When nothing inside it remains accessible, the folder
    /// answers not found, like anything else the caller may not see.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid,
    /// nothing visible exists at the path, or repository access fails.
    pub async fn get_entry_by_path(
        &self,
        context: &ExecutionContext,
        path: &EntryPath,
    ) -> Result<EntryListItem, MetadataServiceError> {
        validate_context(context)?;
        let entry = self
            .repository
            .find_active_entry_by_path(context, path)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        self.visible_view(context, entry).await
    }

    /// Returns one visible entry by identifier, traversal folders included.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid,
    /// the entry is not visible, or repository access fails.
    pub async fn visible_entry(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
    ) -> Result<EntryListItem, MetadataServiceError> {
        validate_context(context)?;
        let entry = self
            .repository
            .find_active_entry(context, entry_id)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        self.visible_view(context, entry).await
    }

    async fn visible_view(
        &self,
        context: &ExecutionContext,
        entry: super::AuthorizableEntry,
    ) -> Result<EntryListItem, MetadataServiceError> {
        let entry_id = entry.entry.id;
        let authorization = entry.authorization(context.authorization());
        let item = entry
            .into_list_item(authorization)
            .ok_or(MetadataServiceError::NotFound)?;
        self.repository
            .record_metadata_access(context, &[entry_id])
            .await?;
        Ok(item)
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
        record_then_list_activity(
            self.repository.record_metadata_access(context, &[entry_id]),
            || self.repository.list_entry_activity(context, entry_id),
        )
        .await
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

        // Supplying the existing parent is still a rename, not a move. File
        // update authority must not require creating children in its parent.
        if let Some(parent_id) = command
            .parent_id
            .filter(|parent_id| Some(*parent_id) != source.entry.parent_id)
        {
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

async fn record_then_list_activity<RecordFuture, List, ListFuture>(
    record: RecordFuture,
    list: List,
) -> Result<Vec<ActivityEvent>, MetadataServiceError>
where
    RecordFuture: Future<Output = Result<(), MetadataRepositoryError>>,
    List: FnOnce() -> ListFuture,
    ListFuture: Future<Output = Result<Vec<ActivityEvent>, MetadataRepositoryError>>,
{
    record.await?;
    list().await.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::{MetadataRepositoryError, record_then_list_activity};

    #[tokio::test]
    async fn activity_access_precedes_history_load() -> Result<(), super::MetadataServiceError> {
        let recorded = Arc::new(AtomicBool::new(false));
        let record_observation = Arc::clone(&recorded);
        let list_observation = Arc::clone(&recorded);

        let events = record_then_list_activity(
            async move {
                record_observation.store(true, Ordering::SeqCst);
                Ok::<_, MetadataRepositoryError>(())
            },
            move || async move {
                assert!(
                    list_observation.load(Ordering::SeqCst),
                    "the current metadata access must precede the history read"
                );
                Ok::<_, MetadataRepositoryError>(Vec::new())
            },
        )
        .await?;

        assert!(events.is_empty());
        Ok(())
    }
}
