//! Listing archives through the router: zip, tar.gz and 7z files read from
//! an object store in memory, with the caller's read access deciding.
//!
//! ```bash
//! BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5460/briefcase_mig \
//!   cargo test --lib api::archive_tests -- --test-threads=1
//! ```

use std::{io::Write as _, sync::Arc};

use axum::http::{Method, StatusCode};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::{
    drive_access_tests::in_drive,
    drives_tests::{World, code},
    memory_store::MemoryStore,
};
use crate::{
    application::{
        content::{ContentRepository as _, Prepared, SmallUploadCommand, StagedContent},
        context::ExecutionContext,
        idempotency::IdempotencyKey,
        ports::{ObjectChecksum, ObjectChecksumAlgorithm, ObjectChecksumType, StoredObject},
    },
    domain::{entry::EntryName, ids::EntryId},
};

/// Publishes a file whose bytes the memory store then serves.
async fn publish(
    w: &World,
    store: &MemoryStore,
    context: &ExecutionContext,
    parent: &str,
    name: &str,
    content_type: &str,
    bytes: &[u8],
) -> anyhow::Result<String> {
    let staged = std::env::temp_dir().join(format!("briefcase-archive-{}", Uuid::now_v7()));
    tokio::fs::write(&staged, bytes).await?;
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let payload = StagedContent {
        path: staged.as_path(),
        offset: 0,
        size: bytes.len() as u64,
        sha256: digest,
    };
    let command = SmallUploadCommand {
        parent_id: EntryId::from_uuid(Uuid::parse_str(parent)?)?,
        name: EntryName::new(name)?,
        content_type: content_type.to_owned(),
        idempotency_key: IdempotencyKey::new(Uuid::now_v7().to_string())?,
        request_hash: [9; 32],
        self_destruct: None,
    };
    let repository = &w.f.state.content_adapter;
    let Prepared::Acquired(preparation) = repository
        .prepare_small_upload(context, &command, &payload)
        .await
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
    else {
        anyhow::bail!("a fresh upload is never a replay");
    };
    store.insert(&preparation.key, bytes.to_vec());
    let stored = StoredObject {
        key: preparation.key.clone(),
        etag: None,
        provider_version_id: None,
        size: payload.size,
        checksum: Some(ObjectChecksum::new(
            ObjectChecksumAlgorithm::Sha256,
            ObjectChecksumType::FullObject,
            STANDARD.encode(digest),
        )?),
    };
    let entry = repository
        .commit_small_upload(context, &command, &payload, &preparation, &stored)
        .await
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    tokio::fs::remove_file(&staged).await?;
    Ok(entry.to_string())
}

fn paths(listing: &Value) -> Vec<String> {
    listing["entries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["path"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn zip_bytes() -> anyhow::Result<Vec<u8>> {
    use zip::{CompressionMethod, write::SimpleFileOptions};
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.add_directory("site/", options)?;
    writer.start_file("site/index.html", options)?;
    writer.write_all(b"<h1>hello</h1>")?;
    writer.start_file("site/style.css", options)?;
    writer.write_all(b"h1 {}")?;
    Ok(writer.finish()?.into_inner())
}

fn tar_gz_bytes() -> anyhow::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o644);
    builder.append_data(&mut header, "logs/today.log", &b"ok\n\n"[..])?;
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(&builder.into_inner()?)?;
    Ok(gzip.finish()?)
}

fn seven_zip_bytes() -> anyhow::Result<Vec<u8>> {
    let source = tempfile::tempdir()?;
    std::fs::write(source.path().join("track.wav"), b"RIFF....")?;
    let target = tempfile::tempdir()?;
    let archive = target.path().join("takes.7z");
    sevenz_rust2::compress_to_path(source.path(), &archive)?;
    Ok(std::fs::read(archive)?)
}

#[tokio::test]
async fn archives_list_their_contents_for_whoever_may_read_them() -> anyhow::Result<()> {
    let store = MemoryStore::default();
    let Some(w) = World::with_store(Arc::new(store.clone())).await? else {
        return Ok(());
    };
    let folder = w.folder(&w.ada, "bundles", None, None).await?;
    let own = in_drive(&w, &w.ada, &w.ada).await?;
    let zip = publish(
        &w,
        &store,
        &own,
        &folder,
        "site.zip",
        "application/zip",
        &zip_bytes()?,
    )
    .await?;
    let logs = publish(
        &w,
        &store,
        &own,
        &folder,
        "logs.tar.gz",
        "application/gzip",
        &tar_gz_bytes()?,
    )
    .await?;
    let takes = publish(
        &w,
        &store,
        &own,
        &folder,
        "takes.7z",
        "application/x-7z-compressed",
        &seven_zip_bytes()?,
    )
    .await?;
    let notes = publish(
        &w,
        &store,
        &own,
        &folder,
        "notes.md",
        "text/markdown",
        b"# Notes",
    )
    .await?;
    let listing = |entry: &str| format!("/api/v1/entries/{entry}/archive");

    for (entry, format, expected) in [
        (
            &zip,
            "zip",
            vec!["site/", "site/index.html", "site/style.css"],
        ),
        (&logs, "tar.gz", vec!["logs/today.log"]),
        (&takes, "7z", vec!["track.wav"]),
    ] {
        let (status, body) = w.call(&w.ada, Method::GET, &listing(entry), None).await?;
        assert_eq!(status, StatusCode::OK, "{format}: {body}");
        assert_eq!(body["format"], format);
        assert_eq!(body["truncated"], false);
        assert_eq!(paths(&body), expected, "{format}");
    }
    let (status, body) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("{}?limit=1", listing(&zip)),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(paths(&body), vec!["site/"]);
    assert_eq!(body["truncated"], true);
    assert_eq!(body["truncated_because"], "entry_limit");

    for (entry, expected) in [(&notes, "not_an_archive"), (&folder, "not_an_archive")] {
        let (status, body) = w.call(&w.ada, Method::GET, &listing(entry), None).await?;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(code(&body), expected);
    }
    let (status, body) = w
        .call(
            &w.ada,
            Method::GET,
            &format!("{}?limit=0", listing(&zip)),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    // Someone who may not read the file learns nothing; a share opens it.
    let (status, _) = w.call(&w.bob, Method::GET, &listing(&zip), None).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = w
        .share(&w.ada, &folder, "carbon", &w.bob.id, &["read"])
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = w.call(&w.bob, Method::GET, &listing(&zip), None).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(paths(&body).len(), 3);
    Ok(())
}
