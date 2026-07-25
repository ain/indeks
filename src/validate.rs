//! Input checks. Everything here runs before the first network call.
//!
//! Checks collect into [`ValidationErrors`] rather than returning on the first
//! failure, so a single run tells the user everything that is wrong.

use std::fmt;

use url::Url;

use crate::cli::SubmissionArgs;
use crate::credentials::Credential;
use crate::engine::Kind;
use crate::sitemap::Source;

/// A single rejected input.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// A `--url` or `--sitemap` value that is not a usable target.
    #[error("--{flag} {value}: {reason}")]
    Argument {
        flag: &'static str,
        value: String,
        reason: String,
    },

    /// Neither `--url` nor `--sitemap` was given.
    #[error("at least one --url or --sitemap is required")]
    NoTargets,

    /// `--credentials` was not given.
    #[error("--credentials is required")]
    MissingCredentials,

    /// `--credentials` was given but is unusable.
    #[error("--credentials {value}: {reason}")]
    Credentials { value: String, reason: String },
}

/// Build an [`ValidationError::Argument`] for a rejected flag value.
pub(crate) fn argument(
    flag: &'static str,
    value: &str,
    reason: impl Into<String>,
) -> ValidationError {
    ValidationError::Argument {
        flag,
        value: value.to_string(),
        reason: reason.into(),
    }
}

/// Every problem found in one pass over the arguments.
#[derive(Debug)]
pub struct ValidationErrors {
    pub errors: Vec<ValidationError>,
    /// Whether to close with the `--dry-run` recommendation. Suppressed when
    /// the failing run already used `--dry-run`.
    pub advise_dry_run: bool,
}

pub const DRY_RUN_HINT: &str =
    "Consider --dry-run to validate input without contacting external systems.";

