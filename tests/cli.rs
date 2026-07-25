//! End-to-end checks of the real binary.
//!
//! Nothing here reaches the public internet: tests either pass `--dry-run`, fail
//! during validation, or point `INDEKS_INDEXNOW_ENDPOINT` at a local mock server.

use assert_cmd::Command;
use httpmock::MockServer;
use indeks::engine::google::ENDPOINT_ENV as GOOGLE_ENDPOINT_ENV;
use indeks::engine::indexnow::ENDPOINT_ENV;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

const KEY: &str = "abcdef0123456789abcdef0123456789";
const CREDENTIALS_FILE: &str = concat!(env!("OUT_DIR"), "/service-account.json");
const SITEMAP_FILE: &str = "tests/fixtures/sitemap.xml";

fn indeks() -> Command {
    Command::cargo_bin("indeks").unwrap()
}

#[test]
fn dry_run_reports_a_valid_single_url() {
    indeks()
        .args([
            "bing",
            "--url",
            "https://example.com/a",
            "--credentials",
            KEY,
        ])
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(contains("Dry run: no external system will be contacted."))
        .stdout(contains("Engine: Bing IndexNow"))
        .stdout(contains("https://example.com/a"))
        .stdout(contains("Input is valid."));
}

#[test]
fn dry_run_never_echoes_the_token() {
    indeks()
        .args([
            "bing",
            "--url",
            "https://example.com/a",
            "--credentials",
            KEY,
        ])
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(contains(KEY).not());
}

#[test]
fn accepts_repeated_urls_and_sitemaps_together() {
    indeks()
        .args([
            "google",
            "--url",
            "https://example.com/a",
            "--url",
            "https://example.com/b",
            "--sitemap",
            "https://example.com/sitemap.xml",
            "--sitemap",
            SITEMAP_FILE,
            "--credentials",
            CREDENTIALS_FILE,
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(contains("Engine: Google Indexing API"))
        .stdout(contains("URLs (2):"))
        .stdout(contains("Sitemaps (2):"))
        .stdout(contains("would be fetched and expanded"))
        .stdout(contains("local file"));
}

#[test]
fn dry_run_expands_a_local_sitemap() {
    indeks()
        .args([
            "bing",
            "--sitemap",
            SITEMAP_FILE,
            "--credentials",
            KEY,
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(contains("local file, 3 URLs"))
        .stdout(contains("https://example.com/about"));
}

#[test]
fn dry_run_rejects_a_local_sitemap_index() {
    indeks()
        .args([
            "bing",
            "--sitemap",
            "tests/fixtures/sitemap-index.xml",
            "--credentials",
            KEY,
            "--dry-run",
        ])
        .assert()
        .code(2)
        .stderr(contains("sitemap index files are not supported"));
}

#[test]
fn submits_and_reports_each_url() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("POST").path("/indexnow");
        then.status(200);
    });

    indeks()
        .env(ENDPOINT_ENV, server.url("/indexnow"))
        .args([
            "bing",
            "--url",
            "https://example.com/a",
            "--url",
            "https://example.com/b",
            "--credentials",
            KEY,
        ])
        .assert()
        .success()
        .stdout(contains(
            "[200] successfully submitted https://example.com/a",
        ))
        .stdout(contains(
            "[200] successfully submitted https://example.com/b",
        ))
        // Without --verbose the exchange is not logged at all.
        .stderr(contains("> POST").not())
        .stderr(contains("< HTTP").not());
}

#[test]
fn submits_urls_expanded_from_a_sitemap() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method("POST")
            .path("/indexnow")
            .body_includes("https://example.com/contact");
        then.status(200);
    });

    indeks()
        .env(ENDPOINT_ENV, server.url("/indexnow"))
        .args(["bing", "--sitemap", SITEMAP_FILE, "--credentials", KEY])
        .assert()
        .success()
        .stdout(contains("successfully submitted https://example.com/about"));

    mock.assert();
}

#[test]
fn a_refused_url_fails_the_run_and_advises_verbose() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("POST").path("/indexnow");
        then.status(403);
    });

    indeks()
        .env(ENDPOINT_ENV, server.url("/indexnow"))
        .args([
            "bing",
            "--url",
            "https://example.com/a",
            "--credentials",
            KEY,
        ])
        .assert()
        .code(1)
        .stdout(contains(
            "[403] https://example.com/a: the key was not accepted",
        ))
        .stderr(contains("1 of 1 URLs were not accepted"))
        .stderr(contains(
            "Please consider using `--verbose` to find out more",
        ));
}

