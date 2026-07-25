//! Assembling the URL list from every source, against a local mock server.

use httpmock::MockServer;
use indeks::credentials::Credential;
use indeks::sitemap::Source;
use indeks::targets;
use indeks::validate::Validated;
use url::Url;

const SITEMAP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/a</loc></url>
  <url><loc>https://example.com/shared</loc></url>
</urlset>"#;

fn validated(urls: &[&str], sitemaps: Vec<Source>) -> Validated {
    Validated {
        urls: urls.iter().map(|v| Url::parse(v).unwrap()).collect(),
        sitemaps,
        credential: Credential::Token("abcdef0123456789".to_string()),
        dry_run: false,
        verbose: false,
    }
}

fn serve(server: &MockServer, path: &str, body: &str) -> Source {
    let body = body.to_string();
    server.mock(move |when, then| {
        when.method("GET").path(path.to_string());
        then.status(200).body(body);
    });
    Source::Remote(Url::parse(&server.url(path)).unwrap())
}

#[test]
fn combines_direct_urls_with_expanded_sitemaps() {
    let server = MockServer::start();
    let source = serve(&server, "/sitemap.xml", SITEMAP);

    let collected = targets::collect(
        &validated(&["https://example.com/direct"], vec![source]),
        &reqwest::blocking::Client::new(),
    )
    .unwrap();

    assert_eq!(
        collected.iter().map(Url::to_string).collect::<Vec<_>>(),
        [
            "https://example.com/direct",
            "https://example.com/a",
            "https://example.com/shared",
        ]
    );
}

#[test]
fn a_url_in_both_a_flag_and_a_sitemap_is_submitted_once() {
    let server = MockServer::start();
    let source = serve(&server, "/sitemap.xml", SITEMAP);

    let collected = targets::collect(
        &validated(&["https://example.com/shared"], vec![source]),
        &reqwest::blocking::Client::new(),
    )
    .unwrap();

    assert_eq!(collected.len(), 2, "{collected:?}");
    assert_eq!(collected[0].as_str(), "https://example.com/shared");
}

#[test]
fn the_same_url_in_two_sitemaps_is_submitted_once() {
    let server = MockServer::start();
    let first = serve(&server, "/one.xml", SITEMAP);
    let second = serve(&server, "/two.xml", SITEMAP);

    let collected = targets::collect(
        &validated(&[], vec![first, second]),
        &reqwest::blocking::Client::new(),
    )
    .unwrap();

    assert_eq!(collected.len(), 2, "{collected:?}");
}

#[test]
fn one_unusable_sitemap_fails_the_whole_run() {
    let server = MockServer::start();
    let good = serve(&server, "/good.xml", SITEMAP);
    let bad = serve(
        &server,
        "/bad.xml",
        "<urlset><url><loc>/relative</loc></url></urlset>",
    );

    let error = targets::collect(
        &validated(&[], vec![good, bad]),
        &reqwest::blocking::Client::new(),
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("is not an absolute URL"),
        "{error}"
    );
}