impl fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.errors.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            write!(f, "error: {error}")?;
        }
        if self.advise_dry_run {
            write!(f, "\n\n{DRY_RUN_HINT}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationErrors {}

/// Arguments that passed validation, with every value already parsed.
#[derive(Debug)]
pub struct Validated {
    pub urls: Vec<Url>,
    pub sitemaps: Vec<Source>,
    pub credential: Credential,
    pub dry_run: bool,
    pub verbose: bool,
}

/// Check every argument, reporting all problems at once.
///
/// `--credentials` is required even under `--dry-run`: a dry run is a pre-flight
/// check, and it is worth little if it passes on input the real run would reject.
/// Checking a credentials file means reading it and parsing its JSON, which
/// contacts no external system.
pub fn validate(args: &SubmissionArgs, engine: Kind) -> Result<Validated, ValidationErrors> {
    let mut errors = Vec::new();

    let mut urls = Vec::new();
    for raw in &args.urls {
        match absolute_url("url", raw) {
            Ok(url) => urls.push(url),
            Err(error) => errors.push(error),
        }
    }

    let mut sitemaps = Vec::new();
    for raw in &args.sitemaps {
        match Source::classify(raw) {
            Ok(source) => sitemaps.push(source),
            Err(error) => errors.push(error),
        }
    }

    if args.urls.is_empty() && args.sitemaps.is_empty() {
        errors.push(ValidationError::NoTargets);
    }

    let credential = match &args.credentials {
        Some(raw) => match Credential::parse(raw).and_then(|credential| {
            // The engine has the last word: an IndexNow key has a shape, and a
            // Google service-account file has required fields.
            engine.check_credential(&credential)?;
            Ok(credential)
        }) {
            Ok(credential) => Some(credential),
            Err(error) => {
                errors.push(error);
                None
            }
        },
        None => {
            errors.push(ValidationError::MissingCredentials);
            None
        }
    };

    match credential {
        Some(credential) if errors.is_empty() => Ok(Validated {
            urls,
            sitemaps,
            credential,
            dry_run: args.dry_run,
            verbose: args.verbose,
        }),
        _ => Err(ValidationErrors {
            errors,
            advise_dry_run: !args.dry_run,
        }),
    }
}

/// Parse a value that must be an absolute http(s) URL.
///
/// There is no separate host check: `http` and `https` are special schemes, so
/// `Url::parse` already rejects a missing host with `EmptyHost`.
pub fn absolute_url(flag: &'static str, value: &str) -> Result<Url, ValidationError> {
    let url = Url::parse(value).map_err(|error| match error {
        url::ParseError::RelativeUrlWithoutBase => argument(flag, value, "not an absolute URL"),
        other => argument(flag, value, other.to_string()),
    })?;

    match url.scheme() {
        "http" | "https" => Ok(url),
        scheme => Err(argument(
            flag,
            value,
            format!("scheme `{scheme}` is not supported; use http or https"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key that satisfies IndexNow's own format rules, so that these tests
    /// exercise argument handling rather than the engine's credential check.
    const KEY: &str = "abcdef0123456789";

    fn args(urls: &[&str], sitemaps: &[&str], credentials: Option<&str>) -> SubmissionArgs {
        SubmissionArgs {
            urls: urls.iter().map(|value| value.to_string()).collect(),
            sitemaps: sitemaps.iter().map(|value| value.to_string()).collect(),
            credentials: credentials.map(str::to_string),
            dry_run: false,
            verbose: false,
        }
    }

    fn validate_bing(args: &SubmissionArgs) -> Result<Validated, ValidationErrors> {
        validate(args, Kind::Bing)
    }

    #[test]
    fn accepts_absolute_http_urls() {
        for value in [
            "https://example.com/a",
            "http://example.com",
            "https://x.io/",
        ] {
            assert!(absolute_url("url", value).is_ok(), "rejected {value}");
        }
    }

    #[test]
    fn rejects_relative_url() {
        let error = absolute_url("url", "/page").unwrap_err();
        assert_eq!(error.to_string(), "--url /page: not an absolute URL");
    }

    #[test]
    fn rejects_unsupported_scheme() {
        let error = absolute_url("url", "ftp://example.com/a").unwrap_err();
        assert!(error.to_string().contains("scheme `ftp` is not supported"));
    }

    #[test]
    fn rejects_url_without_host() {
        let error = absolute_url("url", "https://").unwrap_err();
        assert_eq!(error.to_string(), "--url https://: empty host");
    }

    #[test]
    fn accepts_a_valid_run() {
        let validated = validate_bing(&args(&["https://example.com/a"], &[], Some(KEY))).unwrap();

        assert_eq!(validated.urls.len(), 1);
        assert_eq!(validated.credential, Credential::Token(KEY.to_string()));
    }

    #[test]
    fn requires_at_least_one_target() {
        let errors = validate_bing(&args(&[], &[], Some(KEY))).unwrap_err();
        assert_eq!(errors.errors.len(), 1);
        assert!(matches!(errors.errors[0], ValidationError::NoTargets));
    }

    #[test]
    fn requires_credentials_even_when_dry_running() {
        let mut args = args(&["https://example.com/a"], &[], None);
        args.dry_run = true;
        let errors = validate_bing(&args).unwrap_err();
        assert!(matches!(
            errors.errors[0],
            ValidationError::MissingCredentials
        ));
    }

    #[test]
    fn reports_every_problem_at_once() {
        let errors =
            validate_bing(&args(&["/relative", "ftp://example.com"], &[], None)).unwrap_err();
        assert_eq!(errors.errors.len(), 3, "{:?}", errors.errors);
    }

    #[test]
    fn advises_dry_run_unless_already_dry_running() {
        let errors = validate_bing(&args(&[], &[], Some(KEY))).unwrap_err();
        assert!(errors.to_string().contains(DRY_RUN_HINT));

        let mut dry = args(&[], &[], Some(KEY));
        dry.dry_run = true;
        let errors = validate_bing(&dry).unwrap_err();
        assert!(!errors.to_string().contains(DRY_RUN_HINT));
    }

    #[test]
    fn renders_one_error_per_line() {
        let errors = validate_bing(&args(&["/a", "/b"], &[], Some(KEY))).unwrap_err();
        let rendered = errors.to_string();
        let lines: Vec<_> = rendered
            .lines()
            .filter(|l| l.starts_with("error:"))
            .collect();
        assert_eq!(lines.len(), 2, "{rendered}");
    }
}
