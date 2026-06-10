# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

## CGT decision support — parcel-selection optimiser and pre-sale what-if (2026-06-10)

(REQUIREMENTS 2026-06-10; `docs/ato/cgt-keeping-records-shares.md` — parcel choice is the taxpayer's. Read-only; nothing persisted.)

- [ ] Parcel-selection optimiser report: given listing, account, units, sale date, price (live-fetched default per the live-valuation rules; explicit override wins) → candidate strategies (minimise current-year gain, maximise discount-eligible proportion, harvest losses first, FIFO baseline), each with per-parcel allocations and gross gain / discountable split
- [ ] Pre-sale what-if: net-capital-gain accepts a hypothetical disposal (units, proceeds, date, allocations or a named strategy) and returns the year's figures with and without it — dry run, no rows written; whole-of-income tax estimate stays out of scope
- [ ] Web UI: screens via the existing `REPORTS`/action config
- [ ] Tests: each strategy's allocation choice; what-if leaves the DB untouched; API tests
- [ ] Docs: `docs/API.md`, README Features

## Compliance alert reports — wash sales and franking at-risk foresight (2026-06-10)

(REQUIREMENTS 2026-06-10; non-blocking, pattern: MIC validation / settlement coverage.)

- [ ] Mirror the ATO wash-sale guidance (TR 2008/1 / current ATO page) into `docs/ato/` + `OVERVIEW.md` — read before implementing
- [ ] Wash-sale report: every loss-realising Sell with a Buy of the same listing within a configurable window (default 30 days), either side, across all holding accounts; writes never rejected
- [ ] Franking at-risk foresight report: each dividend whose credits are denied by the 45-day walk (with the failing window/dates), plus a contemplated-sale mode reusing the holding-period walk; surfaced near the Sell flow in the UI
- [ ] Tests: window edges; cross-account detection; denied-credit explanation matches the tax summary's denial
- [ ] Docs: `docs/API.md` (both reports), README Features

## Tax-return label mapping on the CSV exports (2026-06-10)

(REQUIREMENTS 2026-06-10.)

- [ ] Verify the current year's myTax/paper labels from the ATO instructions and mirror the label reference into `docs/ato/` (+ `OVERVIEW.md`), recording which year's form the mapping targets
- [ ] Carry the mapping on the exports themselves (second header row or label column) without changing existing columns; document the full mapping in `docs/API.md`
- [ ] Tests: export carries the labels; existing column assertions unchanged
- [ ] Docs: `docs/API.md`, README

## Interest income (2026-06-10)

(REQUIREMENTS 2026-06-10. The `income` entity is listing-keyed, so interest needs its own entity.)

- [ ] New entity `interest_income` (standard module pattern + migration): date paid, amount, currency (AUD default; ATO-rate conversion at the month paid), TFN withholding, optional `holding_account_id`, source description
- [ ] Tax summary: `interest_income` line per FY, included in `gross_assessable_investment_income` (and so netted by deductions); TFN amount joins the existing withholding line; CSV export updated
- [ ] Web UI: `ENTITIES` entry
- [ ] Tests: entity CRUD; FY aggregation + FX conversion + fail-loudly; gross/net identity
- [ ] Docs: `docs/SCHEMA.md` (incl. Relationships), `docs/API.md`, README Features

## Operational hardening — restore, off-disk backups, localhost default (2026-06-10)

(REQUIREMENTS 2026-06-10.)

- [ ] `--backup-dir` option (default: beside the DB, as today) so backups can land on another volume; scheduler/backup job honours it
- [ ] Document the restore procedure in the README and prove it with a test (backup → mutate → restore → assert pre-mutation state)
- [ ] Default `--host` changes to `127.0.0.1`; `0.0.0.0` remains opt-in and the README security note inverts accordingly
- [ ] Tests: backup lands in the configured dir; restore round-trip; default-bind assertion
- [ ] Docs: README flags table + Scheduled maintenance section

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

