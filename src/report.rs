//! Output for submission results and dry runs.

use crate::credentials::Credential;
use crate::engine::Outcome;
use crate::error::{Error, Result};
use crate::sitemap::{Preview, Source};
use crate::validate::Validated;

/// Shown after a failure, but only when the failing run was not already verbose.
pub const VERBOSE_HINT: &str = "Please consider using `--verbose` to find out more";

/// Print one line per URL, then fail the run if anything was refused.
///
/// Successes report the status code and `successfully submitted {URL}`.
/// Failures report the status code and the error text from the response.
pub fn report(outcomes: &[Outcome], verbose: bool) -> Result<()> {
    for line in lines(outcomes) {
        println!("{line}");
    }

    match summary(outcomes, verbose) {
        Some(summary) => Err(Error::Submission(summary)),
        None => Ok(()),
    }
}

/// One output line per outcome.
pub fn lines(outcomes: &[Outcome]) -> Vec<String> {
    outcomes
        .iter()
        .map(|outcome| match &outcome.error {
            None => format!(
                "[{}] successfully submitted {}",
                outcome.status, outcome.url
            ),
            Some(error) => format!("[{}] {}: {error}", outcome.status, outcome.url),
        })
        .collect()
}

/// The closing message for a run that refused at least one URL, or `None` when
/// everything was accepted.
pub fn summary(outcomes: &[Outcome], verbose: bool) -> Option<String> {
    let failed = outcomes.iter().filter(|o| !o.succeeded()).count();
    if failed == 0 {
        return None;
    }

    let mut summary = format!("{failed} of {} URLs were not accepted", outcomes.len());
    if !verbose {
        summary.push_str(&format!("\n\n{VERBOSE_HINT}"));
    }
    Some(summary)
}

/// Describe what a real run would do, having contacted nothing.
pub fn dry_run(engine: &str, validated: &Validated, previews: &[Preview]) {
    println!("Dry run: no external system will be contacted.");
    println!("Engine: {engine}");
    match &validated.credential {
        // Never echo the token itself; it is a secret and the output may be pasted.
        Credential::Token(_) => println!("Credentials: token"),
        Credential::File(path) => println!("Credentials: file {} (valid JSON)", path.display()),
    }

    if !validated.urls.is_empty() {
        println!("\nURLs ({}):", validated.urls.len());
        for url in &validated.urls {
            println!("  {url}");
        }
    }

    if !previews.is_empty() {
        println!("\nSitemaps ({}):", previews.len());
        for preview in previews {
            match (&preview.source, &preview.urls) {
                (Source::Remote(url), _) => println!("  {url} — would be fetched and expanded"),
                (Source::Local(path), Some(urls)) => {
                    println!("  {} — local file, {} URLs", path.display(), urls.len());
                    for url in urls {
                        println!("    {url}");
                    }
                }
                (Source::Local(path), None) => println!("  {} — local file", path.display()),
            }
        }
    }

    println!("\nInput is valid. Remove --dry-run to submit.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    fn outcome(url: &str, status: u16, error: Option<&str>) -> Outcome {
        Outcome {
            url: Url::parse(url).unwrap(),
            status,
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn reports_a_success_with_its_status() {
        let outcomes = [outcome("https://example.com/a", 200, None)];
        assert_eq!(
            lines(&outcomes),
            ["[200] successfully submitted https://example.com/a"]
        );
    }

    #[test]
    fn reports_a_failure_with_the_error_text() {
        let outcomes = [outcome("https://example.com/a", 403, Some("key not valid"))];
        assert_eq!(
            lines(&outcomes),
            ["[403] https://example.com/a: key not valid"]
        );
    }

    #[test]
    fn no_summary_when_everything_was_accepted() {
        let outcomes = [
            outcome("https://example.com/a", 200, None),
            outcome("https://example.com/b", 200, None),
        ];
        assert!(summary(&outcomes, false).is_none());
    }

    #[test]
    fn summary_counts_only_the_failures() {
        let outcomes = [
            outcome("https://example.com/a", 200, None),
            outcome("https://example.com/b", 403, Some("nope")),
            outcome("https://example.com/c", 429, Some("slow down")),
        ];
        let summary = summary(&outcomes, false).unwrap();
        assert!(
            summary.starts_with("2 of 3 URLs were not accepted"),
            "{summary}"
        );
    }

    #[test]
    fn advises_verbose_only_when_not_already_verbose() {
        let outcomes = [outcome("https://example.com/a", 403, Some("nope"))];
        assert!(summary(&outcomes, false).unwrap().contains(VERBOSE_HINT));
        assert!(!summary(&outcomes, true).unwrap().contains(VERBOSE_HINT));
    }
}
