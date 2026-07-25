# indeks

CLI to push URLs and sitemaps to search engines for indexing.

## Status

Both engines work end to end. The tool has not yet been run against the live Google or
Bing APIs — every test so far is against a local mock server.

## Install

Requires Rust 1.88 or newer.

```
cargo build --release
```

The binary lands in `target/release/indeks`. The examples below use `cargo run --`,
which works the same from a checkout.

## Usage

```
indeks <google|bing> [--url <URL>]... [--sitemap <SITEMAP>]... --credentials <TOKEN_OR_PATH> [--dry-run] [--verbose]
```

### Commands

The first positional argument selects the search engine. One is required.

| Command | Target |
| --- | --- |
| `google` | Google Indexing API (`indexing.googleapis.com`) |
| `bing` | Bing via IndexNow (`api.indexnow.org`) |
| `help` | Print help for the tool or a subcommand |

### Parameters

| Parameter | Description |
| --- | --- |
| `--url <URL>` | A single absolute `http`/`https` URL to submit. Repeatable. |
| `--sitemap <SITEMAP>` | A sitemap whose `<loc>` entries are submitted. Accepts an absolute `http`/`https` URL, a local file path, or a `file://` URL. Repeatable. |
| `--credentials <TOKEN_OR_PATH>` | An API token, or a path to a JSON credentials file. Required. |

`--url` and `--sitemap` can be combined freely, and either may be repeated, but at
least one of them is required.

A URL that appears more than once — passed twice, or passed directly and also present
in a sitemap — is submitted once.

### Flags

| Flag | Description |
| --- | --- |
| `--dry-run` | Validate the input without contacting any external system. |
| `--verbose` | Log all network activity: connection setup, TLS negotiation, headers, bodies and timings. |
| `-h`, `--help` | Print help. |
| `-V`, `--version` | Print the version. |

### Environment

