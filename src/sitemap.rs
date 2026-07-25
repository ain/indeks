//! Fetching and parsing of sitemaps.org XML sitemaps.

use std::fmt;
use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::escape;
use quick_xml::events::{BytesRef, Event};
use url::Url;

use crate::validate::{ValidationError, argument};

/// Where a `--sitemap` argument points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A remote sitemap, fetched with GET. Not fetched under `--dry-run`.
    Remote(Url),

    /// A local file or `file://` URL. Parsed even under `--dry-run`, since
    /// reading it contacts no external system.
    Local(PathBuf),
}

impl Source {
    /// Decide what a `--sitemap` value points at.
    ///
    /// An existing file wins over URL parsing, so a bare `sitemap.xml` works.
    /// Otherwise the value must be an absolute http(s) or `file://` URL.
    pub fn classify(raw: &str) -> Result<Self, ValidationError> {
        if raw.trim().is_empty() {
            return Err(argument("sitemap", raw, "is empty"));
        }

        let path = Path::new(raw);
        if path.is_file() {
            return Ok(Source::Local(path.to_path_buf()));
        }

        match Url::parse(raw) {
            Ok(url) if url.scheme() == "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|()| argument("sitemap", raw, "is not a usable file path"))?;
                if !path.is_file() {
                    return Err(argument("sitemap", raw, "no such file"));
                }
                Ok(Source::Local(path))
            }
            // No host check: http and https are special schemes, so a missing
            // host has already failed in `Url::parse` above.
            Ok(url) if matches!(url.scheme(), "http" | "https") => Ok(Source::Remote(url)),
            Ok(url) => Err(argument(
                "sitemap",
                raw,
                format!(
                    "scheme `{}` is not supported; use http, https or a file path",
                    url.scheme()
                ),
            )),
            Err(_) if path.is_dir() => Err(argument(
                "sitemap",
                raw,
                "is a directory, not a sitemap file",
            )),
            Err(_) => Err(argument(
                "sitemap",
                raw,
                "is neither an absolute URL nor an existing file",
            )),
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Remote(url) => write!(f, "{url}"),
            Source::Local(path) => write!(f, "{}", path.display()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SitemapError {
    #[error("{source_name}: not valid XML: {reason}")]
    Malformed { source_name: String, reason: String },

    #[error("{source_name}: no <urlset><url><loc> entries found")]
    Empty { source_name: String },

    #[error("{source_name}: <loc> {value}: {reason}")]
    InvalidLoc {
        source_name: String,
        value: String,
        reason: String,
    },

    /// Nested sitemaps are out of scope for the initial version.
    #[error("{source_name}: sitemap index files are not supported")]
    SitemapIndex { source_name: String },

    #[error("{source_name}: could not be read: {reason}")]
    Unreadable { source_name: String, reason: String },
}

/// What a dry run can say about one sitemap without contacting anything.
#[derive(Debug)]
pub struct Preview {
    pub source: Source,
    /// The entries of a local sitemap. `None` for a remote one, which a dry run
    /// does not fetch.
    pub entries: Option<Vec<Entry>>,
}

/// One `<url>` from a sitemap, with the fields prioritisation can order by.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub url: Url,
    /// `<priority>`, 0.0 to 1.0. `None` when absent or unusable; sitemaps.org
    /// defines 0.5 as the default, but that belongs to whoever orders entries.
    pub priority: Option<f32>,
    pub changefreq: Option<ChangeFreq>,
}

/// `<changefreq>`, ordered least to most frequent so that deriving `Ord` gives
/// the ranking prioritisation needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangeFreq {
    Never,
    Yearly,
    Monthly,
    Weekly,
    Daily,
    Hourly,
    Always,
}

impl ChangeFreq {
    /// Parse one of the seven values sitemaps.org defines. Anything else is
    /// `None`: an unrecognised frequency is no more usable than an absent one.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "always" => Some(ChangeFreq::Always),
            "hourly" => Some(ChangeFreq::Hourly),
            "daily" => Some(ChangeFreq::Daily),
            "weekly" => Some(ChangeFreq::Weekly),
            "monthly" => Some(ChangeFreq::Monthly),
            "yearly" => Some(ChangeFreq::Yearly),
            "never" => Some(ChangeFreq::Never),
            _ => None,
        }
    }
}

