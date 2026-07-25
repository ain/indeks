//! Shared HTTP client, verbose logging, and the one place a request is sent.

use std::time::{Duration, Instant};

use reqwest::blocking::{Client, Request};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use reqwest::{StatusCode, Version};
use tracing::Level;
use tracing_subscriber::EnvFilter;

use crate::error::Result;

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// How long a single request may take, including the response body.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Header values that are never logged, whatever the verbosity.
const REDACTED: &str = "[redacted]";

/// Install the tracing subscriber. Logs go to stderr, leaving stdout for results.
///
/// Under `--verbose` this raises `hyper`, `reqwest` and `rustls` so that
/// connection setup, TLS negotiation and the HTTP exchange are all shown,
/// alongside the request and response detail logged by [`send`]. `RUST_LOG`
/// overrides either default.
pub fn init_tracing(verbose: bool) {
    let default = if verbose {
        // `reqwest::connect::verbose` is held at `off` on purpose: it is the raw
        // wire dump, and it would show credentials that this crate redacts.
        "indeks=trace,reqwest=trace,reqwest::connect::verbose=off,hyper=trace,\
         hyper_util=trace,rustls=debug,h2=debug"
    } else {
        "indeks=info"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(verbose)
        .with_level(verbose);

    // `try_init` rather than `init`: a second call should not abort the process.
    if verbose {
        let _ = builder.try_init();
    } else {
        let _ = builder.without_time().try_init();
    }
}

/// Build the client used for every request in a run.
///
/// Note what is deliberately *not* enabled: `connection_verbose`. It makes
/// reqwest dump raw wire bytes, which would carry the signed assertion and the
/// access token straight past the redaction in [`send_with`]. The logging here
/// shows the same exchange with the secrets removed.
pub fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(USER_AGENT)
        .timeout(TIMEOUT)
        .build()?)
}

/// A completed request and its response, with the body already read.
#[derive(Debug)]
pub struct Exchange {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
    pub elapsed: Duration,
}

impl Exchange {
    /// The response body, or a stand-in when the server sent none.
    pub fn body_or(&self, fallback: &str) -> String {
        let trimmed = self.body.trim();
        if trimmed.is_empty() {
            fallback.to_string()
        } else {
            trimmed.to_string()
        }
    }
}

/// What to log in place of a body that carries a secret.
///
/// Headers are handled separately and unconditionally; this covers exchanges
/// where the secret is in the body, such as an OAuth token request and its
/// answer.
#[derive(Debug, Default, Clone, Copy)]
pub struct Redaction<'a> {
    pub request_body: Option<&'a str>,
    pub response_body: Option<&'a str>,
}

impl Redaction<'_> {
    /// Log neither body, for an exchange that is a secret at both ends.
    pub fn both() -> Self {
        Self {
            request_body: Some(REDACTED),
            response_body: Some(REDACTED),
        }
    }
}

/// Send one request, logging both halves of the exchange.
///
/// Every request in a run goes through here, so `--verbose` covers sitemap
/// fetches as well as submissions.
pub fn send(client: &Client, request: Request) -> reqwest::Result<Exchange> {
    send_with(client, request, Redaction::default())
}

/// Send one request, hiding the parts of it named by `redaction`.
pub fn send_with(
    client: &Client,
    request: Request,
    redaction: Redaction<'_>,
) -> reqwest::Result<Exchange> {
    log_request(&request, redaction.request_body);

    let started = Instant::now();
    let response = client.execute(request)?;
    let elapsed = started.elapsed();

    let status = response.status();
    let version = response.version();
    let headers = response.headers().clone();
    let body = response.text()?;

    log_response(
        status,
        version,
        &headers,
        redaction.response_body.unwrap_or(&body),
        elapsed,
    );

    Ok(Exchange {
        status,
        headers,
        body,
        elapsed,
    })
}

fn log_request(request: &Request, body_override: Option<&str>) {
    if !tracing::enabled!(Level::DEBUG) {
        return;
    }

    let mut lines = format!(
        "> {} {} {:?}",
        request.method(),
        request.url(),
        request.version()
    );
    for (name, value) in request.headers() {
        lines.push_str(&format!("\n> {name}: {}", header_value(name, value)));
    }
    match body_override {
        Some(replacement) => lines.push_str(&format!("\n>\n> {replacement}")),
        None => {
            if let Some(body) = request.body().and_then(|body| body.as_bytes()) {
                lines.push_str(&format!("\n>\n> {}", String::from_utf8_lossy(body)));
            }
        }
    }

    tracing::debug!("{lines}");
}

fn log_response(
    status: StatusCode,
    version: Version,
    headers: &HeaderMap,
    body: &str,
    elapsed: Duration,
) {
    if !tracing::enabled!(Level::DEBUG) {
        return;
    }

    let mut lines = format!("< {version:?} {status} ({elapsed:.1?})");
    for (name, value) in headers {
        lines.push_str(&format!("\n< {name}: {}", header_value(name, value)));
    }
    if !body.is_empty() {
        lines.push_str(&format!("\n<\n< {body}"));
    }

    tracing::debug!("{lines}");
}

/// Render a header for logging, hiding credentials.
///
/// `--verbose` is meant to be pasted into a bug report, so the bearer token is
/// never part of it even though everything else is.
fn header_value(name: &HeaderName, value: &HeaderValue) -> String {
    if name == AUTHORIZATION {
        return REDACTED.to_string();
    }
    value.to_str().unwrap_or("<not valid UTF-8>").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_the_authorization_header() {
        let value = HeaderValue::from_static("Bearer ya29.a0AfH6SMB-secret");
        assert_eq!(header_value(&AUTHORIZATION, &value), REDACTED);
    }

    #[test]
    fn logs_other_headers_as_they_are() {
        let name = HeaderName::from_static("content-type");
        let value = HeaderValue::from_static("application/json");
        assert_eq!(header_value(&name, &value), "application/json");
    }

    #[test]
    fn falls_back_when_the_body_is_empty() {
        let exchange = Exchange {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: "  \n ".to_string(),
            elapsed: Duration::ZERO,
        };
        assert_eq!(exchange.body_or("no response body"), "no response body");
    }

    #[test]
    fn trims_a_body_that_is_present() {
        let exchange = Exchange {
            status: StatusCode::FORBIDDEN,
            headers: HeaderMap::new(),
            body: "\n  key not valid\n".to_string(),
            elapsed: Duration::ZERO,
        };
        assert_eq!(exchange.body_or("unused"), "key not valid");
    }

    #[test]
    fn the_user_agent_names_the_tool_and_version() {
        assert!(USER_AGENT.starts_with("indeks/"), "{USER_AGENT}");
    }
}
