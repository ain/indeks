# Publishing to crates.io

Written 2026-07-26 against commit `1617fef`.

**Status: steps 1–5 are done** (`4654d2c`). Both blockers below are fixed, the metadata
is in, and `cargo publish --dry-run` succeeds. What remains is `cargo login`, which needs
a token and a person, then `cargo publish` and the tag.

The crate name **`indeks` is available**: `GET /api/v1/crates/indeks` returns
`crate 'indeks' does not exist`. Nothing similar is registered.

## Two blockers

Both were found by reading `cargo package --list` rather than by inspection, and
neither is obvious from the source tree.

### The package ships a log file

`tekkie-dev-rate-limited-index-api.log` is tracked in git, so `cargo package` includes
it. Publishing would put 183 tekkie.dev URLs and the Google Cloud project number
`925833730953` on crates.io permanently, in every downloaded copy.

Fix: delete it, or move it under `spec/` as evidence for the rate-limit work and exclude
that directory from the package.

### Every `cargo install` generates an RSA key it never uses

`build.rs` runs for consumers, not only for this repository's tests. Anyone installing
the crate compiles `rsa` and `num-bigint-dig` as build dependencies and then spends CPU
generating a 2048-bit test key that nothing in the binary touches. It is the direct cost
of moving the key out of the repository the way we did.

Fix: generate the key from a **dev-dependency** instead of a build dependency, in
`tests/common/mod.rs`. Removes `build.rs`, `[build-dependencies] rsa, rand`, and both
`build-override` profiles; `[profile.dev.package.rsa] opt-level = 3` keeps generation
under a second for tests. Consumers pay nothing.

### The path crate that was tried and rejected

The first design put the generator in a `testkey/` path crate listed as a
dev-dependency, so that unit tests inside the library could share it with the
integration tests. Testing what cargo actually does with such a dependency ruled it out:

- `cargo package` **succeeds** — a path-only dev-dependency is not an error.
- The published manifest contains an **empty `[dev-dependencies]`**: cargo strips a path
  dependency that carries no `version`.
- The helper crate's source is **not** in the tarball.
- Verification passes, because it builds the library and not the tests.

The consequence only appears afterwards: `tests/` still ships, but `cargo test` in the
unpacked crate fails to compile, because the dev-dependency it needs is gone. Excluding
`tests/` would have been the other way out.

Putting the generator in `tests/common/mod.rs` avoids the question. `rsa` and `rand` are
ordinary versioned dev-dependencies, so nothing is stripped and the packaged crate runs
its own suite — checked by unpacking the tarball and running it.

The cost is that **no unit test can use a real key**, since `tests/` is invisible to
`src/`. Four had to move. Two of them exercised the private `sign()`, so they were
replaced by an integration test that captures the assertion the mock token endpoint
receives and checks every claim through the public path — which is closer to what Google
actually validates anyway.

## Metadata to add

`Cargo.toml` already has `name`, `version`, `edition`, `rust-version`, `description` and
`license`. Publishing wants:

```toml
repository = "https://github.com/ain/indeks"
readme = "README.md"
keywords = ["seo", "sitemap", "indexing", "indexnow", "cli"]
categories = ["command-line-utilities", "web-programming"]
exclude = ["spec/", ".github/", "CLAUDE.md", "*.log"]
```

- `repository` is what crates.io links to. Without it the crate page has no source link.
- `keywords` are capped at five, twenty characters each. `categories` must come from
  crates.io's fixed list; both of the above are on it.
- `exclude` trims the tarball to what a consumer needs. **`tests/` stays in**, so the
  published crate can still be verified by whoever downloads it.

## Decisions to take before the first publish

### The whole library becomes public API

`src/lib.rs` exposes every module, so publishing invites dependence on
`indeks::sitemap`, `indeks::engine`, `indeks::validate` and the rest — and semver then
binds those signatures. This is not hypothetical: milestone 1 of the prioritisation work
changed `sitemap::parse` from `Vec<Url>` to `Vec<Entry>`, which would have been a
breaking release.

Either:

- accept it, and rely on `0.x` allowing breaking changes in minor releases; or
- mark the library `#[doc(hidden)]` and state in the README that only the binary is
  supported, leaving the modules free to move.

### GPL-3.0-only has more bite as a library

For a CLI the licence is unremarkable: users run it. Published as a library, anything
linking `indeks` must also be GPL-3.0. That is a deliberate position to hold, not a
reason to change — but it should be held knowingly.

## Dependency licences

Every dependency that ships is permissive, and therefore GPL-3.0 compatible.
Compatibility is one-directional — GPL-3.0 can absorb permissive code, permissive
licences cannot absorb GPL — so the combination is sound.

The authority for this is `cargo deny list`, not a hand-rolled `cargo tree` survey. A
first pass with the latter got two things wrong, which is the reason `deny.toml` exists:

- It **missed `webpki-root-certs`**, the only crate whose licence needed a decision.
- It reported "nothing copyleft in the tree" without noticing that `r-efi` offers
  LGPL-2.1-or-later — harmlessly, as one of three options, but the claim was
  luckier than it was informed.

Counting crates per licence is also less meaningful than it looks: a crate offering
`MIT OR Apache-2.0` appears under both, so the totals overlap and different counting
methods disagree. What matters is the set of licences and the crates that leave no
choice.

### The shape of the tree

Overwhelmingly `MIT OR Apache-2.0`. Beyond that:

| Licence | Where it comes from |
| --- | --- |
| Unicode-3.0 | 19 ICU crates, reached through `url` |
| ISC | the `rustls` family, `aws-lc-*`, `simple_asn1`, `untrusted` |
| CDLA-Permissive-2.0 | `webpki-root-certs` — **the only single-licence surprise** |
| BSD-3-Clause | `subtle`; also part of `encoding_rs` and `aws-lc-sys` |
| Apache-2.0 with no alternative | `sync_wrapper`, and components of `aws-lc-sys` |

### `webpki-root-certs` and CDLA-Permissive-2.0

Reached through `reqwest` → `rustls-platform-verifier`, it is the Mozilla root CA bundle:
a permissive licence covering **data** rather than code. No copyleft, and only the
disclaimer has to travel with it. Fedora treats it as allowed; it is not on the FSF's
reviewed list, so a project needing strict FSF compatibility should form its own view
rather than inherit this one.

It is allowed in `deny.toml` with that reasoning recorded next to it.

### This works because the licence is GPL-3.0 and not GPL-2.0

Apache-2.0's patent-retaliation and termination clauses are incompatible with GPL-2.0 and
explicitly compatible with GPL-3.0. `sync_wrapper` is Apache-2.0 only, and `aws-lc-sys`
carries Apache-2.0-only components. Had `Cargo.toml` said `GPL-2.0-only`, today's tree
would not be compliant. Worth remembering before ever changing the licence field.

### Attribution survives the GPL

GPL-3.0 does not discharge the notice requirements of MIT, BSD, ISC, Apache-2.0 or
Unicode-3.0. Publishing **source** to crates.io does not raise the question — each
dependency arrives with its own licence text. Distributing **binaries** does: a GitHub
release artifact, a Homebrew bottle or a container image should carry a third-party
notices file. `cargo about` generates one.

The largest notice burden is `aws-lc-sys`, whose SPDX expression joins seven licences
with `AND` because it vendors C from the AWS-LC / BoringSSL / OpenSSL lineage. It is in
the tree because of two choices: `rustls`'s default crypto provider and `jsonwebtoken`'s
`aws_lc_rs` feature. Moving both to `ring` or `rust_crypto` would shrink that surface.
Not a compliance problem — just a longer file.

### One unlicensed crate, outside what ships

`stringmetrics v2.2.2` declares **no licence field at all**, which strictly means all
rights reserved. It arrives through `httpmock`, so it is a dev-dependency: not linked
into the binary, not in the published tarball, and of no consequence to anyone
downstream. It is still unlicensed code being compiled locally and in CI.

### Enforcement

`deny.toml` states the policy and `cargo deny check` enforces it in CI, so a copyleft or
unlicensed dependency cannot arrive unnoticed. It excludes dev-dependencies, matching the
reasoning above: only distributed code is subject to licence compatibility. The trade-off
is that advisories are not checked for test-only crates.

### An advisory that `deny.toml` ignores, with reasons

`cargo deny` reports **RUSTSEC-2023-0071** against `rsa 0.9.10`: a timing sidechannel
(the Marvin attack) that can leak private key material to an attacker able to observe
operations. There is no patched release.

It was ignored while `rsa` was a *build* dependency: unreachable there, since it only
generated a throwaway key, but build-dependencies survive `exclude-dev = true` and stay
in the graph.

**Resolved.** Moving key generation to `tests/common` made `rsa` a dev-dependency, so
`exclude-dev` drops it and the advisory with it. `deny.toml` now has no ignores at all.
Two problems, one fix.

## More decisions before the first publish

### The status caveat becomes the shop window

crates.io renders `README.md` on the crate page. Its status section currently records
that **nothing has ever run against the live Google or Bing APIs** — every test is
against a local mock. That is honest and appropriate for `0.1.0`, and it should stay
near the top rather than being softened for publication.

### Publishing cannot be undone

A version can be yanked, which stops new dependants resolving it, but it can never be
deleted or replaced. `0.1.0` is spent the moment it lands, and so is the crate name.

## Steps

1. Delete or relocate `tekkie-dev-rate-limited-index-api.log`.
2. Add the metadata above to `Cargo.toml`.
3. Replace `build.rs` with generation in `tests/common/mod.rs`; drop both
   `build-override` profiles; move the four unit tests that need a real key.
4. `cargo package --list` — read every line and confirm nothing unexpected ships.
5. `cargo publish --dry-run` — builds the packaged crate from scratch, which catches
   anything that only works in this working tree.
6. `cargo test && cargo clippy --all-targets && cargo fmt --check`, and let CI go green.
7. **Run by hand:** `cargo login`. It needs a crates.io API token tied to the account,
   so it is not something to automate or delegate.
8. `cargo publish`.
9. `git tag -a v0.1.0 -m "0.1.0" && git push origin v0.1.0`.

## Afterwards, optionally

- A release workflow that publishes on tag. crates.io supports GitHub OIDC **trusted
  publishing**, which avoids keeping a long-lived token in repository secrets; the
  current setup steps should be checked against crates.io's own documentation before
  wiring it up, rather than assumed.
- A `CHANGELOG.md`. Worth starting at the first release rather than reconstructing it
  later.
- `cargo mutants` on a schedule. A full sweep took 69 minutes, so it does not belong in
  the per-push workflow, but a weekly run would catch coverage rot.

## Not part of this

- Publishing to any other registry, or shipping OS packages (Homebrew, AUR, `.deb`).
- Splitting the crate into a `indeks-core` library and an `indeks` binary. That is the
  thorough answer to the public-API question above, and it is a larger change than a
  first publish needs.
- Cross-compiled release binaries attached to GitHub releases.