/// The `<url>` children this parser reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Loc,
    Priority,
    ChangeFreq,
}

impl Field {
    fn of(name: &str) -> Option<Self> {
        match name {
            "loc" => Some(Field::Loc),
            "priority" => Some(Field::Priority),
            "changefreq" => Some(Field::ChangeFreq),
            _ => None,
        }
    }
}

/// A `<url>` element being read.
#[derive(Debug, Default)]
struct Pending {
    loc: Option<String>,
    priority: Option<f32>,
    changefreq: Option<ChangeFreq>,
}

/// Extract every `urlset > url` from a sitemap document, with its `<loc>` and
/// the fields that can order it.
///
/// Requires at least one usable entry, and every `loc` to be an absolute http(s)
/// URL. A `<url>` carrying no `<loc>` contributes nothing.
///
/// Elements are matched on local name, so a namespace prefix is accepted and a
/// missing or unexpected `xmlns` does not cause a valid sitemap to be rejected.
/// Only the exact `urlset > url > *` nesting counts, which keeps the `<loc>` of
/// an image or video extension — nested a level deeper — out of the results.
pub fn parse(source_name: &str, xml: &str) -> Result<Vec<Entry>, SitemapError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut path: Vec<String> = Vec::new();
    let mut entries = Vec::new();
    let mut pending: Option<Pending> = None;
    let mut text: Option<String> = None;
    let mut root_seen = false;

    loop {
        let event = reader
            .read_event()
            .map_err(|error| malformed(source_name, error.to_string()))?;

        match event {
            Event::Eof => break,

            Event::Start(start) => {
                let name = local_name(start.name().as_ref());
                if !root_seen {
                    root_seen = true;
                    check_root(source_name, &name)?;
                }
                path.push(name);

                if is_url(&path) {
                    pending = Some(Pending::default());
                } else if field(&path).is_some() {
                    text = Some(String::new());
                }
            }

            Event::Empty(empty) => {
                let name = local_name(empty.name().as_ref());
                if !root_seen {
                    root_seen = true;
                    check_root(source_name, &name)?;
                }
                path.push(name);

                // A self-closing element holds no text, so an empty <loc/> is
                // rejected the same way a blank one is.
                if field(&path) == Some(Field::Loc)
                    && let Some(pending) = pending.as_mut()
                {
                    pending.loc = Some(String::new());
                }
                path.pop();
            }

            Event::End(_) => {
                if let (Some(field), Some(value)) = (field(&path), text.take())
                    && let Some(pending) = pending.as_mut()
                {
                    store(source_name, pending, field, &value);
                }

                if is_url(&path)
                    && let Some(pending) = pending.take()
                    && let Some(entry) = finish(source_name, pending)?
                {
                    entries.push(entry);
                }

                path.pop();
            }

            Event::Text(chunk) if text.is_some() => {
                let raw = chunk
                    .decode()
                    .map_err(|error| malformed(source_name, error.to_string()))?;
                text.get_or_insert_default().push_str(&raw);
            }

            // CDATA is literal, so it is decoded as-is.
            Event::CData(cdata) if text.is_some() => {
                let raw = cdata
                    .decode()
                    .map_err(|error| malformed(source_name, error.to_string()))?;
                text.get_or_insert_default().push_str(&raw);
            }

            // Entity references are separate events rather than part of the
            // surrounding text, so `&amp;` in a <loc> is resolved here.
            Event::GeneralRef(reference) if text.is_some() => {
                let resolved = resolve_reference(source_name, &reference)?;
                text.get_or_insert_default().push_str(&resolved);
            }

            _ => {}
        }
    }

    if entries.is_empty() {
        return Err(SitemapError::Empty {
            source_name: source_name.to_string(),
        });
    }

    Ok(entries)
}

