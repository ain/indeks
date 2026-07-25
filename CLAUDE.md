# indeks

CLI to push URLs and sitemaps to search engines for indexing. Rust, GPL-3.0-only.

`spec/initial-functionality.md` is the requirements document; `spec/initial-implementation-plan.md`
is the plan that was worked through, and records the reasoning behind the choices below.

## Status

Feature-complete against the initial spec. Both engines submit and report, and there are
no `todo!`s left.

**Nothing has ever run against the live Google or Bing APIs.** Every test is against a
local mock server, so the request shapes match a reading of the two protocols rather
than confirmed real-world responses. Treat the first live run as the real test.

## Stack

Rust 1.97.1, edition 2024, `rust-version = "1.88"`.

Library plus binary: `src/lib.rs` holds every module and `src/main.rs` is a thin
wrapper, so each stage is unit-testable and nothing important hides in `main`.

HTTP is **blocking** (`reqwest`, `rustls`) by design — IndexNow batches an entire run
into one request per host and Google's Indexing API allows 200 URLs per day, so there
is no concurrency worth having.

## Shape of a run

`main::run` is the whole flow, in order:

1. `cli` parses arguments. Values stay `String`s here on purpose, so that `validate` can
   report every bad one rather than clap aborting on the first.
2. `http::init_tracing` — before validation, so `--verbose` covers everything.
3. `validate::validate(args, kind)` — all input checks, collecting every failure into
   one `ValidationErrors`. Engine-specific credential rules run here too, via
   `engine::Kind::check_credential`, so a bad key fails **before** any network call.
4. `--dry-run` stops here: `sitemap::preview` parses local sitemaps, `report::dry_run`
   describes what would happen, and nothing is contacted.
5. `engine::Kind::build` produces a `Box<dyn Submitter>`.
6. `targets::collect` fetches and expands sitemaps, then dedupes across all sources.
7. `Submitter::submit` returns one `Outcome` per URL, and `report::report` prints them
   and decides the exit code.

## Conventions worth keeping

- **Every request goes through `http::send`.** That single choke point is what makes
  `--verbose` cover sitemap fetches as well as submissions.
- **Formatting logic is pure and separate from printing** (`report::lines`,
  `report::summary`, `indexnow::failure`, `google::failure`). Printing lives in thin
  wrappers. This is deliberate: `.gitignore` carries a `cargo mutants` entry, and pure
  functions are what mutation testing can actually kill.
- **Errors carry the exit code**, via `Error::exit_code`: `2` for anything about input
  (including sitemaps), `1` for a submission that failed, `0` otherwise.
- Input problems are reported all at once, each on its own `error:` line.

## Secrets

- `Authorization` is redacted from logs unconditionally, verbose or not.
- `http::send_with` takes a `Redaction` for exchanges whose *body* is a secret. Google's
  token exchange uses it at both ends: the signed assertion is as good as a token for an
  hour, and the answer is the token itself.
- reqwest's `connection_verbose` wire dump is deliberately **off**. It prints raw bytes
  and bypasses all of the above; a test in `tests/cli.rs` pins that secrets stay out of
  verbose output. Do not re-enable it.
- An IndexNow key is **not** redacted: the protocol requires it to be published at
  `https://<host>/<key>.txt`, and showing it is what makes a 403 diagnosable.

## Tests

`cargo test` runs 121 tests: unit tests beside the code, plus four integration suites.

- `tests/cli.rs` — the real binary, end to end.
- `tests/engine_google.rs`, `tests/engine_indexnow.rs`, `tests/sitemap.rs` — network
  paths against `httpmock`.

Nothing reaches the public internet. Tests either use `--dry-run`, fail during
validation, or point `INDEKS_GOOGLE_ENDPOINT` / `INDEKS_INDEXNOW_ENDPOINT` at a mock.

Tests sign with a **real RSA key**, so JWT signing is genuinely exercised rather than
mocked — but the key is never committed. `build.rs` generates one into `OUT_DIR` along
with a service-account file around it; tests reach it through
`crate::GENERATED_SERVICE_ACCOUNT` (unit tests) or `concat!(env!("OUT_DIR"), …)`
(integration tests). It is written once per `OUT_DIR` and reused.

That is why `Cargo.toml` sets `opt-level = 3` for build scripts in **both** profiles:
`rsa` key generation takes close to a minute unoptimised, and under a second optimised.

## Commands

```
cargo build
cargo test
cargo clippy --all-targets
cargo fmt
cargo mutants
cargo run -- <google|bing> --url <url> --credentials <token|path.json>
```

macOS builds emit one linker warning about compact unwind data in `aws_lc_*` symbols.
It comes from `aws-lc-rs` (the rustls crypto provider, also used by `jsonwebtoken` via
its `aws_lc_rs` feature), not from this crate, and is harmless.

## Notes for future edits

- Rewrite the sections above to match what is actually there, rather than appending.
- Sitemap index files, retries and backoff are all deliberately out of scope; see the
  plan's closing section before adding them.
