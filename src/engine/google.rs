//! Google via the Indexing API.
//!
//! With a service-account file: sign a JWT, exchange it for an access token at
//! the token endpoint, then publish one notification per URL. With a bare token,
//! the exchange is skipped and the token is used directly.
//!
//! Note that the Indexing API is quota-limited to 200 URLs per day.

use std::time::{SystemTime, UNIX_EPOCH};

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
}

impl Google {
    pub fn new(credential: Credential, client: reqwest::blocking::Client) -> Self {
        Self {
            credential,
            client,
            endpoint: std::env::var(ENDPOINT_ENV).unwrap_or_else(|_| ENDPOINT.to_string()),
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
            let request = self
                .client
                .post(&self.endpoint)
                .bearer_auth(&token)
                .json(&Notification {
                    url: url.to_string(),
                    kind: "URL_UPDATED",
                })
                .build()?;

            let exchange = http::send(&self.client, request)?;
            let status = exchange.status;

            outcomes.push(Outcome {
                url: url.clone(),
                status: status.as_u16(),
                error: failure(status, &exchange.body_or("")),
            });

            // The Indexing API allows 200 URLs a day. Once it starts refusing,
            // sending the rest of a large sitemap achieves nothing.
            if status == StatusCode::TOO_MANY_REQUESTS {
                outcomes.extend(urls[index + 1..].iter().map(|url| Outcome {
                    url: url.clone(),
                    status: status.as_u16(),
                    error: Some("not attempted: submission stopped after a rate limit".to_string()),
                }));
                break;
            }
        }

        Ok(outcomes)
    }
}

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

    let explanation = match status.as_u16() {
        401 => Some("the credentials were not accepted"),
        403 => Some(
            "the service account must be an owner of the property in Search Console, and the Indexing API must be enabled for its project",
        ),
        429 => Some("rate limited; the Indexing API allows 200 URLs per day"),
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
    use base64::Engine as _;
    use std::path::PathBuf;

    const FIXTURE: &str = "tests/fixtures/credentials.json";

    fn account() -> ServiceAccount {
        ServiceAccount::load(&PathBuf::from(FIXTURE)).unwrap()
    }

    /// Decode a JWT segment without verifying anything.
    fn segment(token: &str, index: usize) -> serde_json::Value {
        let part = token.split('.').nth(index).unwrap();
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(part)
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn signs_an_rs256_assertion() {
        let token = sign(&account(), 1_000_000).unwrap();
        assert_eq!(token.split('.').count(), 3, "{token}");
        assert_eq!(segment(&token, 0)["alg"], "RS256");
    }

    #[test]
    fn the_assertion_claims_what_google_expects() {
        let token = sign(&account(), 1_000_000).unwrap();
        let claims = segment(&token, 1);

        assert_eq!(
            claims["iss"],
            "indeks-test@indeks-test.iam.gserviceaccount.com"
        );
        assert_eq!(claims["scope"], SCOPE);
        assert_eq!(claims["aud"], "https://oauth2.googleapis.com/token");
        assert_eq!(claims["iat"], 1_000_000);
        assert_eq!(claims["exp"], 1_000_000 + ASSERTION_LIFETIME);
    }

    #[test]
    fn a_bare_token_needs_no_exchange() {
        let credential = Credential::Token("ya29.test-token".to_string());
        let token = access_token(&credential, &reqwest::blocking::Client::new()).unwrap();
        assert_eq!(token, "ya29.test-token");
    }

    #[test]
    fn accepts_a_service_account_file() {
        assert!(check_credential(&Credential::File(PathBuf::from(FIXTURE))).is_ok());
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

    #[test]
    fn explains_a_rate_limit_with_the_daily_quota() {
        let error = failure(StatusCode::TOO_MANY_REQUESTS, "").unwrap();
        assert!(error.contains("200 URLs per day"), "{error}");
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
