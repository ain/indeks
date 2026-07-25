//! Assembling the final list of URLs to submit.

use std::collections::HashSet;

use reqwest::blocking::Client;
use url::Url;

use crate::sitemap::{self, SitemapError};
use crate::validate::Validated;

/// Expand every sitemap and combine the result with the URLs given directly.
///
/// A sitemap that cannot be read or parsed fails the run here, before anything
/// is submitted.
pub fn collect(validated: &Validated, client: &Client) -> Result<Vec<Url>, SitemapError> {
    let mut urls = validated.urls.clone();
    for source in &validated.sitemaps {
        urls.extend(sitemap::load(source, client)?);
    }
    Ok(dedupe(urls))
}

/// Drop repeats, keeping the first occurrence.
///
/// A URL listed twice — passed with `--url` and also present in a sitemap, say —
/// should cost one submission, not two, since both engines meter them.
pub fn dedupe(urls: Vec<Url>) -> Vec<Url> {
    let mut seen = HashSet::new();
    urls.into_iter()
        .filter(|url| seen.insert(url.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(values: &[&str]) -> Vec<Url> {
        values.iter().map(|v| Url::parse(v).unwrap()).collect()
    }

    #[test]
    fn keeps_distinct_urls_in_order() {
        let input = urls(&["https://example.com/b", "https://example.com/a"]);
        assert_eq!(dedupe(input.clone()), input);
    }

    #[test]
    fn drops_repeats_keeping_the_first() {
        let input = urls(&[
            "https://example.com/a",
            "https://example.com/b",
            "https://example.com/a",
        ]);
        assert_eq!(
            dedupe(input),
            urls(&["https://example.com/a", "https://example.com/b"])
        );
    }

    #[test]
    fn treats_a_bare_host_and_a_trailing_slash_as_one_url() {
        let input = urls(&["https://example.com", "https://example.com/"]);
        assert_eq!(dedupe(input).len(), 1);
    }

    #[test]
    fn keeps_urls_that_differ_only_in_query_or_fragment() {
        let input = urls(&[
            "https://example.com/a",
            "https://example.com/a?x=1",
            "https://example.com/a#top",
        ]);
        assert_eq!(dedupe(input).len(), 3);
    }
}
