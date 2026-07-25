# Resumable submissions

Status: proposed, not implemented. Follows on from
[`initial-functionality.md`](initial-functionality.md).

## Purpose

A run that is cut short by a quota currently leaves nothing behind but console output.
The URLs that never got sent are named only in prose — `not attempted: the daily publish
quota is spent` — one line each, in a stream that also contains everything that
succeeded. Nothing carries over to the next run.

That makes the next run start from the top of the sitemap, which is actively harmful
when the thing that stopped it was a daily quota: the fresh quota is spent re-sending
URLs that were already accepted, and the run stops in the same place again.

The real case, from 2026-07-25 against `https://tekkie.dev/sitemap.xml`:

| | |
| --- | --- |
| URLs in the sitemap | 183 |
| Accepted before the quota ran out | 27 |
| Reported as not attempted | 155 |
| Google's daily publish quota | 200, resetting midnight Pacific |

Re-running the same command the next day would re-submit those first 27 — 14% of a
day's quota spent on work already done — and would still not reach the tail of the
sitemap. With a sitemap even slightly larger, a site could never finish: every run
re-sends the same head, and the tail is never reached at all.

The feature exists so that **a run knows what the last one did not finish**.

## Scope

Two additions:

- `--report <path>` — write a machine-readable record of what the run did.
- `--resume <path>` — take this run's URLs from a previous report, submitting only what
  was not accepted.

They are independent: a report is useful on its own as a log, and `--resume` reads any
report, whoever wrote it.

## Behaviour

### `--report <path>`

Writes a JSON report describing every URL the run considered, whether or not the run
succeeded. Writing it is the last thing a run does, and it happens **even when the run
exits non-zero** — a failed run is exactly when the report matters.

The file is written atomically (write to a temporary file in the same directory, then
rename) so that an interrupted run cannot truncate a report from an earlier one.

Under `--dry-run` no report is written; nothing was submitted, so there is nothing to
record. Attempting both is not an error, but the run says that no report was written.

### `--resume <path>`

Reads a report and uses the URLs that were **not accepted** as this run's targets, in
their original order.

- Satisfies the "at least one `--url` or `--sitemap`" requirement on its own.
- Combines with `--url` and `--sitemap` if given: the sets are merged and deduplicated,
  exactly as those two already combine with each other.
- If every entry in the report was accepted, the run stops before contacting anything,
  reports that there is nothing left to submit, and exits `0`.
- The URLs are re-validated like any other input. A report is a file on disk that a
  user may have edited, so it is not trusted to contain well-formed absolute URLs.

Validation failures — missing file, invalid JSON, unknown version, malformed URL — are
ordinary input errors: reported with everything else, before any network call, exit `2`.

### Engine mismatch

A report records which engine produced it. Resuming a Google report against `bing` is
allowed — submitting the leftovers elsewhere is a reasonable thing to want — but the run
notes the mismatch, because it is more often a mistake than an intent.

## Report format

```json
{
  "version": 1,
  "engine": "google",
  "finished_at": 1785073018,
  "outcomes": [
    { "url": "https://tekkie.dev/browsers/safari-has-made-it-to-5",
      "status": 200, "accepted": true, "attempted": true },
    { "url": "https://tekkie.dev/flash/as/filereference-postdata-property-for-file-uploads",
      "status": 429, "accepted": false, "attempted": true,
      "error": "Quota exceeded for quota metric 'Publish requests' and limit 'Publish requests per day' … (the daily publish quota is spent; it resets at midnight Pacific)" },
    { "url": "https://tekkie.dev/telecom/mobile-speeds-doubling-up-in-northern-europe",
      "status": 429, "accepted": false, "attempted": false,
      "error": "not attempted: the daily publish quota is spent, and it resets at midnight Pacific" }
  ]
}
```

- `version` — the format's own version. An unrecognised value is rejected rather than
  guessed at, so that a later format cannot be silently half-read by an older binary.
- `engine` — `google` or `bing`.
- `finished_at` — Unix seconds. Deliberately not RFC 3339: a formatted timestamp would
  mean a date-handling dependency for one field that nothing reads programmatically.
- `outcomes` — every URL the run considered, in submission order.

### `attempted` is a field, not a message

`Outcome` currently distinguishes "refused by the API" from "never sent" only by the
wording of `error`. That is fine for printing and wrong for anything else — resuming
must not depend on matching English prose.

Implementing this feature therefore means giving `Outcome` an explicit `attempted: bool`
(or replacing the `error: Option<String>` pair with a small enum), and having the
engines set it rather than encoding the distinction in a string. The console output
stays as it is; only the type changes.

Both states resume identically — neither was accepted — but the distinction belongs in
the record. A URL Google refused with a 403 needs a fix on the site; one that was never
sent needs only another day's quota.

## Worked example

```
$ indeks google --sitemap https://tekkie.dev/sitemap.xml \
                --credentials ./service-account.json \
                --report ./tekkie-run.json
[200] successfully submitted https://tekkie.dev/…
…
27 of 183 URLs were accepted; the daily publish quota is spent
Report written to ./tekkie-run.json

$ # the next day
$ indeks google --resume ./tekkie-run.json \
                --credentials ./service-account.json \
                --report ./tekkie-run.json
156 URLs left to submit
[200] successfully submitted https://tekkie.dev/…
```

Reading and writing the same path in one run is supported, and is expected to be the
normal way to use this: the atomic write means the report is only replaced once the new
one is complete.

## Testing

- Unit: serialising and deserialising a report; rejecting an unknown `version`;
  selecting only unaccepted outcomes; preserving order.
- Integration: a run rate-limited partway writes a report whose unaccepted entries are
  exactly the URLs not accepted; resuming that report submits exactly those and no
  others; resuming a fully accepted report contacts nothing.
- CLI: `--resume` alone satisfies the target requirement; a corrupt report fails
  validation with exit `2` and no network call; `--dry-run --report` writes nothing.

## Out of scope

- Scheduling. Deciding *when* to run again is the job of cron or a CI schedule.
- Tracking remaining quota. `indeks` learns the quota is gone by being told so; it does
  not model Google's counters or try to predict how many URLs will fit.
- Splitting a sitemap across days automatically. A run submits what it is given and
  records what it could not.
- Any report-driven behaviour beyond resubmission, such as diffing runs over time.

## Relationship to the underlying problem

This makes a quota-limited workflow survivable; it does not make it good. For a site
like tekkie.dev the Indexing API is the wrong instrument — it is documented as being for
`JobPosting` and `BroadcastEvent` pages, and 200 URLs a day gives a 183-URL sitemap no
headroom. Submitting a sitemap through the Search Console API would remove the need to
resume at all for that case, and is tracked separately.

Resumability still earns its place: it applies to any interrupted run, including
transport failures and per-host IndexNow failures, not only to Google's daily quota.