/// Record one field of the `<url>` being read.
///
/// A `priority` or `changefreq` that cannot be used is warned about and left
/// unset rather than failing the parse: it would otherwise reject sitemaps that
/// work today, for runs that never asked for prioritisation.
fn store(source_name: &str, pending: &mut Pending, field: Field, value: &str) {
    match field {
        Field::Loc => pending.loc = Some(value.to_string()),

        Field::Priority => match value.trim().parse::<f32>() {
            Ok(priority) if (0.0..=1.0).contains(&priority) => pending.priority = Some(priority),
            Ok(priority) => {
                let clamped = priority.clamp(0.0, 1.0);
                tracing::warn!(
                    "{source_name}: <priority> {priority} is out of range, using {clamped}"
                );
                pending.priority = Some(clamped);
            }
            Err(_) => {
                tracing::warn!("{source_name}: <priority> {value:?} is not a number, ignoring it");
            }
        },

        Field::ChangeFreq => match ChangeFreq::parse(value) {
            Some(changefreq) => pending.changefreq = Some(changefreq),
            None => {
                tracing::warn!(
                    "{source_name}: <changefreq> {value:?} is not a known value, ignoring it"
                );
            }
        },
    }
}

/// Turn a finished `<url>` into an entry, or nothing when it carried no `<loc>`.
fn finish(source_name: &str, pending: Pending) -> Result<Option<Entry>, SitemapError> {
    let Some(loc) = pending.loc else {
        return Ok(None);
    };

    Ok(Some(Entry {
        url: parse_loc(source_name, &loc)?,
        priority: pending.priority,
        changefreq: pending.changefreq,
    }))
}

/// Read a local sitemap, or fetch a remote one, then parse it.
pub fn load(
    source: &Source,
    client: &reqwest::blocking::Client,
) -> Result<Vec<Entry>, SitemapError> {
    let name = source.to_string();
    let xml = match source {
        Source::Local(path) => read_local(&name, path)?,
        Source::Remote(url) => fetch_remote(&name, url, client)?,
    };
    parse(&name, &xml)
}

/// Describe each sitemap without contacting anything: local ones are parsed,
/// remote ones are left unexpanded.
pub fn preview(sources: &[Source]) -> Result<Vec<Preview>, SitemapError> {
    sources
        .iter()
        .map(|source| {
            let entries = match source {
                Source::Local(path) => {
                    let name = source.to_string();
                    Some(parse(&name, &read_local(&name, path)?)?)
                }
                Source::Remote(_) => None,
            };
            Ok(Preview {
                source: source.clone(),
                entries,
            })
        })
        .collect()
}

fn read_local(source_name: &str, path: &Path) -> Result<String, SitemapError> {
    std::fs::read_to_string(path).map_err(|error| SitemapError::Unreadable {
        source_name: source_name.to_string(),
        reason: error.to_string(),
    })
}

fn fetch_remote(
    source_name: &str,
    url: &Url,
    client: &reqwest::blocking::Client,
) -> Result<String, SitemapError> {
    let unreadable = |reason: String| SitemapError::Unreadable {
        source_name: source_name.to_string(),
        reason,
    };

    let request = client
        .get(url.clone())
        .build()
        .map_err(|error| unreadable(error.to_string()))?;

    // Through `http::send` rather than straight to reqwest, so that --verbose
    // covers sitemap fetches too.
    let exchange =
        crate::http::send(client, request).map_err(|error| unreadable(error.to_string()))?;

    if !exchange.status.is_success() {
        return Err(unreadable(format!(
            "the server answered {}",
            exchange.status
        )));
    }

    Ok(exchange.body)
}

