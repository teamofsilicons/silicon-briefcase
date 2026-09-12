//! Backpressured tar.zst delivery using the ordinary per-entry access policy.

use std::io::{self, Write};

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, header},
    response::Response,
};
use tokio::sync::mpsc;
use tokio_util::io::{StreamReader, SyncIoBridge};

use super::super::{mapping::metadata_error, state::AppState};
use crate::{
    application::{
        content::ContentIntent,
        context::ExecutionContext,
        service::{EntryListItem, ListEntriesQuery, PageRequest},
    },
    domain::ids::EntryId,
    error::AppError,
};

/// A bounded output channel also makes a disconnected download stop compression.
static ARCHIVE_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

struct Output(mpsc::Sender<io::Result<Bytes>>);

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for chunk in bytes.chunks(64 * 1024) {
            self.0
                .blocking_send(Ok(Bytes::copy_from_slice(chunk)))
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "download closed"))?;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) async fn serve(
    state: &AppState,
    context: &ExecutionContext,
    root: EntryId,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    if headers.contains_key(header::RANGE) {
        return Err(AppError::bad_request("folder_range_unsupported"));
    }
    let entry = state
        .metadata
        .visible_entry(context, root)
        .await
        .map_err(metadata_error)?;
    if !entry.is_folder() {
        return Err(AppError::NotFound);
    }
    let (name, _) = location(&entry);
    let filename = format!("{}.tar.zst", name.rsplit('/').next().unwrap_or("folder"));
    let disposition = format!(
        "attachment; filename=\"folder.tar.zst\"; filename*=UTF-8''{}",
        percent_encoding::utf8_percent_encode(&filename, percent_encoding::NON_ALPHANUMERIC)
    );
    let permit = ARCHIVE_SLOTS
        .try_acquire()
        .map_err(|_| AppError::RateLimited {
            retry_after_seconds: 2,
        })?;
    let (tx, rx) = mpsc::channel(4);
    let state = state.clone();
    let context = context.clone();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let result = build(&state, &context, root, &runtime, &tx);
        if let Err(error) = result {
            tracing::warn!(error = %error, "folder download interrupted");
            let _ = tx.blocking_send(Err(io::Error::other("folder download interrupted")));
        }
    });
    let stream = futures::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|chunk| (chunk, rx))
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "application/zstd")
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CACHE_CONTROL, "private, no-store")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "sandbox; default-src 'none'",
        )
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from_stream(stream))
        .map_err(|_| AppError::Internal {
            category: "archive_response",
        })
}

fn build(
    state: &AppState,
    context: &ExecutionContext,
    root: EntryId,
    runtime: &tokio::runtime::Handle,
    tx: &mpsc::Sender<io::Result<Bytes>>,
) -> anyhow::Result<()> {
    let encoder = zstd::stream::write::Encoder::new(Output(tx.clone()), 3)?;
    let mut archive = tar::Builder::new(encoder);
    // Keep a stack of folder cursors, not a manifest of every file in the tree.
    let mut pending = vec![(root, String::new(), None)];
    while let Some((parent, prefix, cursor)) = pending.pop() {
        if tx.is_closed() {
            return Ok(());
        }
        let page = runtime.block_on(state.metadata.list_entries(
            context,
            &ListEntriesQuery {
                parent_id: Some(parent),
                filter: None,
                page: PageRequest::new(cursor, 100)?,
            },
        ))?;
        if let Some(cursor) = page.next_cursor {
            pending.push((parent, prefix.clone(), Some(cursor)));
        }
        for item in page.items {
            let (path, size) = location(&item);
            let name = path.rsplit('/').next().unwrap_or(path);
            // Entry names are validated by the domain; backslashes are rejected
            // too so archives remain safe for Windows extractors.
            if name.contains('\\') {
                anyhow::bail!("unsafe archive entry name");
            }
            let relative = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            let mut header = tar::Header::new_gnu();
            header.set_mtime(0);
            header.set_mode(if item.is_folder() { 0o755 } else { 0o644 });
            if item.is_folder() {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_cksum();
                archive.append_data(&mut header, &relative, io::empty())?;
                pending.push((item.id(), relative, None));
            } else {
                // Rechecks permission and current content for each file; content
                // is never fetched via a provider URL or buffered as a whole.
                let delivery = runtime.block_on(state.content.open_content(
                    context,
                    item.id(),
                    ContentIntent::Download,
                    None,
                ))?;
                let _ = size;
                header.set_size(delivery.total_size);
                header.set_cksum();
                let reader = SyncIoBridge::new_with_handle(
                    StreamReader::new(delivery.body),
                    runtime.clone(),
                );
                archive.append_data(&mut header, &relative, reader)?;
            }
        }
    }
    archive.into_inner()?.finish()?;
    Ok(())
}

