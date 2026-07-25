//! Top-level error type and the exit codes it maps to.

use std::process::ExitCode;

use crate::sitemap::SitemapError;
use crate::validate::{ValidationError, ValidationErrors};

pub type Result<T> = std::result::Result<T, Error>;

/// Exit code for a run that could not submit anything because the input was
/// rejected, or because the arguments did not make sense.
pub const EXIT_USAGE: u8 = 2;

/// Exit code for a run whose input was fine but whose submission failed.
pub const EXIT_SUBMISSION_FAILED: u8 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Input was rejected before any network call was made.
    #[error("{0}")]
    Usage(#[from] ValidationErrors),

    /// A sitemap could not be read or parsed. This is an input problem like any
    /// other, and is caught before anything is submitted.
    #[error("error: {0}")]
    Sitemap(#[from] SitemapError),

    /// At least one URL was refused by the search engine.
    #[error("{0}")]
    Submission(String),

    /// The request never completed: connection, TLS or decoding failure.
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
}

impl From<ValidationError> for Error {
    /// A lone input problem found outside the validation pass, such as when an
    /// engine is built.
    fn from(error: ValidationError) -> Self {
        Error::Usage(ValidationErrors {
            errors: vec![error],
            advise_dry_run: true,
        })
    }
}

impl Error {
    /// The process exit code this error should produce.
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Error::Usage(_) | Error::Sitemap(_) => ExitCode::from(EXIT_USAGE),
            Error::Submission(_) | Error::Network(_) => ExitCode::from(EXIT_SUBMISSION_FAILED),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sitemap::SitemapError;

    fn code(error: Error) -> ExitCode {
        error.exit_code()
    }

    #[test]
    fn input_problems_exit_two() {
        let sitemap = SitemapError::Empty {
            source_name: "test".to_string(),
        };
        assert_eq!(
            format!("{:?}", code(Error::Sitemap(sitemap))),
            format!("{:?}", ExitCode::from(EXIT_USAGE))
        );

        let usage = Error::from(ValidationError::NoTargets);
        assert_eq!(
            format!("{:?}", code(usage)),
            format!("{:?}", ExitCode::from(EXIT_USAGE))
        );
    }

    #[test]
    fn a_failed_submission_exits_one() {
        assert_eq!(
            format!("{:?}", code(Error::Submission("refused".to_string()))),
            format!("{:?}", ExitCode::from(EXIT_SUBMISSION_FAILED))
        );
    }

    #[test]
    fn a_lone_validation_error_still_advises_dry_run() {
        let error = Error::from(ValidationError::NoTargets);
        assert!(
            error.to_string().contains(crate::validate::DRY_RUN_HINT),
            "{error}"
        );
    }
}