| Variable | Effect |
| --- | --- |
| `RUST_LOG` | Overrides the log filter, in either verbosity. |
| `INDEKS_GOOGLE_ENDPOINT` | Replaces the Indexing API endpoint. |
| `INDEKS_INDEXNOW_ENDPOINT` | Replaces the IndexNow endpoint — another participant (Bing's own, Yandex, Seznam), or a local server. |

## Examples

Check a mixed set of inputs without touching the network:

```
$ indeks bing --url https://example.com/a \
              --sitemap tests/fixtures/sitemap.xml \
              --sitemap https://example.com/sitemap.xml \
              --credentials abcdef0123456789 \
              --dry-run
Dry run: no external system will be contacted.
Engine: Bing IndexNow
Credentials: token

URLs (1):
  https://example.com/a

Sitemaps (2):
  tests/fixtures/sitemap.xml — local file, 3 URLs
    https://example.com/
    https://example.com/about
    https://example.com/contact
  https://example.com/sitemap.xml — would be fetched and expanded

Input is valid. Remove --dry-run to submit.
```

Use a credentials file instead of a token:

```
indeks google --url https://example.com/a --credentials ./service-account.json --dry-run
```

## Validation

Input is checked before any network call, and **every** problem is reported at once
rather than one per run:

```
$ indeks google --url /relative --sitemap ftp://x.example --credentials ./nope.json
error: --url /relative: not an absolute URL
error: --sitemap ftp://x.example: scheme `ftp` is not supported; use http, https or a file path
error: --credentials ./nope.json: no such file

Consider --dry-run to validate input without contacting external systems.
```

`--credentials` is required even with `--dry-run`. A dry run is a pre-flight check, and
it is worth little if it passes on input that a real run would reject. A credentials
file is read and its JSON parsed during a dry run, since neither step contacts an
external system.

A value passed to `--credentials` is treated as a file if it exists on disk, and as a
token otherwise. A value that looks like a path but does not exist is an error rather
than a token, because treating it as one would surface later as a confusing 401.

### Sitemaps

A sitemap must be valid XML in the [sitemaps.org](https://www.sitemaps.org/protocol.html)
format, with at least one `urlset > url > loc`, and every `loc` an absolute `http` or
`https` URL. Namespace prefixes are accepted, and only that exact nesting is read — the
`<loc>` of an image or video extension is nested a level deeper and is ignored.

Sitemap index files (`<sitemapindex>`) are **not supported** and are reported as such.

Under `--dry-run`, local sitemaps are read and fully parsed, but remote ones are not
fetched; only the URL itself is checked.

## Google (Indexing API)

The credential is either a service-account JSON file, or a bare OAuth access token.

With a service-account file, `indeks` signs an RS256 JWT and exchanges it for an access
token before submitting; the file must have `type: service_account`, `client_email` and
a usable `private_key`, all of which are checked during validation. With a bare token,
the exchange is skipped and the token is used as-is.

The service account must be an **owner** of the property in Search Console, and the
Indexing API must be enabled for its project. A 403 says so.

Each URL is one request, publishing a `URL_UPDATED` notification.

### Quotas and what a 429 means

Two limits apply, per Google Cloud project:

| Quota | Default |
| --- | --- |
| Publish requests per day | 200, resetting at midnight Pacific |
| Requests per minute, all endpoints | 380 |

A 429 names which one ran out, and `indeks` reads that rather than guessing:

- **Per day** — waiting cannot help before midnight Pacific, so the run stops
  immediately. Remaining URLs are reported as not attempted.
- **Per minute**, or a 429 naming no metric — the limit clears on its own, so the
  request is retried with a doubling backoff (10s, 20s, 40s), honouring `Retry-After`
  when the server sends one. Only if it still fails does the run stop.

Note that the Indexing API is **restricted to pages with `JobPosting` structured data,
or `BroadcastEvent` inside a `VideoObject`**. Other content may be accepted and then
throttled, and quota-increase requests for it are unlikely to be granted. For an
ordinary site, submitting a sitemap through Search Console is the supported route.

`INDEKS_GOOGLE_ENDPOINT` overrides the endpoint, for testing against a local server.

## Bing (IndexNow)

The credential is an IndexNow key: 8–128 characters of `a-z`, `A-Z`, `0-9` or `-`. It
can be passed directly, or held in a JSON file as a `key` field:

```json
{ "key": "abcdef0123456789" }
```

IndexNow also requires that key to be published at `https://<host>/<key>.txt` for every
host being submitted. `indeks` cannot do that for you, and a 403 says so with the exact
URL the file must appear at.

An IndexNow key is not a secret — the protocol requires it to be public — so it is not
redacted from logs or error messages, where it is what makes a 403 diagnosable.

URLs are grouped by host, since a request may carry only one, and each host's batch is
sent separately. One host failing does not stop the others.

`INDEKS_INDEXNOW_ENDPOINT` overrides the endpoint, for pointing at another IndexNow
participant (Bing's own, Yandex, Seznam) or at a local server.

## Output

By default a successful submission logs its response code and the URL, and a failure
logs the response code and the error text:

```
$ indeks bing --url https://example.com/a --url https://example.com/b --credentials abcdef0123456789
[200] successfully submitted https://example.com/a
[200] successfully submitted https://example.com/b
```

When a run fails without `--verbose`, it closes by recommending it:

```
$ indeks bing --url https://example.com/a --credentials abcdef0123456789
[403] https://example.com/a: the key was not accepted; it must be readable at https://example.com/abcdef0123456789.txt
1 of 1 URLs were not accepted

Please consider using `--verbose` to find out more
```

Results go to stdout and logging goes to stderr, so output can be piped whatever the
verbosity. `RUST_LOG` overrides the log filter in either mode.

Verbose output can be pasted into a bug report as-is. The `Authorization` header is
always redacted, and so are both halves of Google's token exchange — the signed
assertion is as good as a token for an hour, and the answer is the token itself.

An IndexNow key is the exception: it is public by protocol, and showing it is what
makes a 403 diagnosable.

For the same reason, reqwest's raw wire dump (`connection_verbose`) is deliberately not
enabled — it would print the bytes that the redaction above removes. `--verbose` shows
the same exchange, redacted, plus connection setup and TLS negotiation.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Everything was submitted, or a dry run passed |
| `1` | Input was fine but submission failed |
| `2` | Invalid arguments, invalid input, or an unreadable sitemap |

## Development

```
cargo test
cargo clippy --all-targets
cargo fmt
```

The requirements are in `spec/initial-functionality.md` and the plan being worked
through, including the milestone numbering referenced by the `todo!`s in the source, is
in `spec/initial-implementation-plan.md`.

## License

GPL-3.0-only. See [LICENSE](LICENSE).
