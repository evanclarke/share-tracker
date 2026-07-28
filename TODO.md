# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

## Health check: held but never priced (REQUIREMENTS 2026-07-28)
`reports::health`'s `errored_prices` only catches a listing whose fetches *fail* — a row exists
with `status = 'error'`. The case that actually bit leaves no row at all: a day that was held and
never fetched, which is silent and permanent. Listing 7 (LAC) was bought 2021-03-25 but entered
five years later, so nothing ever attempted those days; the only symptom was 544 snapshots stuck
stale over exactly 2021-03-25..2022-09-19, and by the time it was found Yahoo no longer served
`LAC` before 2023-10-02, so the range was unrecoverable. It recurs whenever a trade is entered
later than the 14-day `COLLECTION_LOOKBACK_DAYS` window on a listing not otherwise held — an
established workflow here, since entry is batched from the statement archive.
- [ ] `GET /reports/health` gains an `unpriced_days` list, the missing-row counterpart of `errored_prices`: for each date in a listing's held span, its **valuation day** (`Market::latest_trading_day_on_or_before`) has no stored row at all. Defined as exactly what `reports::valuation::stored_valuations` asks for, so there are no false positives; a day whose stored row is errored stays in `errored_prices` — the two lists partition the problem
- [ ] Exclude days whose close is not final yet (`Market::latest_complete_trading_day`), so today and an unsettled crypto candle never appear; use the same held-as-at-that-date rule as the valuation path (`closing_price::db_held_listing_ids(pool, Some(date))`), so a fully-sold listing stops being reported after its sale and a sold-then-rebought listing is covered for both spans
- [ ] Row shape mirrors `errored_prices` — `listing_id`, `ticker`, `unpriced_days`, `earliest_date`, `latest_date` — ordered by `earliest_date` so the oldest (least recoverable) hole reads first
- [ ] Read each listing's stored dates once into a set and walk its held span in memory (one query per listing, no per-day round trip), following the existing `FxRates`/`RenameHistory` pre-loading pattern — a naive per-listing-per-day walk over six years of history is thousands of iterations
- [ ] Surface on the `#/prices` screen beside the errored-price list, reusing its existing Backfill action; UI item asserted against the served bundle per the web-testing convention
- [ ] Tests: a held day with no row is reported; an errored day is *not* (it belongs to `errored_prices`); a non-trading day and a not-yet-final close are not; a fully-sold listing isn't reported for dates after its sale; a hole straddling a rename resolves its trading calendar as at the date; a fully-priced database reports an empty list
- [ ] Docs: `docs/API.md`'s Health section (the new list, its fields, and the `errored_prices`/`unpriced_days` partition), plus README's Features list if the health check is described there. No schema change and no migration — reads `trades`, `parcel_allocations`, and `closing_prices` only
- [ ] Deliberately NOT in scope: auto-backfilling what it finds. The check reports; closing the hole stays a deliberate act (`POST /closing_prices/backfill`, or a manual price for a day the provider can never serve) — a silently auto-filled hole is how the wrong series gets in

## Authentication if ever exposed beyond localhost (honourable mention)
Currently handled correctly: the server binds `127.0.0.1` by default and exposing it is an explicit
`--host` opt-in documented as unauthenticated. Only matters if the server is ever exposed.
- [ ] If/when exposure is wanted: add an auth layer (e.g. a bearer token/basic-auth middleware over the whole router) before recommending `--host 0.0.0.0` for anything but a trusted LAN; until then this section records the decision that localhost-only is the accepted posture

