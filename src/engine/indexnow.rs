//! Bing via IndexNow.
//!
//! Every URL in a request must share one host, so URLs are grouped by host and
//! one request is sent per group (split further if a group exceeds the batch
//! limit).
//!
//! IndexNow also requires a key file hosted at `https://<host>/<key>.txt`, which
//! this tool cannot create on the user's behalf; a 403 is reported with that
//! requirement spelled out.

use std::collections::HashMap;

use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;
use url::Url;

use crate::credentials::Credential;
use crate::engine::{Outcome, Submitter};
use crate::error::Result;
use crate::http;
use crate::validate::ValidationError;

pub const NAME: &str = "Bing IndexNow";
pub const ENDPOINT: &str = "https://api.indexnow.org/indexnow";

/// Environment variable that replaces [`ENDPOINT`].
///
/// IndexNow is a shared protocol with several endpoints — Bing's own, Yandex's,
/// Seznam's — and pointing at one of those, or at a local server during testing,
/// needs no change to this crate.
pub const ENDPOINT_ENV: &str = "INDEKS_INDEXNOW_ENDPOINT";

/// URLs accepted in a single request, per the IndexNow protocol.
const MAX_BATCH: usize = 10_000;

const MIN_KEY_LENGTH: usize = 8;
const MAX_KEY_LENGTH: usize = 128;

pub struct IndexNow {
    pub key: String,
    pub client: reqwest::blocking::Client,
    /// Where to post. Overridden by tests; [`ENDPOINT`] everywhere else.
    pub endpoint: String,
}

impl IndexNow {
    pub fn new(key: String, client: reqwest::blocking::Client) -> Self {
        Self {
            key,
            client,
            endpoint: std::env::var(ENDPOINT_ENV).unwrap_or_else(|_| ENDPOINT.to_string()),
        }
    }
}

impl Submitter for IndexNow {
    fn name(&self) -> &'static str {
        NAME
    }

    fn submit(&self, urls: &[Url]) -> Result<Vec<Outcome>> {
        let mut outcomes = Vec::new();

        for (host, grouped) in group_by_host(urls) {
            for batch in grouped.chunks(MAX_BATCH) {
                let payload = Payload {
                    host: host.clone(),
                    key: self.key.clone(),
                    key_location: None,
                    url_list: batch.iter().map(Url::to_string).collect(),
                };

                let request = self
                    .client
                    .post(&self.endpoint)
                    .header(CONTENT_TYPE, "application/json; charset=utf-8")
                    .json(&payload)
                    .build()?;

                let exchange = http::send(&self.client, request)?;
                let error = failure(exchange.status, &exchange.body_or(""), &host, &self.key);

                outcomes.extend(batch.iter().map(|url| Outcome {
                    url: url.clone(),
                    status: exchange.status.as_u16(),
                    error: error.clone(),
                }));
            }
        }

        Ok(outcomes)
    }
}

/// Request body for one host's batch.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    pub host: String,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_location: Option<String>,
    pub url_list: Vec<String>,
}

/// Resolve and check the IndexNow key.
///
/// A token is the key itself. A credentials file is JSON holding a `key` field,
/// which keeps `--credentials` uniform across the two engines.
///
/// Unlike a Google bearer token, an IndexNow key is not a secret: the protocol
/// requires it to be published at `https://<host>/<key>.txt`. It is therefore
/// not redacted from logs or error messages, where it is what makes a 403
/// diagnosable.
pub fn key(credential: &Credential) -> std::result::Result<String, ValidationError> {
    let (value, raw) = match credential {
        Credential::Token(token) => (token.clone(), token.clone()),
        Credential::File(path) => {
            let display = path.display().to_string();
            let contents = std::fs::read_to_string(path).map_err(|error| {
                credentials_error(&display, format!("could not be read: {error}"))
            })?;
            let json: serde_json::Value = serde_json::from_str(&contents).map_err(|error| {
                credentials_error(&display, format!("is not valid JSON: {error}"))
            })?;
            let key = json
                .get("key")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    credentials_error(&display, "has no string `key` field, which IndexNow needs")
                })?;
            (key.to_string(), display)
        }
    };

    check_key(&value, &raw)?;
    Ok(value)
}

