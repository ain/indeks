//! Classification and loading of `--credentials`.

use std::path::{Path, PathBuf};

use crate::validate::ValidationError;

/// What the user passed to `--credentials`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// An API token used as-is: an IndexNow key, or a Google access token.
    Token(String),

    /// A JSON credentials file on disk.
    File(PathBuf),
}

impl Credential {
    /// Decide whether the raw value names a file or is a token, and check it.
    ///
    /// A value that exists on disk is a file, and is read and parsed as JSON
    /// here — neither step contacts an external system, so both happen under
    /// `--dry-run` too. Engine-specific field checks belong to the engine.
    ///
    /// A value that only *looks* like a path — it contains a separator or ends
    /// in `.json` — but does not exist is an error, because treating it as a
    /// token would surface later as a confusing 401 from the API.
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        if raw.trim().is_empty() {
            return Err(error(raw, "is empty"));
        }

        let path = Path::new(raw);
        if path.is_file() {
            check_json(path)?;
            return Ok(Credential::File(path.to_path_buf()));
        }

        if path.is_dir() {
            return Err(error(raw, "is a directory, not a credentials file"));
        }

        if looks_like_path(raw) {
            return Err(error(raw, "no such file"));
        }

        Ok(Credential::Token(raw.to_string()))
    }
}

/// Whether a value that does not exist on disk was nonetheless meant as a path.
///
/// Both separators are checked whatever the platform: Windows accepts either,
/// and a Windows-shaped path pasted on Unix is still clearly not a token.
fn looks_like_path(raw: &str) -> bool {
    raw.contains('/') || raw.contains('\\') || raw.ends_with(".json")
}

fn error(value: &str, reason: impl Into<String>) -> ValidationError {
    ValidationError::Credentials {
        value: value.to_string(),
        reason: reason.into(),
    }
}

/// Read a credentials file and confirm it holds valid JSON.
fn check_json(path: &Path) -> Result<(), ValidationError> {
    let raw = path.display().to_string();
    let contents = std::fs::read_to_string(path)
        .map_err(|source| error(&raw, format!("could not be read: {source}")))?;
    serde_json::from_str::<serde_json::Value>(&contents)
        .map_err(|source| error(&raw, format!("is not valid JSON: {source}")))?;
    Ok(())
}

/// The fields `indeks` needs from a Google service-account JSON file.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ServiceAccount {
    pub client_email: String,
    pub private_key: String,
    #[serde(default = "default_token_uri")]
    pub token_uri: String,
}

fn default_token_uri() -> String {
    "https://oauth2.googleapis.com/token".to_string()
}

impl ServiceAccount {
    /// Read and check a service-account file, requiring
    /// `type: service_account`, `client_email` and `private_key`.
    ///
    /// The private key is parsed here as well, so an unusable one is reported
    /// during validation rather than at signing time.
    pub fn load(path: &Path) -> Result<Self, ValidationError> {
        let raw = path.display().to_string();
        let contents = std::fs::read_to_string(path)
            .map_err(|source| error(&raw, format!("could not be read: {source}")))?;
        let json: serde_json::Value = serde_json::from_str(&contents)
            .map_err(|source| error(&raw, format!("is not valid JSON: {source}")))?;

        match json.get("type").and_then(serde_json::Value::as_str) {
            Some("service_account") => {}
            Some(other) => {
                return Err(error(
                    &raw,
                    format!("has type `{other}`; Google needs a service-account file"),
                ));
            }
            None => {
                return Err(error(
                    &raw,
                    "has no `type`; Google needs a service-account file",
                ));
            }
        }

        let account: ServiceAccount = serde_json::from_value(json).map_err(|source| {
            error(
                &raw,
                format!("is missing something a service-account file needs: {source}"),
            )
        })?;

        jsonwebtoken::EncodingKey::from_rsa_pem(account.private_key.as_bytes()).map_err(
            |source| {
                error(
                    &raw,
                    format!("has a `private_key` that cannot be used: {source}"),
                )
            },
        )?;

        Ok(account)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_FILE: &str = "tests/fixtures/credentials.json";
    const MALFORMED_FILE: &str = "tests/fixtures/malformed.json";

    #[test]
    fn treats_a_bare_string_as_a_token() {
        assert_eq!(
            Credential::parse("abcdef0123456789").unwrap(),
            Credential::Token("abcdef0123456789".to_string())
        );
    }

    #[test]
    fn treats_an_existing_file_as_a_file() {
        assert_eq!(
            Credential::parse(VALID_FILE).unwrap(),
            Credential::File(PathBuf::from(VALID_FILE))
        );
    }

    #[test]
    fn rejects_a_missing_path_rather_than_calling_it_a_token() {
        // One value per clause of `looks_like_path`: extension, forward slash,
        // backslash.
        for value in [
            "missing.json",
            "/etc/indeks/nope",
            r"C:\keys\indeks",
            "./nope.json",
        ] {
            let error = Credential::parse(value).unwrap_err();
            assert!(
                error.to_string().contains("no such file"),
                "{value}: {error}"
            );
        }
    }

    #[test]
    fn a_service_account_without_a_token_uri_uses_googles() {
        let account: ServiceAccount = serde_json::from_str(
            r#"{"client_email": "a@b.example", "private_key": "unchecked here"}"#,
        )
        .unwrap();
        assert_eq!(account.token_uri, "https://oauth2.googleapis.com/token");
    }

    #[test]
    fn a_service_account_keeps_its_own_token_uri() {
        let account: ServiceAccount = serde_json::from_str(
            r#"{"client_email": "a@b.example", "private_key": "x", "token_uri": "https://example.com/token"}"#,
        )
        .unwrap();
        assert_eq!(account.token_uri, "https://example.com/token");
    }

    #[test]
    fn rejects_a_file_that_is_not_json() {
        let error = Credential::parse(MALFORMED_FILE).unwrap_err();
        assert!(error.to_string().contains("is not valid JSON"), "{error}");
    }

    #[test]
    fn rejects_an_empty_value() {
        assert!(
            Credential::parse("   ")
                .unwrap_err()
                .to_string()
                .contains("is empty")
        );
    }

    #[test]
    fn rejects_a_directory() {
        let error = Credential::parse("tests/fixtures").unwrap_err();
        assert!(error.to_string().contains("is a directory"), "{error}");
    }
}
