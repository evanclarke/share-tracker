# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

## Known-limitation documentation — gifts, pre-CGT holdings, indexation (2026-06-10)

(REQUIREMENTS 2026-06-10. Documentation-only; no modelling.)

- [ ] Known limitations (docs/API.md + README): gifts / off-market related-party transfers are a disposal at market value (market-value substitution) — enterable today as a manual Sell or Buy at market value
- [ ] Known limitations: pre-CGT holdings (acquired before 20 September 1985) are outside CGT and not modelled — the system would wrongly compute gains on such a parcel
- [ ] Known limitations: the indexation method (pre-21 September 1999 acquisitions, frozen at Sep 1999) is not modelled; the 50% discount is used throughout

