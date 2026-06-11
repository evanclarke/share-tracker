# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

## Scheduler timezone support (2026-06-10)

(Per-entry timezones in `schedule.cron` so market-close-driven jobs are expressed in the market's own timezone instead of Sydney-local approximations — the Sydney↔New York offset swings 14–16h across DST transitions, shifting the 11:30 price-import's margin over the NYSE close by two hours over the year. `chrono-tz` is already a dependency; `croner`'s `find_next_occurrence` is generic over `chrono::TimeZone`.)

- [ ] Schedule format: optional IANA timezone field between the cron expression and the job name (e.g. `30 16 * * 1-5 America/New_York price-import`); absent → local time as today; unknown zone names rejected at startup via `ScheduleError::Parse` with the line number
- [ ] `next_run` computes the occurrence in the entry's timezone; the `next run scheduled` INFO line shows the zone (`%Z`)
- [ ] DST gap/fold behaviour (e.g. a 02:30 job on the spring-forward day) covered by an explicit test
- [ ] Cap each sleep (e.g. 1h) and recompute, so a DST transition mid-sleep re-anchors the wall-clock target (pre-existing issue with `Local` too)
- [ ] `schedule.cron`: move the price-import entries to their market timezones and rewrite the Sydney-clock comment block
- [ ] Tests: parse with/without the tz field; bad zone name rejected; next-occurrence in a non-local zone; DST cases; committed `schedule.cron` still validates
- [ ] Docs: README Scheduled maintenance section (schedule file format)

## Known-limitation documentation — gifts, pre-CGT holdings, indexation (2026-06-10)

(REQUIREMENTS 2026-06-10. Documentation-only; no modelling.)

- [ ] Known limitations (docs/API.md + README): gifts / off-market related-party transfers are a disposal at market value (market-value substitution) — enterable today as a manual Sell or Buy at market value
- [ ] Known limitations: pre-CGT holdings (acquired before 20 September 1985) are outside CGT and not modelled — the system would wrongly compute gains on such a parcel
- [ ] Known limitations: the indexation method (pre-21 September 1999 acquisitions, frozen at Sep 1999) is not modelled; the 50% discount is used throughout

