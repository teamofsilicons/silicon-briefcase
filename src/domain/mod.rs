//! Pure domain types and authorization policy.
//!
//! Types in this module deliberately have no knowledge of HTTP, PostgreSQL,
//! IAM wire formats, or S3 clients. Adapters translate external data into these
//! types only after authenticating it and validating its source.

pub mod access;
pub mod actor;
pub mod entry;
pub mod filter;
pub mod ids;
pub mod media;
pub mod multipart;
pub mod notification;
pub mod permission;
pub mod storage;
pub mod version;
