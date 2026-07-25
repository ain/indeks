//! IndexNow submission, against a local mock server.

use httpmock::MockServer;
use indeks::engine::Submitter;
use indeks::engine::indexnow::IndexNow;
use url::Url;

const KEY: &str = "abcdef0123456789";

fn engine(server: &MockServer) -> IndexNow {
    IndexNow {
        key: KEY.to_string(),
        client: reqwest::blocking::Client::new(),
        endpoint: server.url("/indexnow"),
    }
}

fn urls(values: &[&str]) -> Vec<Url> {
    values.iter().map(|v| Url::parse(v).unwrap()).collect()
}

#[test]
fn submits_one_batch_per_host() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method("POST").path("/indexnow");
        then.status(200);
    });

    let outcomes = engine(&server)
        .submit(&urls(&[
            "https://a.example/1",
            "https://b.example/1",
            "https://a.example/2",
        ]))
        .unwrap();

    // Two hosts, so two requests, but one outcome per URL.
    assert_eq!(mock.calls(), 2);
    assert_eq!(outcomes.len(), 3);
    assert!(outcomes.iter().all(|o| o.succeeded()));
    assert!(outcomes.iter().all(|o| o.status == 200));
}

#[test]
fn sends_the_payload_indexnow_expects() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method("POST")
            .path("/indexnow")
            .header("content-type", "application/json; charset=utf-8")
            .json_body(serde_json::json!({
                "host": "example.com",
                "key": KEY,
                "urlList": ["https://example.com/a", "https://example.com/b"],
            }));
        then.status(200);
    });

    engine(&server)
        .submit(&urls(&["https://example.com/a", "https://example.com/b"]))
        .unwrap();

    mock.assert();
}

#[test]
fn treats_202_as_accepted() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("POST").path("/indexnow");
        then.status(202);
    });

    let outcomes = engine(&server)
        .submit(&urls(&["https://example.com/a"]))
        .unwrap();

    assert!(outcomes[0].succeeded());
    assert_eq!(outcomes[0].status, 202);
}

#[test]
fn explains_a_403_by_naming_the_key_file() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("POST").path("/indexnow");
        then.status(403);
    });

    let outcomes = engine(&server)
        .submit(&urls(&["https://example.com/a"]))
        .unwrap();

    let error = outcomes[0].error.as_ref().unwrap();
    assert_eq!(
        error,
        &format!("the key was not accepted; it must be readable at https://example.com/{KEY}.txt")
    );
}

#[test]
fn keeps_the_response_body_of_a_failure() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("POST").path("/indexnow");
        then.status(422).body("URL does not belong to host");
    });

    let outcomes = engine(&server)
        .submit(&urls(&["https://example.com/a"]))
        .unwrap();

    let error = outcomes[0].error.as_ref().unwrap();
    assert!(
        error.starts_with("URL does not belong to host ("),
        "{error}"
    );
    assert_eq!(outcomes[0].status, 422);
}

#[test]
fn one_failing_host_does_not_stop_the_others() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("POST")
            .path("/indexnow")
            .body_includes("good.example");
        then.status(200);
    });
    server.mock(|when, then| {
        when.method("POST")
            .path("/indexnow")
            .body_includes("bad.example");
        then.status(429);
    });

    let outcomes = engine(&server)
        .submit(&urls(&["https://bad.example/1", "https://good.example/1"]))
        .unwrap();

    assert_eq!(outcomes.len(), 2);
    assert!(!outcomes[0].succeeded(), "{:?}", outcomes[0]);
    assert!(outcomes[1].succeeded(), "{:?}", outcomes[1]);
}

/// An address nothing is listening on: bound to claim a free port, then closed.
fn closed_address() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    address
}

#[test]
fn reports_a_connection_failure_as_an_error() {
    let endpoint = format!("http://{}/indexnow", closed_address());

    let engine = IndexNow {
        key: KEY.to_string(),
        client: reqwest::blocking::Client::new(),
        endpoint,
    };

    assert!(engine.submit(&urls(&["https://example.com/a"])).is_err());
}