fn malformed(source_name: &str, reason: impl Into<String>) -> SitemapError {
    SitemapError::Malformed {
        source_name: source_name.to_string(),
        reason: reason.into(),
    }
}

/// Resolve one entity reference found inside a `<loc>`.
///
/// Only character references and the five predefined XML entities are accepted.
/// An entity declared in a DTD is rejected rather than ignored, which also keeps
/// this parser away from entity-expansion tricks.
fn resolve_reference(source_name: &str, reference: &BytesRef<'_>) -> Result<String, SitemapError> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|error| malformed(source_name, error.to_string()))?
    {
        return Ok(character.to_string());
    }

    let name = reference
        .decode()
        .map_err(|error| malformed(source_name, error.to_string()))?;

    escape::resolve_predefined_entity(&name)
        .map(str::to_string)
        .ok_or_else(|| malformed(source_name, format!("unknown entity &{name};")))
}

/// The element name with any namespace prefix stripped.
fn local_name(qualified: &[u8]) -> String {
    let name = String::from_utf8_lossy(qualified);
    match name.split_once(':') {
        Some((_, local)) => local.to_string(),
        None => name.into_owned(),
    }
}

fn check_root(source_name: &str, name: &str) -> Result<(), SitemapError> {
    match name {
        "urlset" => Ok(()),
        "sitemapindex" => Err(SitemapError::SitemapIndex {
            source_name: source_name.to_string(),
        }),
        other => Err(SitemapError::Malformed {
            source_name: source_name.to_string(),
            reason: format!("root element is <{other}>, expected <urlset>"),
        }),
    }
}

/// Whether the path is exactly `urlset > url`.
fn is_url(path: &[String]) -> bool {
    path.len() == 2 && path[0] == "urlset" && path[1] == "url"
}

/// Which `<url>` child the path names, if it is one this parser reads.
///
/// The depth check is what keeps an image or video extension's `<loc>`, nested
/// one level further in, out of the results.
fn field(path: &[String]) -> Option<Field> {
    if path.len() == 3 && path[0] == "urlset" && path[1] == "url" {
        Field::of(&path[2])
    } else {
        None
    }
}

