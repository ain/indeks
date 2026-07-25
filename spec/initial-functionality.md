# Initial functionality

## Tool description
`indeks` is CLI tool that the uses search engine APIs – Google Search Console or Bing IndexNow – to send across URLs for indexing.

## Supported search engines
- Google (Google Search Console)
- Bing (Bing Webmaster Tools)

## CLI features

### Flags

- `--dry-run` to test input data integrity without any interaction to external systems
- `--verbose` to log all network activity with handshakes, response headers etc. Disabled by default.

### Parameters

- `--url <url>` to send across single URL. Only absolute URLs are allowed and should be validated before transaction. Usage of `--dry-run` should be advised on vaidation errors.
- `--sitemap <sitemap url>` to send across the entire sitemap. Sitemap must follow Sitemap XML format of sitemaps.org and include at least one `urlset` > `url` > `loc` that can be parsed and included in the API payload. Sitemap must be valid XML. `loc` node must include absolute URL.
- both `--url` and `--sitemap` can be simultaneously used
- multiple `--url` and `--sitemap` parameters can be used in single command
- at least one `--url` or one `--sitemap` is required
- `--credentials` must include either a token or a path to credentials file (JSON). Path to credentials file must be valid and refer to file that uses valid JSON formatting.

## Subcommands

First positional arguments should reflect supported search engines:

- `google` for Google Search Console
- `bing` for Bing Webmaster Tools

Example: `index bing --url ...`

### Parsing and input validation

Command must fail with respective description before any network transaction (such as API call), if:

- required parameters are missing
- any of the parameters are invalid

### Response handling

By default only response codes and "successfully submitted {URL}" should be logged when transaction was successful. On failure, the response with the error text should be logged. When `--verbose` is sed, the output should be rich and include all technical information that is available from the transaction.

On failure, also a recommendation should be displayed with "Please consider using `--verbose` to find out more", but only if `--verbose` was not used by the failing execution already.
