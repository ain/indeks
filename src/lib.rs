//! `indeks` submits URLs and sitemaps to search engines for indexing.
//!
//! The binary is a thin wrapper around this library so that every stage —
//! argument parsing, validation, sitemap parsing, submission, reporting — is
//! unit-testable on its own.

/// The service-account file `build.rs` generates for tests, with a real RSA key
/// that belongs to no account. Integration tests build the same path themselves.
#[cfg(test)]
pub(crate) const GENERATED_SERVICE_ACCOUNT: &str =
    concat!(env!("OUT_DIR"), "/service-account.json");

pub mod cli;
pub mod credentials;
pub mod engine;
pub mod error;
pub mod http;
pub mod report;
pub mod sitemap;
pub mod targets;
pub mod validate;
