//! Google via the Indexing API.
//!
//! With a service-account file: sign a JWT, exchange it for an access token at
//! the token endpoint, then publish one notification per URL. With a bare token,
//! the exchange is skipped and the token is used directly.
//!
//! Note that the Indexing API is quota-limited to 200 URLs per day.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::StatusCode;
use url::Url;

use crate::credentials::{Credential, ServiceAccount};
use crate::engine::{Outcome, Submitter};
use crate::error::{Error, Result};
use crate::http::{self, Redaction};
use crate::validate::ValidationError;

pub const NAME: &str = "Google Indexing API";
pub const ENDPOINT: &str = "https://indexing.googleapis.com/v3/urlNotifications:publish";
pub const SCOPE: &str = "https://www.googleapis.com/auth/indexing";

/// Environment variable that replaces [`ENDPOINT`], for testing against a local
/// server.
pub const ENDPOINT_ENV: &str = "INDEKS_GOOGLE_ENDPOINT";

const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// How long a signed assertion stays valid. Google allows up to an hour.
const ASSERTION_LIFETIME: u64 = 3600;

pub struct Google {
    pub credential: Credential,
    pub client: reqwest::blocking::Client,
    /// Where to publish. Overridden by tests; [`ENDPOINT`] everywhere else.
    pub endpoint: String,
    /// How long to wait out a rate limit that looks transient.
    pub backoff: Backoff,
}

impl Google {
    pub fn new(credential: Credential, client: reqwest::blocking::Client) -> Self {
        Self {
            credential,
            client,
            endpoint: std::env::var(ENDPOINT_ENV).unwrap_or_else(|_| ENDPOINT.to_string()),
            backoff: Backoff::default(),
        }
    }
}

impl Submitter for Google {
    fn name(&self) -> &'static str {
        NAME
    }

    fn submit(&self, urls: &[Url]) -> Result<Vec<Outcome>> {
        let token = access_token(&self.credential, &self.client)?;
        let mut outcomes = Vec::with_capacity(urls.len());

        for (index, url) in urls.iter().enumerate() {
            let exchange = self.publish(url, &token)?;
            let status = exchange.status;

            outcomes.push(Outcome {
                url: url.clone(),
                status: status.as_u16(),
                error: failure(status, &exchange.body_or("")),
            });

            // A rate limit that survived `publish` is not going to clear during
            // this run, so sending the rest of a large sitemap achieves nothing.
            if status == StatusCode::TOO_MANY_REQUESTS {
                let reason = match rate_limit(&exchange.body) {
                    RateLimit::Daily => {
                        "not attempted: the daily publish quota is spent, and it resets at midnight Pacific"
                    }
                    _ => "not attempted: still rate limited after retrying",
                };
                outcomes.extend(urls[index + 1..].iter().map(|url| Outcome {
                    url: url.clone(),
                    status: status.as_u16(),
                    error: Some(reason.to_string()),
                }));
                break;
            }
        }

        Ok(outcomes)
    }
}

impl Google {
    /// Publish one URL, retrying while the rate limit looks like it will clear.
    ///
    /// A daily-quota 429 is returned immediately: nothing frees up before
    /// midnight Pacific, so waiting only delays the report. A per-minute one is
    /// worth waiting out, and a 429 that names no metric is treated the same
    /// way — guessing "transient" costs a minute, guessing "daily" abandons a
    /// run that would have succeeded.
    fn publish(&self, url: &Url, token: &str) -> Result<http::Exchange> {
        let mut attempt = 0;

        loop {
            let request = self
                .client
                .post(&self.endpoint)
                .bearer_auth(token)
                .json(&Notification {
                    url: url.to_string(),
                    kind: "URL_UPDATED",
                })
                .build()?;

            let exchange = http::send(&self.client, request)?;

            if exchange.status != StatusCode::TOO_MANY_REQUESTS {
                return Ok(exchange);
            }

            let limit = rate_limit(&exchange.body);
            if limit == RateLimit::Daily || attempt >= self.backoff.attempts {
                return Ok(exchange);
            }

            let wait = retry_after(&exchange.headers).unwrap_or_else(|| self.backoff.wait(attempt));
            tracing::warn!(
                "rate limited ({limit}); retrying {url} in {}s",
                wait.as_secs()
            );
            std::thread::sleep(wait);
            attempt += 1;
        }
    }
}

