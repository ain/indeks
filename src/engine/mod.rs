//! Search engine backends.

pub mod google;
pub mod indexnow;

use reqwest::blocking::Client;
use url::Url;

use crate::credentials::Credential;
use crate::error::Result;
use crate::validate::ValidationError;

/// Which search engine a run targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Google,
    Bing,
}

impl Kind {
    /// Name used in output.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Google => google::NAME,
            Kind::Bing => indexnow::NAME,
        }
    }

    /// Check that a credential is usable for this engine.
    ///
    /// Called during validation, so that an unusable key fails the run before
    /// any sitemap is fetched or any URL submitted.
    pub fn check_credential(
        self,
        credential: &Credential,
    ) -> std::result::Result<(), ValidationError> {
        match self {
            Kind::Bing => indexnow::key(credential).map(|_| ()),
            Kind::Google => google::check_credential(credential),
        }
    }

    /// Build the submitter for this engine.
    pub fn build(
        self,
        credential: &Credential,
        client: Client,
    ) -> std::result::Result<Box<dyn Submitter>, ValidationError> {
        match self {
            Kind::Bing => Ok(Box::new(indexnow::IndexNow::new(
                indexnow::key(credential)?,
                client,
            ))),
            Kind::Google => Ok(Box::new(google::Google::new(credential.clone(), client))),
        }
    }
}

/// The result of submitting one URL.
///
/// Batching backends such as IndexNow submit many URLs in a single request; the
/// response status is then recorded against every URL in that batch.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub url: Url,
    pub status: u16,
    /// The error text from the response, or `None` when the URL was accepted.
    pub error: Option<String>,
}

impl Outcome {
    pub fn succeeded(&self) -> bool {
        self.error.is_none()
    }
}

pub trait Submitter {
    /// Name used in output, e.g. `Google Indexing API`.
    fn name(&self) -> &'static str;

    /// Submit every URL, returning one outcome per URL.
    ///
    /// Returns `Err` only when the exchange itself failed — a transport error,
    /// or credentials that could not be turned into a token. URLs rejected by
    /// the API come back as failed outcomes.
    fn submit(&self, urls: &[Url]) -> Result<Vec<Outcome>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn bing_refuses_a_key_that_indexnow_would_reject() {
        let error = Kind::Bing
            .check_credential(&Credential::Token("short".to_string()))
            .unwrap_err();
        assert!(error.to_string().contains("8-128 characters"), "{error}");
    }

    #[test]
    fn bing_accepts_a_well_formed_key() {
        let credential = Credential::Token("abcdef0123456789".to_string());
        assert!(Kind::Bing.check_credential(&credential).is_ok());
    }

    #[test]
    fn google_refuses_a_file_that_is_not_a_service_account() {
        let credential = Credential::File(PathBuf::from("tests/fixtures/indexnow-key.json"));
        let error = Kind::Google.check_credential(&credential).unwrap_err();
        assert!(error.to_string().contains("service-account"), "{error}");
    }

    #[test]
    fn google_accepts_a_service_account_file() {
        let credential = Credential::File(PathBuf::from(crate::GENERATED_SERVICE_ACCOUNT));
        assert!(Kind::Google.check_credential(&credential).is_ok());
    }

    #[test]
    fn each_engine_names_itself() {
        assert_eq!(Kind::Bing.name(), indexnow::NAME);
        assert_eq!(Kind::Google.name(), google::NAME);
    }
}