/// The key must be 8–128 characters of `a-z`, `A-Z`, `0-9` or `-`.
fn check_key(value: &str, raw: &str) -> std::result::Result<(), ValidationError> {
    if !(MIN_KEY_LENGTH..=MAX_KEY_LENGTH).contains(&value.chars().count()) {
        return Err(credentials_error(
            raw,
            format!("an IndexNow key must be {MIN_KEY_LENGTH}-{MAX_KEY_LENGTH} characters long"),
        ));
    }

    if let Some(bad) = value
        .chars()
        .find(|c| !c.is_ascii_alphanumeric() && *c != '-')
    {
        return Err(credentials_error(
            raw,
            format!("an IndexNow key may not contain `{bad}`; use a-z, A-Z, 0-9 or -"),
        ));
    }

    Ok(())
}

fn credentials_error(value: &str, reason: impl Into<String>) -> ValidationError {
    ValidationError::Credentials {
        value: value.to_string(),
        reason: reason.into(),
    }
}

/// Group URLs by host, since IndexNow accepts only one host per request.
///
/// Hosts keep the order in which they were first seen, so output follows input.
pub fn group_by_host(urls: &[Url]) -> Vec<(String, Vec<Url>)> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<Url>> = HashMap::new();

    for url in urls {
        let host = url.host_str().unwrap_or_default().to_string();
        match groups.get_mut(&host) {
            Some(group) => group.push(url.clone()),
            None => {
                order.push(host.clone());
                groups.insert(host, vec![url.clone()]);
            }
        }
    }

    order
        .into_iter()
        .filter_map(|host| groups.remove(&host).map(|group| (host, group)))
        .collect()
}