/// Which of the Indexing API's quotas a 429 came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimit {
    /// `Publish requests per day`, 200 by default. Resets at midnight Pacific.
    Daily,
    /// A per-minute ceiling — 380 requests per project across all endpoints.
    /// Clears on its own.
    PerMinute,
    /// A 429 naming no metric this code recognises.
    Unknown,
}

impl std::fmt::Display for RateLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RateLimit::Daily => write!(f, "daily quota"),
            RateLimit::PerMinute => write!(f, "per-minute quota"),
            RateLimit::Unknown => write!(f, "unidentified quota"),
        }
    }
}

/// How long to wait between retries of a rate-limited request.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub attempts: u32,
    pub first_wait: Duration,
}

impl Default for Backoff {
    /// Doubling from 10s over three attempts spans 70 seconds, so a per-minute
    /// window is always crossed before giving up.
    fn default() -> Self {
        Self {
            attempts: 3,
            first_wait: Duration::from_secs(10),
        }
    }
}

impl Backoff {
    fn wait(&self, attempt: u32) -> Duration {
        self.first_wait * 2u32.pow(attempt.min(16))
    }
}

/// Read Google's quota message to tell a daily limit from a transient one.
///
/// The body names the metric, e.g. `limit 'Publish requests per day'`.
pub fn rate_limit(body: &str) -> RateLimit {
    let body = body.to_lowercase();
    if body.contains("per day") {
        RateLimit::Daily
    } else if body.contains("per minute") {
        RateLimit::PerMinute
    } else {
        RateLimit::Unknown
    }
}

/// The `Retry-After` header, when it is a plain number of seconds.
///
/// The HTTP-date form is ignored rather than parsed; Google sends seconds, and
/// the fallback is a sensible wait either way.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let seconds: u64 = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds.min(MAX_RETRY_AFTER)))
}

/// Cap on an honoured `Retry-After`, so a large value cannot hang the run.
const MAX_RETRY_AFTER: u64 = 120;

/// Request body for one URL notification.
#[derive(Debug, serde::Serialize)]
pub struct Notification {
    pub url: String,
    /// `URL_UPDATED` or `URL_DELETED`.
    #[serde(rename = "type")]
    pub kind: &'static str,
}

/// The JWT a service account signs to ask for an access token.
#[derive(Debug, serde::Serialize)]
struct Assertion {
    iss: String,
    scope: &'static str,
    aud: String,
    iat: u64,
    exp: u64,
}

#[derive(Debug, serde::Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// Check that a credential can be used against the Indexing API, before any
/// network call. A bare token can only be checked by using it.
pub fn check_credential(credential: &Credential) -> std::result::Result<(), ValidationError> {
    match credential {
        Credential::Token(_) => Ok(()),
        Credential::File(path) => ServiceAccount::load(path).map(|_| ()),
    }
}

