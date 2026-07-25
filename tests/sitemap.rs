//! Network-level checks of remote sitemap fetching, against a local mock server.

use httpmock::MockServer;
use indeks::sitemap::{self, Source};
use url::Url;

const VALID: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/</loc></url>
  <url><loc>https://example.com/about</loc></url>
</urlset>"#;

fn remote(server: &MockServer, path: &str) -> Source {
    Source::Remote(Url::parse(&server.url(path)).unwrap())
}

#[test]
fn fetches_and_parses_a_remote_sitemap() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method("GET").path("/sitemap.xml");
        then.status(200)
            .header("content-type", "application/xml")
            .body(VALID);
    });

    let urls = sitemap::load(
        &remote(&server, "/sitemap.xml"),
        &reqwest::blocking::Client::new(),
    )
    .unwrap();

    mock.assert();
    assert_eq!(
        urls.iter().map(Url::to_string).collect::<Vec<_>>(),
        ["https://example.com/", "https://example.com/about"]
    );
}

#[test]
fn reports_an_error_status_from_the_server() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("GET").path("/missing.xml");
        then.status(404).body("not found");
    });

    let error = sitemap::load(
        &remote(&server, "/missing.xml"),
        &reqwest::blocking::Client::new(),
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("the server answered 404"),
        "{error}"
    );
}

#[test]
fn reports_a_malformed_remote_sitemap() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("GET").path("/broken.xml");
        then.status(200)
            .body("<urlset><url><loc>https://example.com/</url>");
    });

    let error = sitemap::load(
        &remote(&server, "/broken.xml"),
        &reqwest::blocking::Client::new(),
    )
    .unwrap_err();

    assert!(error.to_string().contains("not valid XML"), "{error}");
}

#[test]
fn reports_a_connection_failure() {
    // An address nothing is listening on: bound to claim a free port, then closed.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);

    let url = Url::parse(&format!("http://{address}/sitemap.xml")).unwrap();

    let error = sitemap::load(&Source::Remote(url), &reqwest::blocking::Client::new()).unwrap_err();
    assert!(error.to_string().contains("could not be read"), "{error}");
}
