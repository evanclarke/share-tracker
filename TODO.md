# TODO

Items are only marked done when a passing test exists for them.

This file holds only open / in-flight work. Completed and decided (out-of-scope / not-reproducible)
sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md). When a
section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see
CLAUDE.md.

A section records one finding, and its heading names where it came from — a REQUIREMENTS entry, a
[SCENARIOS.md](SCENARIOS.md) section, or a dated review pass.

**Open: the 2026-09-17 code review pass.** Its findings are the sections below, most-urgent first.
Both of its financial-correctness defects in the CGT arithmetic — the G1 excess's FX date and the
cost-base pipeline's single end-floor — were fixed on 2026-09-17 and moved to
[`DONE/reviews.md`](DONE/reviews.md); next are an availability panic and two write-time validation
holes, a packaging permission, tax-document/label inconsistencies, two frontend state bugs, and a
tail of low-severity concurrency, hygiene, documentation and test-gap items. The pass verified the
three gates green (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
2389 passed / 6.09 s, `node --test 'src/web/*.test.js'` 149 passed), verified the documented
`GET` → edit → `PUT` round trip byte-exact end to end, and probed the running server against a
throwaway database (~120 requests) — the write-time invariant layer, the error bodies, the body
limit, and 60-way concurrent read/write (30/30 `200`, 30/30 `204`, zero `SQLITE_BUSY`) all held. Its
findings are therefore all in corners the existing suite cannot reach, not in the surfaces it
already pins.

Before this pass, every section recorded here was closed and archived — the last was the annual tax
report's version stamp, and before it its foreign income totals (both REQUIREMENTS entries, closed
2026-08-28 and moved to [`DONE/reporting.md`](DONE/reporting.md)), and before it the 2026-08-28
cyclomatic complexity audit, whose six items (the `tax_summary` split, the two nesting outliers, the
`rights_sale` anchoring walk, the `corporate_action` presence flags, the `upsert_sell_in_tx`
parameters struct, and the decision not to gate complexity in CI) closed on 2026-08-28 and moved to
[`DONE/reviews.md`](DONE/reviews.md). The closing narrative that used to stand here — the
pass-by-pass record of driving SCENARIOS.md sections S through AA, and the last two sections to
close before that audit (the distribution calendar and the 2026-08-25 code review) — was moved to
[`DONE/verification-passes.md`](DONE/verification-passes.md) on 2026-08-28. The maintained record of
what has been verified is SCENARIOS.md's
[Verification status](SCENARIOS.md#verification-status) table and its per-section findings blocks;
the maintained record of what was built and decided is the `DONE/*.md` archive.

## A char-boundary slice in `hex_decode` could panic on non-ASCII input (unreachable from HTTP) (2026-09-17 review, nit)

(2026-09-17 review. Defensive only: the review verified the function is not reachable with non-ASCII
input, so this is hardening rather than a live bug.)

- [ ] `src/infra/auth.rs:238-246` slices `&s[i..i + 2]` after checking only that the *byte* length is
  even, so a non-ASCII string of even byte length (an emoji is four bytes) would panic on a char
  boundary
- [ ] Verified unreachable from HTTP: `session_cookie` (`:306-312`) goes through
  `HeaderValue::to_str()`, which rejects every non-visible-ASCII byte, and the only other caller is a
  test. So there is no reproducible defect to fix first
- [ ] Fix: an `s.is_ascii()` guard (or decode bytewise) so the invariant is local rather than
  dependent on a caller two modules away
- [ ] Tests: a unit case passing a non-ASCII even-byte string asserting an error rather than a panic
- [ ] Docs sync: none
