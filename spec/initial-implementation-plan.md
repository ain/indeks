# Initial implementation plan

Implementation plan for [`initial-functionality.md`](initial-functionality.md).

## Resolved ambiguities

The spec left three points open. Decisions made before planning:

| Question | Decision |
| --- | --- |
| Which Google API? | **Indexing API** (`indexing.googleapis.com/v3/urlNotifications:publish`). The Search Console API cannot submit arbitrary URLs — it only submits and lists sitemaps, which would leave `--url` meaningless for Google. |
| Which Bing API? | **IndexNow** (`api.indexnow.org/indexnow`). The spec's intro says IndexNow while the engine list says Bing Webmaster Tools; these are different APIs. IndexNow needs no OAuth, batches up to 10,000 URLs per request, and also reaches Yandex and Seznam. |
| `--dry-run` with `--sitemap`? | **No fetch.** Dry runs validate that the sitemap argument is a well-formed absolute URL and report what would happen. Local paths and `file://` arguments are fully parsed and validated, since that touches no external system. |

## Target CLI shape

```
indeks <google|bing> [--url <u>]... [--sitemap <s>]... --credentials <token|path.json> [--dry-run] [--verbose]
```

Binary name is `indeks`. The spec's example (`index bing --url ...`) is assumed to be a typo.

## Crate layout

```
Cargo.toml
src/
  main.rs          entry point, maps errors to exit codes
  cli.rs           clap derive definitions
  validate.rs      all pre-network checks, collects every error rather than failing on the first
  credentials.rs   Credential::{Token, File} classification + JSON / service-account checks
  sitemap.rs       fetch + parse urlset > url > loc
  http.rs          shared blocking client, verbose tracing layer
  report.rs        success/failure output, --verbose hint
  error.rs         thiserror types
  engine/
    mod.rs         trait Submitter { fn submit(&self, urls: &[Url]) -> Result<Vec<Outcome>> }
    google.rs      Indexing API + service-account JWT
    indexnow.rs    IndexNow
tests/
  cli.rs, sitemap.rs, engine_google.rs, engine_indexnow.rs
  fixtures/*.xml, fixtures/service-account.json
```

## Dependencies

- `clap` 4 (derive) — CLI parsing
- `url` — absolute URL validation
- `quick-xml` — streaming sitemap parsing
- `reqwest` 0.12, **blocking**, rustls + json features
- `serde` / `serde_json`
- `jsonwebtoken` 9 — RS256 signing for the Google service-account grant
- `thiserror`
- `tracing` + `tracing-subscriber` — verbose output

Dev dependencies: `assert_cmd`, `predicates`, `httpmock`.

**Blocking, not async.** IndexNow sends one POST for an entire batch, and Google's Indexing API is
capped at 200 URLs per day, so there is no concurrency to exploit. Hand-rolling the JWT grant
(roughly 40 lines) instead of pulling in `yup-oauth2` keeps the client synchronous and avoids a
large async dependency tree.

## Validation

Everything below runs before any network call.

- A subcommand is required; at least one `--url` or `--sitemap` must be present.
- Each `--url` and `--sitemap`: must parse, scheme must be `http` or `https`, host must be present.
  Error messages name the offending value.
- `--credentials`: classified as a file if the string exists on disk, otherwise as a token. A value
  that *looks* like a path (contains `/` or ends in `.json`) but does not exist is an error rather
  than a token — otherwise the misclassification surfaces later as a confusing 401. Files must be
  valid JSON; for `google` they must also carry `type: service_account`, `client_email` and
  `private_key`.
- `--credentials` is required **even under `--dry-run`**. A dry run is a pre-flight check and is
  worth little if it passes on input the real run would reject; reading a credentials file and
  parsing its JSON contacts no external system, so both happen in a dry run too.
- Failures print every error found, then
  `Consider --dry-run to validate input without contacting external systems.`

Exit codes: `0` success, `1` submission failed, `2` usage or validation error.

## Engines

### IndexNow (`bing`)

`POST https://api.indexnow.org/indexnow` with body `{host, key, keyLocation?, urlList}`.

All URLs in a single request must share a host, so URLs are grouped by host and one POST is sent per
group. The key must be 8–128 hex characters.

IndexNow also requires a key file hosted at `https://<host>/<key>.txt`, which the CLI cannot create
on the user's behalf. A 403 response therefore gets a message pointing at that requirement.