fn location(item: &EntryListItem) -> (&str, Option<u64>) {
    match item {
        EntryListItem::Full(view) => (view.entry.path.as_str(), view.entry.size),
        EntryListItem::Traversal(view) => (view.path.as_str(), None),
    }
}

pub(crate) async fn serve_public(
    state: &AppState,
    tenant: crate::infrastructure::postgres::TenantContext,
    path: String,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    if headers.contains_key(header::RANGE) {
        return Err(AppError::bad_request("folder_range_unsupported"));
    }
    let entry = state
        .content_adapter
        .metadata_repository()
        .public_entry(&tenant, &path)
        .await?;
    if entry.entry_type != "folder" {
        return Err(AppError::NotFound);
    }
    let permit = ARCHIVE_SLOTS
        .try_acquire()
        .map_err(|_| AppError::RateLimited {
            retry_after_seconds: 2,
        })?;
    let (tx, rx) = mpsc::channel(4);
    let state = state.clone();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let result = build_public(&state, &tenant, path, &runtime, &tx);
        if result.is_err() {
            let _ = tx.blocking_send(Err(io::Error::other("folder download interrupted")));
        }
    });
    let stream = futures::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|chunk| (chunk, rx))
    });
    let filename = format!("{}.tar.zst", entry.name);
    Response::builder()
        .header(header::CONTENT_TYPE, "application/zstd")
        .header(
            header::CONTENT_DISPOSITION,
            format!(
                "attachment; filename=\"folder.tar.zst\"; filename*=UTF-8''{}",
                percent_encoding::utf8_percent_encode(
                    &filename,
                    percent_encoding::NON_ALPHANUMERIC
                )
            ),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from_stream(stream))
        .map_err(|_| AppError::Internal {
            category: "archive_response",
        })
}

