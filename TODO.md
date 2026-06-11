# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

## Known-limitation documentation — RSU dividend equivalents, foreign broker interest (2026-06-12)

(REQUIREMENTS 2026-06-12. Documentation-only; no modelling. Doc-only items are test-pinned via `src/doc_checks.rs`.)

- [ ] Known limitations: dividend equivalents on unvested RSU grants are ordinary income when paid and are not modelled — enterable manually as income if paid out in cash
- [ ] Known limitations: interest income reports at question 10 (10L) regardless of source; foreign broker-cash/money-market income strictly belongs at 20E — state the simplification