### Google Indexing API (`google`)

1. Build and sign a service-account JWT.
2. `POST https://oauth2.googleapis.com/token` — `urn:ietf:params:oauth:grant-type:jwt-bearer`,
   scope `https://www.googleapis.com/auth/indexing`.
3. `POST https://indexing.googleapis.com/v3/urlNotifications:publish` with
   `{"url": ..., "type": "URL_UPDATED"}`, one request per URL.

A bare `--credentials` token is used directly as an OAuth access token, skipping the JWT exchange.

## Sitemaps

Fetched with GET on real runs, then parsed as `urlset > url > loc`. At least one `loc` is required
and every `loc` must be an absolute URL. Any parse failure aborts before submission. URLs are
deduplicated across all `--url` and `--sitemap` inputs.

`<sitemapindex>` documents produce an explicit "sitemap index files are not supported" error rather
than a silent zero-URL run. **The spec does not cover nested sitemaps** — this is an assumption, and
supporting them is a candidate for a follow-up.

Under `--dry-run` remote sitemaps are not fetched; only URL form is checked. Local paths and
`file://` arguments are fully parsed and reported.

## Response handling

Default output is one line per URL: status code plus `successfully submitted {URL}`. Failures print
the status and the response's error text. When the run failed and `--verbose` was *not* used, a
closing line recommends `Please consider using --verbose to find out more`.

`--verbose` raises `hyper`, `reqwest` and `rustls` via `tracing-subscriber`, surfacing connection
setup, TLS negotiation and the HTTP exchange, alongside the curl-style request and response summary
that `http::send` logs with headers, body and elapsed time. `RUST_LOG` overrides the default filter
either way. Logs go to stderr so that results stay on stdout.

Notes on what verbose can and cannot show:

- The `Authorization` header is **always** redacted, verbose or not. Verbose output is meant to be
  pasteable into a bug report.
- `reqwest::connect::verbose`, the raw wire dump, is **off**. It was enabled in milestone 4 to show
  the client's default headers (`user-agent`, `accept`, `host`), which are added after a `Request`
  is built and so are missing from the `>` summary line — but milestone 6 found it printing the
  signed assertion and the access token, past every redaction. Those default headers are now
  visible nowhere; that is the accepted cost.
- Cipher-suite-level handshake detail is whatever `rustls` logs; there is no OpenSSL-style dump.

## Milestones

1. `cargo init`, dependencies, module skeleton.
2. `cli.rs` + `validate.rs` + `--dry-run` — a complete, testable, zero-network CLI.
3. `sitemap.rs` with fixture tests.
4. `http.rs` + `report.rs`.
5. IndexNow engine with `httpmock` tests.
6. Google engine, JWT signing, `httpmock` tests.
7. Test sweep and docs: keep functions small and pure so `cargo mutants` has real targets and no
   logic hides in `main`; update `CLAUDE.md` and `README.md` to match what actually landed.

## What landed differently

All seven milestones are done. Where the result differs from the plan above:

- **`src/lib.rs` was added.** The modules are a library and `src/main.rs` is a thin wrapper, so
  every stage is unit-testable and stubs never tripped `dead_code`.
- **`src/targets.rs` was added.** The plan called for cross-source deduplication but gave it no
  home; sitemap expansion and dedup live there.
- **`INDEKS_GOOGLE_ENDPOINT` and `INDEKS_INDEXNOW_ENDPOINT`** override each engine's endpoint. They
  started as a way to test the binary end to end, and the IndexNow one doubles as a real feature —
  the protocol has several participants.
- **`--credentials` accepts a JSON file for IndexNow too** (a `key` field), so the flag behaves the
  same for both engines.
- **Google submission stops on a 429** rather than spending the rest of a large sitemap on requests
  that will be refused. Unattempted URLs are still reported, marked as such.
- **reqwest 0.13 renamed its TLS feature** from `rustls-tls` to `rustls`, and `jsonwebtoken` 10
  needs an explicit crypto provider — `aws_lc_rs`, to share the one `rustls` already pulls in.

## Out of scope for the initial version

- Retries and backoff on transient failures.
- Sitemap index (nested sitemap) expansion.
- Bing Webmaster Tools `SubmitUrlBatch` as an alternative Bing backend.
- Google Search Console sitemap submission.