/// Turn a response into an error message, or `None` when it was accepted.
///
/// IndexNow answers 200 for accepted and 202 for accepted-pending-key-validation.
/// Its error codes are documented but its bodies often are not, so each known
/// status carries an explanation of its own.
fn failure(status: StatusCode, body: &str, host: &str, key: &str) -> Option<String> {
    if status.is_success() {
        return None;
    }

    let explanation = match status.as_u16() {
        400 => Some("the request was rejected as malformed".to_string()),
        403 => Some(format!(
            "the key was not accepted; it must be readable at https://{host}/{key}.txt"
        )),
        422 => Some(format!(
            "the URLs do not all belong to {host}, or the key does not match the protocol"
        )),
        429 => Some("too many requests; try again later".to_string()),
        _ => None,
    };

    Some(match (body.trim().is_empty(), explanation) {
        (true, Some(explanation)) => explanation,
        (true, None) => "the server gave no explanation".to_string(),
        (false, Some(explanation)) => format!("{body} ({explanation})"),
        (false, None) => body.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const KEY: &str = "abcdef0123456789";

    fn urls(values: &[&str]) -> Vec<Url> {
        values.iter().map(|v| Url::parse(v).unwrap()).collect()
    }

    #[test]
    fn takes_the_key_from_a_token() {
        let credential = Credential::Token(KEY.to_string());
        assert_eq!(key(&credential).unwrap(), KEY);
    }

    #[test]
    fn takes_the_key_from_a_credentials_file() {
        let credential = Credential::File(PathBuf::from("tests/fixtures/indexnow-key.json"));
        assert_eq!(key(&credential).unwrap(), "bingkey0123456789abcdef");
    }

    #[test]
    fn rejects_a_credentials_file_without_a_key_field() {
        let credential = Credential::File(PathBuf::from("tests/fixtures/credentials.json"));
        let error = key(&credential).unwrap_err();
        assert!(
            error.to_string().contains("no string `key` field"),
            "{error}"
        );
    }

    #[test]
    fn rejects_a_key_of_the_wrong_length() {
        for value in ["short", &"a".repeat(129)] {
            let error = key(&Credential::Token(value.to_string())).unwrap_err();
            assert!(
                error.to_string().contains("must be 8-128 characters long"),
                "{value}: {error}"
            );
        }
    }

    #[test]
    fn accepts_a_key_at_the_length_limits() {
        for value in ["a".repeat(8), "a".repeat(128)] {
            assert!(key(&Credential::Token(value.clone())).is_ok(), "{value}");
        }
    }

    #[test]
    fn rejects_a_key_with_unusable_characters() {
        let error = key(&Credential::Token("abcdef_0123456".to_string())).unwrap_err();
        assert!(error.to_string().contains("may not contain `_`"), "{error}");
    }

    #[test]
    fn accepts_dashes_in_a_key() {
        assert!(key(&Credential::Token("abcd-ef-0123".to_string())).is_ok());
    }

    #[test]
    fn groups_urls_by_host_keeping_first_seen_order() {
        let input = urls(&[
            "https://b.example/1",
            "https://a.example/1",
            "https://b.example/2",
        ]);
        let grouped = group_by_host(&input);

        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].0, "b.example");
        assert_eq!(grouped[0].1.len(), 2);
        assert_eq!(grouped[1].0, "a.example");
    }

    #[test]
    fn treats_a_subdomain_as_its_own_host() {
        let input = urls(&["https://example.com/a", "https://www.example.com/a"]);
        assert_eq!(group_by_host(&input).len(), 2);
    }

    #[test]
    fn serialises_the_payload_the_way_indexnow_expects() {
        let payload = Payload {
            host: "example.com".to_string(),
            key: KEY.to_string(),
            key_location: None,
            url_list: vec!["https://example.com/a".to_string()],
        };
        let json = serde_json::to_string(&payload).unwrap();

        assert!(
            json.contains(r#""urlList":["https://example.com/a"]"#),
            "{json}"
        );
        assert!(json.contains(r#""host":"example.com""#), "{json}");
        assert!(!json.contains("keyLocation"), "{json}");
    }

    #[test]
    fn accepts_200_and_202() {
        assert!(failure(StatusCode::OK, "", "example.com", KEY).is_none());
        assert!(failure(StatusCode::ACCEPTED, "", "example.com", KEY).is_none());
    }

    #[test]
    fn explains_a_403_with_the_key_file_url() {
        let error = failure(StatusCode::FORBIDDEN, "", "example.com", KEY).unwrap();
        assert_eq!(
            error,
            format!(
                "the key was not accepted; it must be readable at https://example.com/{KEY}.txt"
            )
        );
    }

    #[test]
    fn explains_a_422_by_naming_the_host() {
        let error = failure(StatusCode::UNPROCESSABLE_ENTITY, "", "example.com", KEY).unwrap();
        assert!(
            error.contains("do not all belong to example.com"),
            "{error}"
        );
    }

    #[test]
    fn explains_a_rate_limit() {
        let error = failure(StatusCode::TOO_MANY_REQUESTS, "", "example.com", KEY).unwrap();
        assert_eq!(error, "too many requests; try again later");
    }

    #[test]
    fn names_itself_for_output() {
        let engine = IndexNow::new(KEY.to_string(), reqwest::blocking::Client::new());
        assert_eq!(engine.name(), NAME);
        assert_eq!(engine.name(), "Bing IndexNow");
    }

    #[test]
    fn keeps_the_response_body_alongside_the_explanation() {
        let error = failure(
            StatusCode::BAD_REQUEST,
            "Invalid format",
            "example.com",
            KEY,
        )
        .unwrap();
        assert_eq!(
            error,
            "Invalid format (the request was rejected as malformed)"
        );
    }

    #[test]
    fn falls_back_when_an_unknown_status_has_no_body() {
        let error = failure(StatusCode::INTERNAL_SERVER_ERROR, "", "example.com", KEY).unwrap();
        assert_eq!(error, "the server gave no explanation");
    }

    #[test]
    fn passes_through_the_body_of_an_unknown_status() {
        let error = failure(StatusCode::BAD_GATEWAY, "upstream down", "example.com", KEY).unwrap();
        assert_eq!(error, "upstream down");
    }
}
