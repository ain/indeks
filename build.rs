//! Generates the service-account file that the tests sign assertions with.
//!
//! The key is generated rather than committed. A private key in the repository
//! is permanent — history cannot be un-pushed — and it trips secret scanners
//! even when, as here, it belongs to no account anywhere.
//!
//! Written once per `OUT_DIR` and reused, so this costs one key generation per
//! clean build. `Cargo.toml` optimises build scripts, without which RSA key
//! generation takes the better part of a minute.

use std::path::PathBuf;

use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};

/// Google requires 2048 bits or more, and so does the signer.
const KEY_BITS: usize = 2048;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let path = out_dir.join("service-account.json");
    if path.exists() {
        return;
    }

    let key =
        RsaPrivateKey::new(&mut rand::thread_rng(), KEY_BITS).expect("generate an RSA private key");
    let pem = key
        .to_pkcs8_pem(LineEnding::LF)
        .expect("encode the key as PKCS#8 PEM");

    // Written by hand rather than with serde_json, to keep the build-dependency
    // list to what key generation actually needs. Only the PEM needs escaping.
    let document = format!(
        r#"{{
  "type": "service_account",
  "project_id": "indeks-test",
  "private_key_id": "0000000000000000000000000000000000000000",
  "private_key": "{}",
  "client_email": "indeks-test@indeks-test.iam.gserviceaccount.com",
  "client_id": "000000000000000000000",
  "auth_uri": "https://accounts.google.com/o/oauth2/auth",
  "token_uri": "https://oauth2.googleapis.com/token"
}}
"#,
        pem.replace('\n', "\\n")
    );

    std::fs::write(&path, document).expect("write the generated service account");
}
