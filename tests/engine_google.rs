//! Google Indexing API submission, against a local mock server.
//!
//! The checked-in service account carries a real RSA key generated for tests, so
//! assertions here are genuinely signed. Its `token_uri` is rewritten to point at
//! the mock server.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use httpmock::MockServer;
use indeks::credentials::Credential;
use indeks::engine::Submitter;
use indeks::engine::google::{Backoff, Google};
use url::Url;

const FIXTURE: &str = concat!(env!("OUT_DIR"), "/service-account.json");
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
        // Retry immediately: these tests are about how many attempts happen,
        // not how long they wait.
        backoff: Backoff {
            attempts: 2,
            first_wait: Duration::ZERO,
        },
    }
}

/// A 429 body naming the quota the way Google does.
fn quota_body(metric: &str) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "code": 429,
            "message": format!(
                "Quota exceeded for quota metric 'Publish requests' and limit '{metric}' \
                 of service 'indexing.googleapis.com' for consumer 'project_number:1'."
            ),
            "status": "RESOURCE_EXHAUSTED",
        }
    })
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

/// Google refuses an assertion whose `iat` is not close to now ("Invalid JWT:
/// Token must be a short-lived token"), so the timestamps must be wall-clock.
#[test]
fn the_assertion_is_stamped_with_the_current_time() {
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method("POST").path("/token").is_true(|request| {
            let body = String::from_utf8_lossy(request.body_ref()).to_string();
            let assertion = body
                .split('&')
                .find_map(|pair| pair.strip_prefix("assertion="))
                .expect("no assertion in the token request");

            let claims = assertion.split('.').nth(1).expect("no JWT payload");
            let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(claims)
                .expect("payload is not base64url");
            let claims: serde_json::Value = serde_json::from_slice(&decoded).unwrap();

            let issued_at = claims["iat"].as_u64().unwrap();
            let expires = claims["exp"].as_u64().unwrap();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            issued_at.abs_diff(now) < 60 && expires > now
        });
        then.status(200).json_body(serde_json::json!({
            "access_token": ACCESS_TOKEN,
            "expires_in": 3599,
            "token_type": "Bearer",
        }));
    });
    server.mock(|when, then| {
        when.method("POST").path("/publish");
        then.status(200);
    });

    let credential = Credential::File(service_account(&server.url("/token")));
    engine(&server, credential)
        .submit(&urls(&["https://example.com/a"]))
        .unwrap();

    token.assert();
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
fn retries_a_per_minute_limit_and_carries_on() {
    let server = MockServer::start();
    mock_token_endpoint(&server);

    // Rate limited once, then fine.
    let limited = server.mock(|when, then| {
        when.method("POST").path("/publish").body_includes("/a");
        then.status(429)
            .json_body(quota_body("Requests per minute"));
    });
    let ok = server.mock(|when, then| {
        when.method("POST").path("/publish").body_includes("/b");
        then.status(200);
    });

    let credential = Credential::File(service_account(&server.url("/token")));
    let outcomes = engine(&server, credential)
        .submit(&urls(&["https://example.com/a", "https://example.com/b"]))
        .unwrap();

    // Three attempts at /a: the first plus two retries, all refused.
    assert_eq!(limited.calls(), 3);
    // A per-minute limit that never clears still stops the run, so /b is not
    // attempted — but the message says retrying was tried.
    assert_eq!(ok.calls(), 0);
    assert!(
        outcomes[1]
            .error
            .as_ref()
            .unwrap()
            .contains("still rate limited after retrying"),
        "{:?}",
        outcomes[1]
    );
}

#[test]
fn a_per_minute_limit_that_clears_lets_the_run_finish() {
    let server = MockServer::start();
    mock_token_endpoint(&server);

    let mut sequence = server.mock(|when, then| {
        when.method("POST").path("/publish");
        then.status(429)
            .json_body(quota_body("Requests per minute"));
    });
    let credential = Credential::File(service_account(&server.url("/token")));
    let google = engine(&server, credential);

    // First attempt is refused; by the retry the limit has cleared.
    sequence.delete();
    let publish = server.mock(|when, then| {
        when.method("POST").path("/publish");
        then.status(200);
    });

    let outcomes = google.submit(&urls(&["https://example.com/a"])).unwrap();

    assert_eq!(publish.calls(), 1);
    assert!(outcomes[0].succeeded(), "{:?}", outcomes[0]);
}

#[test]
fn does_not_retry_the_daily_quota() {
    let server = MockServer::start();
    mock_token_endpoint(&server);
    let publish = server.mock(|when, then| {
        when.method("POST").path("/publish");
        then.status(429)
            .json_body(quota_body("Publish requests per day"));
    });

    let credential = Credential::File(service_account(&server.url("/token")));
    let outcomes = engine(&server, credential)
        .submit(&urls(&["https://example.com/a", "https://example.com/b"]))
        .unwrap();

    // Waiting cannot help before midnight Pacific, so exactly one attempt.
    assert_eq!(publish.calls(), 1);
    assert!(
        outcomes[0]
            .error
            .as_ref()
            .unwrap()
            .contains("resets at midnight Pacific")
    );
    assert!(
        outcomes[1]
            .error
            .as_ref()
            .unwrap()
            .contains("the daily publish quota is spent")
    );
}

#[test]
fn honours_retry_after_when_the_server_sends_one() {
    let server = MockServer::start();
    mock_token_endpoint(&server);
    let publish = server.mock(|when, then| {
        when.method("POST").path("/publish");
        then.status(429)
            .header("retry-after", "0")
            .json_body(quota_body("Requests per minute"));
    });

    let credential = Credential::File(service_account(&server.url("/token")));
    engine(&server, credential)
        .submit(&urls(&["https://example.com/a"]))
        .unwrap();

    assert_eq!(publish.calls(), 3);
}

#[test]
fn stops_submitting_once_rate_limited() {
    let server = MockServer::start();
    mock_token_endpoint(&server);
    let publish = server.mock(|when, then| {
        when.method("POST").path("/publish");
        then.status(429)
            .json_body(quota_body("Publish requests per day"));
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
            .contains("the daily publish quota is spent")
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
