# Sitemap prioritisation — implementation plan

Implementation plan for [`sitemap-prioritisation.md`](sitemap-prioritisation.md).
Status: planned, not implemented.

## Why this matters

This is the mitigation for the failure recorded in
[`resumable-submissions.md`](resumable-submissions.md). On 2026-07-25 a run against
`https://tekkie.dev/sitemap.xml` got 27 of 183 URLs into Google before the daily publish
quota ran out. Those 27 were simply the ones at the top of the XML.

With `--prio`, the same 27 would have been the 27 that matter. Resumability makes a
quota-limited run *survivable*; prioritisation makes each run's quota *well spent*. They
are complementary, and neither replaces the other.

## Resolved ambiguities

The spec left three points open. Decisions taken before planning:

| Question | Decision |
| --- | --- |
| `--prio` under `--dry-run`, with a remote sitemap | **Check local only.** Local sitemaps are parsed and their resulting order shown; remote ones report that prioritisation cannot be checked without fetching. Preserves the existing promise that a dry run contacts nothing. |
| Where `--url` values sort | **First**, ahead of everything from a sitemap. A URL named by hand is the strongest available signal of importance, and it carries no `priority` of its own to sort by. |
| Only some entries carry a `priority` | **Usable; a missing `priority` counts as 0.5**, which is the default [sitemaps.org](https://www.sitemaps.org/protocol.html) defines. Any sitemap with at least one `priority` can be prioritised. |

## Ordering rule

One rule is chosen **globally for the run**, across every sitemap given, rather than per
sitemap. Two sitemaps sorted by different keys cannot be merged into a single order, and
a run submits one list.

1. If any `<url>` in any sitemap declares `priority` → sort by priority, descending.
   Entries without one count as `0.5`.
2. Otherwise, if any declares `changefreq` → sort by frequency, descending:
   `always > hourly > daily > weekly > monthly > yearly > never`. Entries without one
   sort last; the protocol defines no default for `changefreq`.
3. Otherwise → the run fails with an error saying the sitemap carries neither.

Ties keep document order. The sort must be stable (`slice::sort_by` is), so that two runs
over an unchanged sitemap submit in an identical order.

Without `--prio`, order is exactly what it is today: document order, `--url` values first.

## Structural change

`sitemap::parse` returns `Vec<Url>` today and discards everything but `<loc>`. It has to
carry the sibling elements instead:

```rust
/// One `<url>` from a sitemap, with the fields prioritisation can order by.
pub struct Entry {
    pub url: Url,
    pub priority: Option<f32>,
    pub changefreq: Option<ChangeFreq>,
}

pub enum ChangeFreq {
    Always, Hourly, Daily, Weekly, Monthly, Yearly, Never,
}
```

This is the bulk of the work. `parse`, `load` and `preview` all change signature, as do
their tests and `targets::collect`. The parser's existing `<loc>`-only guard —
only the exact `urlset > url > loc` nesting counts — extends to `priority` and
`changefreq` at the same depth, so an image or video extension's fields are still
ignored.

Ordering itself belongs in a new `src/prioritise.rs`, pure and free of I/O:

```rust
pub fn order(entries: Vec<Entry>) -> Result<Vec<Url>, PrioritisationError>;
pub fn key(entries: &[Entry]) -> Option<Key>;   // Priority | ChangeFreq | None
```

## Behaviour

- `--prio` without `--sitemap` fails validation, exit `2`, before any network call.
- `--prio` combines with `--url`; those URLs are submitted first, in the order given.
- **Deduplication moves before the sort.** It currently keeps the first occurrence of a
  URL; under `--prio` a URL appearing in two sitemaps must keep its **highest** priority,
  which cannot be decided after ordering has already happened.
- Invalid `priority` values: unparseable is a validation error naming the URL;
  out of range is clamped to `0.0..=1.0` with a warning. Rejecting a 183-URL sitemap over
  one malformed number is worse than clamping it.
- Unknown `changefreq` values are treated as absent, with a warning.

## What `--dry-run` can and cannot tell you

Local sitemaps are parsed and the resulting submission order is printed, which is the
useful half of the feature — seeing what would go first.

For a **remote** sitemap, a dry run reports only that prioritisation cannot be checked
without fetching. It is worth stating plainly: **`--prio --dry-run` against a remote
sitemap cannot promise the real run will succeed.** Condition 3 above — the sitemap
carries neither `priority` nor `changefreq` — is a property of the document, so it can
only surface once the document has been fetched. That is inherent to a dry run
contacting nothing, not something the design can engineer away.

## Effect per engine

`--prio` is worth having on `google`, where each URL is a separate, quota-metered
request and the order decides what gets in before the quota runs out.

It has **almost no effect on `bing`**: IndexNow submits every URL for a host in a single
request, so ordering within the batch is invisible to the API. Combining them is not an
error — the order stays deterministic, and the flag may matter if a batch is ever split —
but nobody should expect it to change outcomes there.

## Milestones

1. `Entry` and `ChangeFreq`: parse `priority` and `changefreq`; behaviour otherwise
   unchanged, existing tests adjusted to the new signatures.
2. `prioritise.rs`: key selection, ordering, and the "not prioritisable" error. Pure
   functions, unit-tested, no I/O.
3. `--prio` flag, its validation, dedup moved ahead of the sort, wiring through
   `targets::collect`.
4. `--dry-run` reporting of the resulting order, and the remote-sitemap caveat.
5. Tests and documentation.

## Testing

- Unit: priority ordering, including missing values at `0.5`; `changefreq` ranking;
  stability of ties; key selection across mixed sitemaps; the neither-present error;
  clamping and rejection of malformed values; dedup keeping the highest priority.
- Integration: a local sitemap ordered end to end; two sitemaps merged into one order;
  a sitemap with neither field failing only on the real run.
- CLI: `--prio` without `--sitemap` exits `2` with no network call; `--prio --dry-run`
  prints the order for a local sitemap and the caveat for a remote one.

## Out of scope

- `lastmod` as an ordering key. The spec names `priority` and `changefreq` only.
- Any ordering the user supplies directly, such as a file listing URLs in preferred
  order.
- Weighting or combining the two keys. The rule picks one; it does not blend them.
- Reordering within an IndexNow batch beyond what falls out of the global sort.