fn parse_loc(source_name: &str, value: &str) -> Result<Url, SitemapError> {
    let trimmed = value.trim();
    let invalid = |reason: &str| SitemapError::InvalidLoc {
        source_name: source_name.to_string(),
        value: trimmed.to_string(),
        reason: reason.to_string(),
    };

    if trimmed.is_empty() {
        return Err(invalid("is empty"));
    }

    let url = Url::parse(trimmed).map_err(|_| invalid("is not an absolute URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid("is not an http or https URL"));
    }

    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "tests/fixtures/sitemap.xml";

    fn locs(xml: &str) -> Vec<String> {
        parse("test", xml)
            .unwrap()
            .iter()
            .map(|entry| entry.url.to_string())
            .collect()
    }

    fn only(xml: &str) -> Entry {
        let mut entries = parse("test", xml).unwrap();
        assert_eq!(entries.len(), 1, "{entries:?}");
        entries.remove(0)
    }

    /// A sitemap of one `<url>` carrying the given children.
    fn one_url(children: &str) -> String {
        format!("<urlset><url><loc>https://example.com/a</loc>{children}</url></urlset>")
    }

    #[test]
    fn classifies_an_http_url_as_remote() {
        let source = Source::classify("https://example.com/sitemap.xml").unwrap();
        assert!(matches!(source, Source::Remote(_)));
    }

    #[test]
    fn classifies_an_existing_file_as_local() {
        assert_eq!(
            Source::classify(FIXTURE).unwrap(),
            Source::Local(PathBuf::from(FIXTURE))
        );
    }

    #[test]
    fn classifies_a_file_url_as_local() {
        let absolute = std::fs::canonicalize(FIXTURE).unwrap();
        let value = Url::from_file_path(&absolute).unwrap().to_string();
        assert_eq!(Source::classify(&value).unwrap(), Source::Local(absolute));
    }

    #[test]
    fn rejects_a_missing_file_url() {
        let error = Source::classify("file:///no/such/sitemap.xml").unwrap_err();
        assert!(error.to_string().contains("no such file"), "{error}");
    }

    #[test]
    fn rejects_a_relative_value_that_does_not_exist() {
        let error = Source::classify("sitemap.xml").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("neither an absolute URL nor an existing file"),
            "{error}"
        );
    }

    #[test]
    fn rejects_an_unsupported_scheme() {
        let error = Source::classify("ftp://example.com/sitemap.xml").unwrap_err();
        assert!(error.to_string().contains("scheme `ftp`"), "{error}");
    }

    #[test]
    fn rejects_a_directory() {
        let error = Source::classify("tests/fixtures").unwrap_err();
        assert!(error.to_string().contains("is a directory"), "{error}");
    }

    #[test]
    fn shows_a_source_as_the_user_gave_it() {
        let remote = Source::Remote(Url::parse("https://example.com/sitemap.xml").unwrap());
        assert_eq!(remote.to_string(), "https://example.com/sitemap.xml");

        let local = Source::Local(PathBuf::from(FIXTURE));
        assert_eq!(local.to_string(), FIXTURE);
    }

    #[test]
    fn ignores_text_outside_a_loc() {
        // With the <loc> guard broken, `lastmod` text would be read as a URL.
        let xml = "<urlset><url><lastmod>2026-07-01</lastmod>\
                   <loc>https://example.com/a</loc>\
                   <changefreq>daily</changefreq></url></urlset>";
        assert_eq!(locs(xml), ["https://example.com/a"]);
    }

    #[test]
    fn ignores_cdata_outside_a_loc() {
        let xml = "<urlset><url><lastmod><![CDATA[2026-07-01]]></lastmod>\
                   <loc>https://example.com/a</loc></url></urlset>";
        assert_eq!(locs(xml), ["https://example.com/a"]);
    }

    #[test]
    fn ignores_entity_references_outside_a_loc() {
        let xml = "<urlset><url><news>Bild &amp; Ton</news>\
                   <loc>https://example.com/a</loc></url></urlset>";
        assert_eq!(locs(xml), ["https://example.com/a"]);
    }

    #[test]
    fn parses_a_plain_sitemap() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
            <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
              <url><loc>https://example.com/</loc><lastmod>2026-07-01</lastmod></url>
              <url><loc>https://example.com/about</loc></url>
            </urlset>"#;
        assert_eq!(
            locs(xml),
            ["https://example.com/", "https://example.com/about"]
        );
    }

    #[test]
    fn parses_the_checked_in_fixture() {
        let xml = std::fs::read_to_string(FIXTURE).unwrap();
        assert_eq!(parse(FIXTURE, &xml).unwrap().len(), 3);
    }

    #[test]
    fn accepts_a_namespace_prefix() {
        let xml = r#"<sm:urlset xmlns:sm="http://www.sitemaps.org/schemas/sitemap/0.9">
              <sm:url><sm:loc>https://example.com/a</sm:loc></sm:url>
            </sm:urlset>"#;
        assert_eq!(locs(xml), ["https://example.com/a"]);
    }

    #[test]
    fn tolerates_whitespace_around_a_loc() {
        let xml = "<urlset><url><loc>\n   https://example.com/a\n  </loc></url></urlset>";
        assert_eq!(locs(xml), ["https://example.com/a"]);
    }

    #[test]
    fn unescapes_entities_in_a_loc() {
        let xml = "<urlset><url><loc>https://example.com/?a=1&amp;b=2</loc></url></urlset>";
        assert_eq!(locs(xml), ["https://example.com/?a=1&b=2"]);
    }

    #[test]
    fn resolves_character_references_in_a_loc() {
        let xml = "<urlset><url><loc>https://example.com/a&#x2F;b&#47;c</loc></url></urlset>";
        assert_eq!(locs(xml), ["https://example.com/a/b/c"]);
    }

    #[test]
    fn rejects_an_entity_declared_in_a_dtd() {
        let xml = r#"<!DOCTYPE urlset [<!ENTITY host "example.com">]>
            <urlset><url><loc>https://&host;/a</loc></url></urlset>"#;
        let error = parse("test", xml).unwrap_err();
        assert!(
            error.to_string().contains("unknown entity &host;"),
            "{error}"
        );
    }

    #[test]
    fn reads_a_loc_from_cdata() {
        let xml =
            "<urlset><url><loc><![CDATA[https://example.com/a?x=1&y=2]]></loc></url></urlset>";
        assert_eq!(locs(xml), ["https://example.com/a?x=1&y=2"]);
    }

    #[test]
    fn ignores_a_loc_nested_deeper_than_url() {
        // Image and video extensions carry their own <loc> one level further in.
        let xml = r#"<urlset xmlns:image="http://www.google.com/schemas/sitemap-image/1.1">
              <url>
                <loc>https://example.com/a</loc>
                <image:image><image:loc>https://cdn.example.com/a.jpg</image:loc></image:image>
              </url>
            </urlset>"#;
        assert_eq!(locs(xml), ["https://example.com/a"]);
    }

    #[test]
    fn rejects_malformed_xml() {
        let error = parse(
            "test",
            "<urlset><url><loc>https://example.com/a</url></urlset>",
        )
        .unwrap_err();
        assert!(error.to_string().contains("not valid XML"), "{error}");
    }

    #[test]
    fn rejects_a_sitemap_index() {
        let xml = r#"<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
              <sitemap><loc>https://example.com/sitemap-1.xml</loc></sitemap>
            </sitemapindex>"#;
        let error = parse("test", xml).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("sitemap index files are not supported"),
            "{error}"
        );
    }

    #[test]
    fn rejects_an_unexpected_root_element() {
        let error = parse("test", "<html><body>nope</body></html>").unwrap_err();
        assert!(
            error.to_string().contains("root element is <html>"),
            "{error}"
        );
    }

    #[test]
    fn rejects_a_urlset_without_entries() {
        let error = parse("test", "<urlset></urlset>").unwrap_err();
        assert!(
            error.to_string().contains("no <urlset><url><loc>"),
            "{error}"
        );
    }

    #[test]
    fn rejects_a_relative_loc() {
        let error = parse("test", "<urlset><url><loc>/about</loc></url></urlset>").unwrap_err();
        assert!(
            error.to_string().contains("is not an absolute URL"),
            "{error}"
        );
    }

    #[test]
    fn rejects_a_non_http_loc() {
        let xml = "<urlset><url><loc>ftp://example.com/a</loc></url></urlset>";
        let error = parse("test", xml).unwrap_err();
        assert!(
            error.to_string().contains("is not an http or https URL"),
            "{error}"
        );
    }

    #[test]
    fn rejects_an_empty_loc() {
        for xml in [
            "<urlset><url><loc></loc></url></urlset>",
            "<urlset><url><loc/></url></urlset>",
            "<urlset><url><loc>   </loc></url></urlset>",
        ] {
            let error = parse("test", xml).unwrap_err();
            assert!(error.to_string().contains("is empty"), "{xml}: {error}");
        }
    }

    #[test]
    fn previews_local_sitemaps_and_leaves_remote_ones_alone() {
        let sources = vec![
            Source::Local(PathBuf::from(FIXTURE)),
            Source::Remote(Url::parse("https://example.com/sitemap.xml").unwrap()),
        ];
        let previews = preview(&sources).unwrap();

        assert_eq!(previews[0].entries.as_ref().unwrap().len(), 3);
        assert!(previews[1].entries.is_none());
    }

    #[test]
    fn reads_priority_and_changefreq() {
        let entry = only(&one_url(
            "<priority>0.8</priority><changefreq>daily</changefreq>",
        ));
        assert_eq!(entry.priority, Some(0.8));
        assert_eq!(entry.changefreq, Some(ChangeFreq::Daily));
    }

    #[test]
    fn a_url_without_them_carries_neither() {
        let entry = only(&one_url(""));
        assert_eq!(entry.priority, None);
        assert_eq!(entry.changefreq, None);
    }

    #[test]
    fn accepts_priority_at_both_ends_of_the_range() {
        assert_eq!(
            only(&one_url("<priority>0.0</priority>")).priority,
            Some(0.0)
        );
        assert_eq!(
            only(&one_url("<priority>1.0</priority>")).priority,
            Some(1.0)
        );
    }

    #[test]
    fn clamps_a_priority_outside_the_range() {
        assert_eq!(only(&one_url("<priority>7</priority>")).priority, Some(1.0));
        assert_eq!(
            only(&one_url("<priority>-2</priority>")).priority,
            Some(0.0)
        );
    }

    #[test]
    fn ignores_a_priority_that_is_not_a_number() {
        // Warned about, not fatal: failing here would reject sitemaps that work
        // today, for runs that never asked to prioritise.
        assert_eq!(only(&one_url("<priority>high</priority>")).priority, None);
    }

    #[test]
    fn ignores_an_unknown_changefreq() {
        assert_eq!(
            only(&one_url("<changefreq>fortnightly</changefreq>")).changefreq,
            None
        );
    }

    #[test]
    fn reads_changefreq_whatever_its_case() {
        assert_eq!(
            only(&one_url("<changefreq>  Weekly </changefreq>")).changefreq,
            Some(ChangeFreq::Weekly)
        );
    }

    #[test]
    fn changefreq_ranks_least_to_most_frequent() {
        let mut all = vec![
            ChangeFreq::Daily,
            ChangeFreq::Never,
            ChangeFreq::Always,
            ChangeFreq::Monthly,
        ];
        all.sort();
        assert_eq!(
            all,
            [
                ChangeFreq::Never,
                ChangeFreq::Monthly,
                ChangeFreq::Daily,
                ChangeFreq::Always
            ]
        );
    }

    #[test]
    fn keeps_each_urls_fields_to_itself() {
        let xml = "<urlset>\
                   <url><loc>https://example.com/a</loc><priority>0.9</priority></url>\
                   <url><loc>https://example.com/b</loc></url>\
                   <url><loc>https://example.com/c</loc><changefreq>hourly</changefreq></url>\
                   </urlset>";
        let entries = parse("test", xml).unwrap();

        assert_eq!(entries[0].priority, Some(0.9));
        assert_eq!(entries[1].priority, None);
        assert_eq!(entries[1].changefreq, None);
        assert_eq!(entries[2].changefreq, Some(ChangeFreq::Hourly));
    }

    #[test]
    fn ignores_the_priority_of_an_extension_element() {
        // An image extension nests its own fields a level deeper.
        let xml = "<urlset><url><loc>https://example.com/a</loc>\
                   <image:image><priority>0.1</priority></image:image>\
                   </url></urlset>";
        assert_eq!(parse("test", xml).unwrap()[0].priority, None);
    }

    #[test]
    fn a_url_without_a_loc_contributes_nothing() {
        let xml = "<urlset>\
                   <url><changefreq>daily</changefreq></url>\
                   <url><loc>https://example.com/a</loc></url>\
                   </urlset>";
        assert_eq!(locs(xml), ["https://example.com/a"]);
    }

    #[test]
    fn preview_reports_an_unreadable_local_sitemap() {
        let sources = vec![Source::Local(PathBuf::from("tests/fixtures/no-such.xml"))];
        let error = preview(&sources).unwrap_err();
        assert!(error.to_string().contains("could not be read"), "{error}");
    }
}
