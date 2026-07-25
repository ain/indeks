# Sitemap prioritisation

## Goal

Submit the most important pages first. If there are failures, the most important pages have been covered.

## Flag

`--prio` flag should be introduced as new feature with the following conditions:

- can only be used in combination with `--sitemap`
- can only be used if sitemap XML structure includes `priority` or `changefreq` node for `url` elements
- if `priority` node is defined in sitemap, `url` elements with highest `priority` values should be picked first
- if `priority` node is not defined, but `changefreq` is defined, `url` elements with most frequent change definition should be picked first
- if neither `priority` or `changefreq` are defined, execution with `--prio` flag should fail with a respective error message. If `--prio` flag is not defined, execution should follow the order in the XML (default behaviour)
- `--prio` flag should also be considered in `--dry-run` in order to validate if sitemap prioritisation is possible