/// Turn a credential into a bearer token, signing and exchanging a JWT when the
/// credential is a service-account file.
pub fn access_token(credential: &Credential, client: &reqwest::blocking::Client) -> Result<String> {
    match credential {
        Credential::Token(token) => Ok(token.clone()),
        Credential::File(path) => {
            let account = ServiceAccount::load(path)?;
            let assertion = sign(&account, now())?;
            exchange(&account, &assertion, client)
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

/// Sign the assertion that the token endpoint trades for an access token.
fn sign(account: &ServiceAccount, issued_at: u64) -> Result<String> {
    let claims = Assertion {
        iss: account.client_email.clone(),
        scope: SCOPE,
        aud: account.token_uri.clone(),
        iat: issued_at,
        exp: issued_at + ASSERTION_LIFETIME,
    };

    // The key already parsed during validation, so a failure here is unexpected
    // rather than a user error, but it is still reported as a credential problem.
    let key = EncodingKey::from_rsa_pem(account.private_key.as_bytes())
        .map_err(|source| credentials_error(account, format!("cannot be used: {source}")))?;

    jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &key).map_err(|source| {
        credentials_error(account, format!("could not sign a token: {source}")).into()
    })
}

/// Trade a signed assertion for an access token.
fn exchange(
    account: &ServiceAccount,
    assertion: &str,
    client: &reqwest::blocking::Client,
) -> Result<String> {
    let request = client
        .post(&account.token_uri)
        .form(&[("grant_type", GRANT_TYPE), ("assertion", assertion)])
        .build()?;

    // Both halves are secrets: the assertion is as good as a token for an hour,
    // and the answer is the token itself.
    let exchange = http::send_with(client, request, Redaction::both())?;

    if !exchange.status.is_success() {
        return Err(Error::Submission(format!(
            "the token endpoint at {} answered {}: {}",
            account.token_uri,
            exchange.status,
            message(&exchange.body).unwrap_or_else(|| exchange.body_or("no explanation given"))
        )));
    }

    serde_json::from_str::<TokenResponse>(&exchange.body)
        .map(|response| response.access_token)
        .map_err(|source| {
            Error::Submission(format!(
                "the token endpoint at {} did not return an access token: {source}",
                account.token_uri
            ))
        })
}

fn credentials_error(account: &ServiceAccount, reason: String) -> ValidationError {
    ValidationError::Credentials {
        value: account.client_email.clone(),
        reason: format!("its private key {reason}"),
    }
}

/// Pull `error.message` out of a Google API error body.
fn message(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("error")?
        .get("message")?
        .as_str()
        .map(str::to_string)
}

/// Turn a response into an error message, or `None` when it was accepted.
fn failure(status: StatusCode, body: &str) -> Option<String> {
    if status.is_success() {
        return None;
    }

    let explanation: Option<String> = match status.as_u16() {
        401 => Some("the credentials were not accepted".to_string()),
        403 => Some(
            "the service account must be an owner of the property in Search Console, and the Indexing API must be enabled for its project"
                .to_string(),
        ),
        // Which quota ran out decides what the user should do next, so the
        // explanation follows the metric Google names rather than assuming one.
        429 => Some(
            match rate_limit(body) {
                RateLimit::Daily => "the daily publish quota is spent; it resets at midnight Pacific",
                RateLimit::PerMinute => {
                    "the per-minute quota is spent; it clears on its own within a minute"
                }
                RateLimit::Unknown => "rate limited",
            }
            .to_string(),
        ),
        _ => None,
    };

    let reported = message(body).unwrap_or_else(|| body.trim().to_string());

    Some(match (reported.is_empty(), explanation) {
        (true, Some(explanation)) => explanation.to_string(),
        (true, None) => "the server gave no explanation".to_string(),
        (false, Some(explanation)) => format!("{reported} ({explanation})"),
        (false, None) => reported,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_bare_token_needs_no_exchange() {
        let credential = Credential::Token("ya29.test-token".to_string());
        let token = access_token(&credential, &reqwest::blocking::Client::new()).unwrap();
        assert_eq!(token, "ya29.test-token");
    }

    #[test]
    fn rejects_a_file_that_is_not_a_service_account() {
        let credential = Credential::File(PathBuf::from("tests/fixtures/indexnow-key.json"));
        let error = check_credential(&credential).unwrap_err();
        assert!(error.to_string().contains("has no `type`"), "{error}");
    }

    #[test]
    fn names_itself_for_output() {
        let engine = Google::new(
            Credential::Token("ya29.test".to_string()),
            reqwest::blocking::Client::new(),
        );
        assert_eq!(engine.name(), NAME);
        assert_eq!(engine.name(), "Google Indexing API");
    }

    #[test]
    fn accepts_a_published_notification() {
        assert!(failure(StatusCode::OK, "{}").is_none());
    }

    #[test]
    fn reports_the_message_google_gives() {
        let body = r#"{"error":{"code":403,"message":"Permission denied. Failed to verify the URL ownership.","status":"PERMISSION_DENIED"}}"#;
        let error = failure(StatusCode::FORBIDDEN, body).unwrap();
        assert!(
            error.starts_with("Permission denied. Failed to verify the URL ownership. ("),
            "{error}"
        );
        assert!(
            error.contains("owner of the property in Search Console"),
            "{error}"
        );
    }

    #[test]
    fn explains_an_unauthorised_response() {
        let error = failure(StatusCode::UNAUTHORIZED, "").unwrap();
        assert_eq!(error, "the credentials were not accepted");
    }

    /// The message from the run against tekkie.dev on 2026-07-25.
    const DAILY_QUOTA_BODY: &str = r#"{"error":{"code":429,"message":"Quota exceeded for quota metric 'Publish requests' and limit 'Publish requests per day' of service 'indexing.googleapis.com' for consumer 'project_number:925833730953'.","status":"RESOURCE_EXHAUSTED"}}"#;

    const PER_MINUTE_BODY: &str = r#"{"error":{"code":429,"message":"Quota exceeded for quota metric 'Requests' and limit 'Requests per minute' of service 'indexing.googleapis.com' for consumer 'project_number:925833730953'.","status":"RESOURCE_EXHAUSTED"}}"#;

    #[test]
    fn reads_the_daily_quota_out_of_googles_message() {
        assert_eq!(rate_limit(DAILY_QUOTA_BODY), RateLimit::Daily);
    }

    #[test]
    fn reads_a_per_minute_quota_out_of_googles_message() {
        assert_eq!(rate_limit(PER_MINUTE_BODY), RateLimit::PerMinute);
    }

    #[test]
    fn an_unrecognised_429_names_no_quota() {
        assert_eq!(rate_limit(""), RateLimit::Unknown);
        assert_eq!(rate_limit("too many requests"), RateLimit::Unknown);
    }

    #[test]
    fn a_daily_limit_says_when_it_resets() {
        let error = failure(StatusCode::TOO_MANY_REQUESTS, DAILY_QUOTA_BODY).unwrap();
        assert!(error.contains("resets at midnight Pacific"), "{error}");
        assert!(error.contains("Quota exceeded for quota metric"), "{error}");
    }

    #[test]
    fn a_per_minute_limit_is_not_described_as_a_daily_one() {
        let error = failure(StatusCode::TOO_MANY_REQUESTS, PER_MINUTE_BODY).unwrap();
        assert!(error.contains("clears on its own"), "{error}");
        assert!(!error.contains("midnight Pacific"), "{error}");
    }

    #[test]
    fn an_unidentified_429_claims_nothing_about_which_quota() {
        let error = failure(StatusCode::TOO_MANY_REQUESTS, "").unwrap();
        assert_eq!(error, "rate limited");
    }

    #[test]
    fn backoff_doubles_and_spans_a_minute() {
        let backoff = Backoff::default();
        assert_eq!(backoff.wait(0), Duration::from_secs(10));
        assert_eq!(backoff.wait(1), Duration::from_secs(20));
        assert_eq!(backoff.wait(2), Duration::from_secs(40));

        let total: u64 = (0..backoff.attempts)
            .map(|n| backoff.wait(n).as_secs())
            .sum();
        assert!(total > 60, "retries span only {total}s");
    }

    #[test]
    fn honours_retry_after_in_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "30".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(30)));
    }

    #[test]
    fn caps_an_outlandish_retry_after() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "99999".parse().unwrap());
        assert_eq!(
            retry_after(&headers),
            Some(Duration::from_secs(MAX_RETRY_AFTER))
        );
    }

    #[test]
    fn ignores_a_retry_after_that_is_not_a_number() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after(&headers), None);

        // The HTTP-date form is deliberately not parsed.
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after(&headers), None);
    }

    #[test]
    fn falls_back_when_there_is_nothing_to_report() {
        let error = failure(StatusCode::INTERNAL_SERVER_ERROR, "").unwrap();
        assert_eq!(error, "the server gave no explanation");
    }

    #[test]
    fn passes_through_a_body_that_is_not_google_json() {
        let error = failure(StatusCode::BAD_GATEWAY, "upstream down").unwrap();
        assert_eq!(error, "upstream down");
    }

    #[test]
    fn serialises_the_notification_the_way_google_expects() {
        let json = serde_json::to_string(&Notification {
            url: "https://example.com/a".to_string(),
            kind: "URL_UPDATED",
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"url":"https://example.com/a","type":"URL_UPDATED"}"#
        );
    }
}
