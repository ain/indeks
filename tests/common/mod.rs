//! Shared test fixtures.
//!
//! The Google tests need a service-account file with a **real** RSA key, so that
//! signing is genuinely exercised rather than mocked. The key is generated here
//! rather than committed: a private key in the repository trips secret scanners
//! and cannot be taken back out of history.
//!
//! Generation is the expensive part, so it happens once per test binary and every
//! service-account file reuses the same key.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};

/// Google requires 2048 bits or more, and so does the signer.
const KEY_BITS: usize = 2048;

/// A PEM-encoded RSA private key, generated once per test binary.
fn private_key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        RsaPrivateKey::new(&mut rand::thread_rng(), KEY_BITS)
            .expect("generate an RSA private key")
            .to_pkcs8_pem(LineEnding::LF)
            .expect("encode the key as PKCS#8 PEM")
            .to_string()
    })
}

/// Write a service-account file whose token endpoint is `token_uri`, and return
/// its path.
///
/// Each call writes its own file, so tests pointing at different mock servers do
/// not tread on each other.
pub fn service_account(token_uri: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    let document = serde_json::json!({
        "type": "service_account",
        "project_id": "indeks-test",
        "private_key_id": "0000000000000000000000000000000000000000",
        "private_key": private_key(),
        "client_email": CLIENT_EMAIL,
        "client_id": "000000000000000000000",
        "auth_uri": "https://accounts.google.com/o/oauth2/auth",
        "token_uri": token_uri,
    });

    let path = std::env::temp_dir().join(format!(
        "indeks-service-account-{}-{}.json",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, serde_json::to_string_pretty(&document).unwrap())
        .expect("write the generated service account");
    path
}

/// The `client_email` in every generated service account, and so the `iss` of
/// every assertion signed with one.
pub const CLIENT_EMAIL: &str = "indeks-test@indeks-test.iam.gserviceaccount.com";

/// Google's real token endpoint, for tests that never reach it.
pub const GOOGLE_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
