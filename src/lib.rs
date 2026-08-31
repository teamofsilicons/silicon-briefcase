//! Silicon Briefcase backend library.
//!
//! Briefcase is an organization-scoped filesystem. Domain policy remains
//! independent from HTTP, PostgreSQL, IAM, and S3 adapters so each boundary can
//! be tested and evolved without weakening tenant or authorization invariants.

#![forbid(unsafe_code)]
#![deny(clippy::dbg_macro)]
#![deny(clippy::expect_used)]
#![deny(clippy::todo)]
#![deny(clippy::unimplemented)]
#![deny(clippy::unwrap_used)]

pub mod api;
pub mod application;
pub mod config;
pub mod domain;
pub mod error;
pub mod infrastructure;
pub mod request_context;
pub mod shutdown;
pub mod telemetry;
pub mod worker;