fn build_public(
    state: &AppState,
    tenant: &crate::infrastructure::postgres::TenantContext,
    path: String,
    runtime: &tokio::runtime::Handle,
    tx: &mpsc::Sender<io::Result<Bytes>>,
) -> anyhow::Result<()> {
    let encoder = zstd::stream::write::Encoder::new(Output(tx.clone()), 3)?;
    let mut archive = tar::Builder::new(encoder);
    let root = format!("{path}/");
    let mut pending = vec![(path, None)];
    while let Some((path, cursor)) = pending.pop() {
        if tx.is_closed() {
            return Ok(());
        }
        let page = runtime.block_on(
            state
                .content_adapter
                .metadata_repository()
                .public_children(tenant, &path, cursor),
        )?;
        if let Some(cursor) = page.next_cursor {
            pending.push((path, Some(cursor)));
        }
        for entry in page.items {
            let relative = entry
                .path
                .strip_prefix(&root)
                .ok_or_else(|| anyhow::anyhow!("archive path changed"))?;
            if relative.contains('\\') {
                anyhow::bail!("unsafe archive entry name");
            }
            let mut header = tar::Header::new_gnu();
            header.set_mtime(0);
            if entry.entry_type == "folder" {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(0o755);
                header.set_size(0);
                header.set_cksum();
                archive.append_data(&mut header, relative, io::empty())?;
                pending.push((entry.path, None));
            } else {
                let delivery = runtime.block_on(super::sharing::open_public(
                    state,
                    tenant,
                    &entry.path,
                    None,
                ))?;
                header.set_mode(0o644);
                header.set_size(delivery.total_size);
                header.set_cksum();
                archive.append_data(
                    &mut header,
                    relative,
                    SyncIoBridge::new_with_handle(
                        StreamReader::new(delivery.body),
                        runtime.clone(),
                    ),
                )?;
            }
        }
    }
    archive.into_inner()?.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::{
            context::ExecutionContext,
            idempotency::IdempotencyKey,
            service::{CreateFolderCommand, MutationMetadata},
        },
        domain::{
            actor::{
                ActorId, ActorKind, ActorRef, AuthenticationMode, OrganizationId, OrganizationRole,
                RequestAuthContext,
            },
            entry::{EntryName, EntryPath},
        },
    };
    use uuid::Uuid;

    #[tokio::test]
    async fn folder_archive_is_decodable_and_confined_to_requested_subtree() -> anyhow::Result<()> {
        let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
            return Ok(());
        };
        let pool = sqlx::PgPool::connect_with(
            url.parse::<sqlx::postgres::PgConnectOptions>()?
                .options([("search_path", "public")]),
        )
        .await?;
        crate::infrastructure::postgres::migrate(&pool).await?;
        let (state, _, _) = crate::api::tests::test_state(pool.clone())?;
        let context = ExecutionContext::new(
            RequestAuthContext::new(
                OrganizationId::new(format!("archive-{}", Uuid::new_v4().simple()))?,
                ActorRef::new(ActorKind::Carbon, ActorId::new("reader:tos")?),
                OrganizationRole::Member,
                vec![],
                AuthenticationMode::Bearer,
            ),
            "archive-test",
        );
        let private = state
            .metadata
            .get_entry_by_path(&context, &EntryPath::new("private/reader:tos")?)
            .await?;
        for name in ["visible", "outside-the-download"] {
            state
                .metadata
                .create_folder(
                    &context,
                    CreateFolderCommand::new(
                        EntryName::new(name)?,
                        Some(private.id()),
                        None,
                        vec![],
                    )?,
                    &MutationMetadata::new(
                        Some(IdempotencyKey::new(Uuid::new_v4().to_string())?),
                        [0; 32],
                    ),
                )
                .await?;
        }
        let visible = state
            .metadata
            .get_entry_by_path(&context, &EntryPath::new("private/reader:tos/visible")?)
            .await?;
        state
            .metadata
            .create_folder(
                &context,
                CreateFolderCommand::new(
                    EntryName::new("empty ✓")?,
                    Some(visible.id()),
                    None,
                    vec![],
                )?,
                &MutationMetadata::new(
                    Some(IdempotencyKey::new(Uuid::new_v4().to_string())?),
                    [1; 32],
                ),
            )
            .await?;
        let response = serve(&state, &context, visible.id(), &HeaderMap::new()).await?;
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/zstd");
        assert!(
            response.headers()[header::CONTENT_DISPOSITION]
                .to_str()?
                .contains("tar%2Ezst")
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await?;
        let decoded = zstd::stream::decode_all(bytes.as_ref())?;
        let mut archive = tar::Archive::new(decoded.as_slice());
        let mut paths = Vec::new();
        for item in archive.entries()? {
            let item = item?;
            assert!(item.header().entry_type().is_dir());
            paths.push(item.path()?.into_owned());
        }
        assert_eq!(paths, vec![std::path::PathBuf::from("empty ✓")]);
        let mut range = HeaderMap::new();
        range.insert(header::RANGE, "bytes=0-10".parse()?);
        assert!(serve(&state, &context, visible.id(), &range).await.is_err());
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn abandoned_download_stops_the_blocking_writer() -> anyhow::Result<()> {
        let (tx, mut rx) = mpsc::channel(1);
        let writer =
            tokio::task::spawn_blocking(move || Output(tx).write_all(&vec![7; 1024 * 1024]));
        let first = rx
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("missing archive chunk"))??;
        assert_eq!(first.len(), 64 * 1024);
        drop(rx);
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), writer).await??;
        assert_eq!(
            result.err().map(|e| e.kind()),
            Some(io::ErrorKind::BrokenPipe)
        );
        Ok(())
    }
}