#[test]
fn a_failing_verbose_run_does_not_advise_verbose_again() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("POST").path("/indexnow");
        then.status(429).body("slow down, too many requests");
    });

    indeks()
        .env(ENDPOINT_ENV, server.url("/indexnow"))
        .args([
            "bing",
            "--url",
            "https://example.com/a",
            "--credentials",
            KEY,
            "--verbose",
        ])
        .assert()
        .code(1)
        .stderr(contains("Please consider using `--verbose`").not())
        // Both halves of the exchange should be logged, bodies included.
        .stderr(contains("> POST"))
        .stderr(contains(r#"> {"host":"example.com""#))
        .stderr(contains("< HTTP"))
        .stderr(contains("< slow down, too many requests"));
}

#[test]
fn rejects_a_malformed_indexnow_key_before_submitting() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method("POST").path("/indexnow");
        then.status(200);
    });

    indeks()
        .env(ENDPOINT_ENV, server.url("/indexnow"))
        .args([
            "bing",
            "--url",
            "https://example.com/a",
            "--credentials",
            "short",
        ])
        .assert()
        .code(2)
        .stderr(contains("must be 8-128 characters long"));

    assert_eq!(mock.calls(), 0, "nothing should have been submitted");
}

#[test]
fn a_verbose_google_run_leaks_neither_assertion_nor_token() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("POST").path("/token");
        then.status(200).json_body(serde_json::json!({
            "access_token": "ya29.secret-access-token",
            "expires_in": 3599,
            "token_type": "Bearer",
        }));
    });
    server.mock(|when, then| {
        when.method("POST").path("/publish");
        then.status(200);
    });

    // A service account whose token endpoint is the mock server.
    let mut account: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(CREDENTIALS_FILE).unwrap()).unwrap();
    account["token_uri"] = serde_json::json!(server.url("/token"));
    let path = std::env::temp_dir().join("indeks-cli-service-account.json");
    std::fs::write(&path, serde_json::to_string(&account).unwrap()).unwrap();

    let assert = indeks()
        .env(GOOGLE_ENDPOINT_ENV, server.url("/publish"))
        .args([
            "google",
            "--url",
            "https://example.com/a",
            "--credentials",
            path.to_str().unwrap(),
            "--verbose",
        ])
        .assert()
        .success()
        .stdout(contains("successfully submitted https://example.com/a"));

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        !stderr.contains("ya29.secret-access-token"),
        "the access token reached the logs:\n{stderr}"
    );
    assert!(
        !stderr.contains("assertion=ey"),
        "the signed assertion reached the logs:\n{stderr}"
    );
    assert!(
        stderr.contains("[redacted]"),
        "the redaction marker is missing:\n{stderr}"
    );
}

#[test]
fn requires_a_target() {
    indeks()
        .args(["bing", "--credentials", KEY])
        .assert()
        .code(2)
        .stderr(contains("at least one --url or --sitemap is required"));
}

#[test]
fn requires_credentials() {
    indeks()
        .args(["bing", "--url", "https://example.com/a"])
        .assert()
        .code(2)
        .stderr(contains("--credentials is required"));
}

#[test]
fn rejects_a_relative_url() {
    indeks()
        .args(["bing", "--url", "/page", "--credentials", KEY])
        .assert()
        .code(2)
        .stderr(contains("--url /page: not an absolute URL"));
}

#[test]
fn reports_every_problem_in_one_run() {
    let assert = indeks()
        .args(["bing", "--url", "/a", "--url", "ftp://example.com"])
        .assert()
        .code(2);

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    let errors = stderr.lines().filter(|l| l.starts_with("error:")).count();
    assert_eq!(
        errors, 3,
        "expected both URLs and the credentials to be reported\n{stderr}"
    );
}

#[test]
fn advises_dry_run_on_validation_failure() {
    indeks()
        .args(["bing", "--url", "/page", "--credentials", KEY])
        .assert()
        .code(2)
        .stderr(contains("Consider --dry-run"));
}

#[test]
fn does_not_advise_dry_run_when_already_dry_running() {
    indeks()
        .args(["bing", "--url", "/page", "--credentials", KEY, "--dry-run"])
        .assert()
        .code(2)
        .stderr(contains("Consider --dry-run").not());
}

#[test]
fn rejects_a_credentials_path_that_does_not_exist() {
    indeks()
        .args([
            "bing",
            "--url",
            "https://example.com/a",
            "--credentials",
            "./missing.json",
            "--dry-run",
        ])
        .assert()
        .code(2)
        .stderr(contains("--credentials ./missing.json: no such file"));
}

#[test]
fn rejects_a_credentials_file_that_is_not_json() {
    indeks()
        .args([
            "bing",
            "--url",
            "https://example.com/a",
            "--credentials",
            "tests/fixtures/malformed.json",
            "--dry-run",
        ])
        .assert()
        .code(2)
        .stderr(contains("is not valid JSON"));
}

#[test]
fn rejects_an_unknown_engine() {
    indeks()
        .args(["yandex", "--url", "https://example.com/a"])
        .assert()
        .code(2);
}

#[test]
fn requires_an_engine() {
    indeks().assert().code(2);
}

#[test]
fn help_lists_the_engines() {
    indeks()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("google"))
        .stdout(contains("bing"));
}
