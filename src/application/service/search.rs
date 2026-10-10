//! Permission-safe filename and extracted-content search.

use crate::{application::context::ExecutionContext, domain::permission::Capability};

use super::{
    MetadataService, MetadataServiceError, SearchQuery, SearchResultView, require_capability,
    validate_context,
};

impl MetadataService {
    /// Searches visible files with query-time authorization defense in depth.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid or
    /// the repository cannot search or audit the visible results.
    pub async fn search(
        &self,
        context: &ExecutionContext,
        query: &SearchQuery,
    ) -> Result<Vec<SearchResultView>, MetadataServiceError> {
        validate_context(context)?;
        let candidates = self.repository.search(context, query).await?;
        let mut results = Vec::with_capacity(usize::from(query.limit));
        let mut accessed = Vec::with_capacity(candidates.len());

        for candidate in candidates {
            if results.len() >= usize::from(query.limit) {
                break;
            }
            let Ok(authorization) = require_capability(&candidate.entry, context, Capability::Read)
            else {
                continue;
            };
            accessed.push(candidate.entry.entry.id);
            results.push(SearchResultView {
                entry: candidate.entry.into_full_view(authorization)?,
                score: candidate.score,
                filename_match: candidate.filename_match,
                content_hits: candidate.content_hits,
                snippets: candidate.snippets,
            });
        }

        if !accessed.is_empty() {
            self.repository
                .record_metadata_access(context, &accessed)
                .await?;
        }
        Ok(results)
    }

    /// Searches several drives, each under its own authorization, and
    /// returns the best results across them: filename matches first, then
    /// content hits, then score.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when a request context is invalid or
    /// the repository cannot search or audit the visible results.
    pub async fn search_drives(
        &self,
        contexts: &[ExecutionContext],
        query: &SearchQuery,
    ) -> Result<Vec<SearchResultView>, MetadataServiceError> {
        let mut found: Vec<(usize, SearchResultView)> = Vec::new();
        for (index, context) in contexts.iter().enumerate() {
            validate_context(context)?;
            for candidate in self.repository.search(context, query).await? {
                let Ok(authorization) =
                    require_capability(&candidate.entry, context, Capability::Read)
                else {
                    continue;
                };
                found.push((
                    index,
                    SearchResultView {
                        entry: candidate.entry.into_full_view(authorization)?,
                        score: candidate.score,
                        filename_match: candidate.filename_match,
                        content_hits: candidate.content_hits,
                        snippets: candidate.snippets,
                    },
                ));
            }
        }
        found.sort_by(|(_, left), (_, right)| {
            right
                .filename_match
                .cmp(&left.filename_match)
                .then(right.content_hits.cmp(&left.content_hits))
                .then(right.score.total_cmp(&left.score))
                .then(
                    left.entry
                        .entry
                        .id
                        .as_uuid()
                        .cmp(&right.entry.entry.id.as_uuid()),
                )
        });
        found.truncate(usize::from(query.limit));
        // Each drive records the accesses its own results produced.
        for (index, context) in contexts.iter().enumerate() {
            let accessed: Vec<_> = found
                .iter()
                .filter(|(origin, _)| *origin == index)
                .map(|(_, result)| result.entry.entry.id)
                .collect();
            if !accessed.is_empty() {
                self.repository
                    .record_metadata_access(context, &accessed)
                    .await?;
            }
        }
        Ok(found.into_iter().map(|(_, result)| result).collect())
    }
}
