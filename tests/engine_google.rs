//! Google Indexing API submission, against a local mock server.
//!
//! The checked-in service account carries a real RSA key generated for tests, so
//! assertions here are genuinely signed. Its `token_uri` is rewritten to point at
//! the mock server.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use httpmock::MockServer;
use indeks::credentials::Credential;
use indeks::engine::Submitter;
use indeks::engine::google::Google;
use url::Url;

const FIXTURE: &str = "tests/fixtures/credentials.json";
const ACCESS_TOKEN: &str = "ya29.test-access-token";

/// Copy the service-account fixture with its token endpoint pointed elsewhere.
fn service_account(token_uri: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap();
    doc["token_uri"] = serde_json::json!(token_uri);

    let path = std::env::temp_dir().join(format!(
        "indeks-service-account-{}-{}.json",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, serde_json::to_string(&doc).unwrap()).unwrap();
    path
}

fn urls(values: &[&str]) -> Vec<Url> {
    values.iter().map(|v| Url::parse(v).unwrap()).collect()
}

/// A mock token endpoint that hands out [`ACCESS_TOKEN`].
fn mock_token_endpoint(server: &MockServer) -> httpmock::Mock<'_> {
    server.mock(|when, then| {
        when.method("POST").path("/token");
        then.status(200).json_body(serde_json::json!({
            "access_token": ACCESS_TOKEN,
            "expires_in": 3599,
            "token_type": "Bearer",
        }));
    })
}

fn engine(server: &MockServer, credential: Credential) -> Google {
    Google {
        credential,
        client: reqwest::blocking::Client::new(),
        endpoint: server.url("/publish"),
    }
}

#[test]
fn exchanges_an_assertion_then_publishes_each_url() {
    let server = MockServer::start();
    let token = mock_token_endpoint(&server);
    let publish = server.mock(|when, then| {
        when.method("POST")
            .path("/publish")
            .header("authorization", format!("Bearer {ACCESS_TOKEN}"));
        then.status(200).json_body(serde_json::json!({
            "urlNotificationMetadata": { "url": "https://example.com/a" }
        }));
    });

    let credential = Credential::File(service_account(&server.url("/token")));
    let outcomes = engine(&server, credential)
        .submit(&urls(&["https://example.com/a", "https://example.com/b"]))
        .unwrap();

    // One token exchange for the run, one publish per URL.
    assert_eq!(token.calls(), 1);
    assert_eq!(publish.calls(), 2);
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(|o| o.succeeded()));
}

#[test]
fn sends_the_notification_google_expects() {
    let server = MockServer::start();
    mock_token_endpoint(&server);
    let publish = server.mock(|when, then| {
        when.method("POST")
            .path("/publish")
            .json_body(serde_json::json!({
                "url": "https://example.com/a",
                "type": "URL_UPDATED",
            }));
        then.status(200);
    });

    let credential = Credential::File(service_account(&server.url("/token")));
    engine(&server, credential)
        .submit(&urls(&["https://example.com/a"]))
        .unwrap();

    publish.assert();
}

#[test]
fn a_bare_token_skips_the_token_endpoint() {
    let server = MockServer::start();
    let token = mock_token_endpoint(&server);
    let publish = server.mock(|when, then| {
        when.method("POST")
            .path("/publish")
            .header("authorization", "Bearer ya29.given-directly");
        then.status(200);
    });

    let credential = Credential::Token("ya29.given-directly".to_string());
    let outcomes = engine(&server, credential)
        .submit(&urls(&["https://example.com/a"]))
        .unwrap();

    assert_eq!(token.calls(), 0, "no assertion should have been exchanged");
    assert_eq!(publish.calls(), 1);
    assert!(outcomes[0].succeeded());
}

#[test]
fn reports_a_permission_failure_with_googles_own_message() {
    let server = MockServer::start();
    mock_token_endpoint(&server);
    server.mock(|when, then| {
        when.method("POST").path("/publish");
        then.status(403).json_body(serde_json::json!({
            "error": {
                "code": 403,
                "message": "Permission denied. Failed to verify the URL ownership.",
                "status": "PERMISSION_DENIED",
            }
        }));
    });

    let credential = Credential::File(service_account(&server.url("/token")));
    let outcomes = engine(&server, credential)
        .submit(&urls(&["https://example.com/a"]))
        .unwrap();

    let error = outcomes[0].error.as_ref().unwrap();
    assert_eq!(outcomes[0].status, 403);
    assert!(
        error.contains("Failed to verify the URL ownership"),
        "{error}"
    );
    assert!(
        error.contains("owner of the property in Search Console"),
        "{error}"
    );
}

#[test]
fn stops_submitting_once_rate_limited() {
    let server = MockServer::start();
    mock_token_endpoint(&server);
    let publish = server.mock(|when, then| {
        when.method("POST").path("/publish");
        then.status(429);
    });

    let credential = Credential::File(service_account(&server.url("/token")));
    let outcomes = engine(&server, credential)
        .submit(&urls(&[
            "https://example.com/a",
            "https://example.com/b",
            "https://example.com/c",
        ]))
        .unwrap();

    // Every URL is still accounted for, but only the first was actually sent.
    assert_eq!(publish.calls(), 1);
    assert_eq!(outcomes.len(), 3);
    assert!(
        outcomes[0]
            .error
            .as_ref()
            .unwrap()
            .contains("200 URLs per day")
    );
    assert!(
        outcomes[1]
            .error
            .as_ref()
            .unwrap()
            .starts_with("not attempted")
    );
    assert!(
        outcomes[2]
            .error
            .as_ref()
            .unwrap()
            .starts_with("not attempted")
    );
}

#[test]
fn a_refused_assertion_fails_the_run() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("POST").path("/token");
        then.status(400).json_body(serde_json::json!({
            "error": "invalid_grant",
            "error_description": "Invalid JWT Signature.",
        }));
    });
    let publish = server.mock(|when, then| {
        when.method("POST").path("/publish");
        then.status(200);
    });

    let credential = Credential::File(service_account(&server.url("/token")));
    let error = engine(&server, credential)
        .submit(&urls(&["https://example.com/a"]))
        .unwrap_err();

    assert_eq!(publish.calls(), 0, "nothing should have been published");
    assert!(
        error.to_string().contains("the token endpoint at"),
        "{error}"
    );
}

#[test]
fn a_token_endpoint_that_answers_nonsense_fails_the_run() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("POST").path("/token");
        then.status(200).body("not json");
    });

    let credential = Credential::File(service_account(&server.url("/token")));
    let error = engine(&server, credential)
        .submit(&urls(&["https://example.com/a"]))
        .unwrap_err();

    assert!(
        error.to_string().contains("did not return an access token"),
        "{error}"
    );
}
