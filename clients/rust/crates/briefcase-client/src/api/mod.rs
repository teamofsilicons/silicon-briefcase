//! The contracted operations, grouped by what they are for.
//!
//! Every operation is a method on [`Client`](crate::Client); these modules
//! only decide which file each one lives in.

mod access;
mod content;
mod entries;
mod environments;
mod login;
mod organization;

pub use content::ContentStream;
