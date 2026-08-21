# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–Q are driven and every finding they raised is closed** in the `DONE/*.md`
archive. Section **R. Listing identity and renames** was driven 2026-08-21 against a throwaway
database with the real price provider. All ten scenarios behaved as designed on their own terms:
`R-01` records a ticker change, an exchange change and both at once, each taking `old_*` from the
listing's own row rather than the request; `R-02` refuses an `effective_date` on or before the
listing's newest rename; `R-03` refuses a colliding ticker and leaves nothing behind (the rename row
and the listing update are one transaction, verified on both the `(exchange_mic, ticker)` index and
the exchange-less partial one); `R-04` refuses to undo any but the newest rename, and scopes the
undo to the listing in the path; `R-05` refuses a `PUT` that would change `ticker` or `exchange_mic`
on a listing with recorded trades, naming `/rename`, while a name-only `PUT` still passes; `R-06`
splits a straddling backfill at the effective date and fetches each span under its own symbol (FB
before 2022-06-09, META from it), and the documented one-off `symbol` override recovers a span whose
old symbol the provider has retired; `R-07` demerges a renamed listing with the cost base split
exactly (A$2,000 → A$1,400 head + A$600 demerged) and the chain intact; `R-08` confirms
`price_symbol` applies to the current identity span only, so a pre-rename date still resolves its own
derived symbol; `R-09` reproduces the documented live-exchange settlement limitation to the day (an
ASX trade on 2024-03-28 settling 2024-04-03, recomputed to 2024-04-02 on a re-save after the listing
moved to XNYS); and `R-10` refuses a listing whose ticker another listing already holds. Row history
recorded every leg — the `listings` UPDATE behind each rename and the `listing_renames` DELETE behind
each undo.

**Eight findings; R-02 is closed (archived in `DONE/reference-data.md`), seven remain open.** They cluster: three are about the rename as a *write* (it is applied
before it is effective, its undo is partial, and it can move a listing to an exchange whose currency
the listing can no longer be given), two are about what the rename chain cannot express (a ticker
reused by another company; a pre-rename provider symbol), one is a report reading today's ticker
where the docs say it reads the row's own date, one is that the whole feature has no web UI, and one
is a refusal message. Each is a `## ` section below, in the order I would fix them.

## SCENARIOS R-04/R-08: undoing a rename leaves behind the price symbol the undone rename set

`POST /listings/:id/rename` can change four fields — `ticker`, `exchange_mic`, `name` and
`price_symbol`. `listing_rename::db_undo` restores **two**: it writes back `old_ticker` and
`old_exchange_mic` and deletes the row. It cannot do better, because `listing_renames` has no
`old_name`/`old_price_symbol` columns to restore from.

`price_symbol` is not cosmetic. `closing_price::yahoo_symbol_for` uses it verbatim, ahead of the
derived mapping, for any date in the listing's **current** identity — which after an undo is the
restored, older identity. So the undone rename's symbol goes on driving every subsequent fetch of
the listing.

Reproduced: listing `OLD`/`OLD.AX` → rename to `NEWER` with `"price_symbol":"NEWER.AX"`,
`"name":"Newer Co"` → `DELETE /listings/3/renames/6` → `204`. `GET /listings/3` answers
`"ticker":"NEW"` with `"name":"Newer Co"` and `"price_symbol":"NEWER.AX"`, and
`POST /closing_prices/fetch` for 2026-08-19 stored an errored row with
`fetched_symbol: "NEWER.AX"` — "yahoo fetch for NEWER.AX failed: Not found". The undo therefore
leaves the listing collecting prices under a symbol that exists only because of the rename that was
undone.

**Fix — Evan chose 2026-08-21: option (a), record what the rename overwrote.**

- [ ] Add `old_name` and `old_price_symbol` to `listing_renames` in a migration and have the undo
      write all four fields back. `listing_renames` is audited, so the migration must DROP and
      re-CREATE its two `*_row_history_*` triggers with the new column list (CLAUDE.md).
- [ ] Update `docs/SCHEMA.md` for the two columns and `docs/API.md`'s undo sentence, which today
      says the undo restores `ticker`/`exchange_mic`.

**Options as put:**

- **(a) Record what the rename overwrote and restore it.** Add `old_name` and `old_price_symbol` to
  `listing_renames` in a migration and have the undo write all four back. `listing_renames` is an
  audited table, so the migration must DROP and re-CREATE its two `*_row_history_*` triggers with
  the new column list (CLAUDE.md). Makes the undo a real undo.
- **(b) Narrow the rename to the identity it tracks.** Refuse a rename body carrying `name` or
  `price_symbol` (`422`), leaving both to `PUT /listings/:id`, which the identity freeze does not
  block for either field. Nothing then survives an undo, because the rename never changed anything
  else. Costs the convenience of one call for the LAAC → LAR shape.
- **(c) Document the undo as partial** and have it say so — no code change beyond the wording, but a
  stale `price_symbol` after an undo stays a silent, price-affecting state.

## SCENARIOS R-01: a rename can move a listing to an exchange quoting another currency, and the currency can then never be fixed

`POST /listings/:id/rename` accepts any known `exchange_mic`. Nothing compares the new exchange's
`currency` with the listing's own, so an AUD listing can be moved to XNYS and keeps `currency: AUD`
— which is the state the currency freeze on `PUT /listings/:id` exists to make unreachable. The
freeze then makes it permanent: `currency` cannot change once the listing has trades, income or
prices, and its documented remedy — "a redenomination is a new listing in the new currency plus a
transfer of the parcels to it" — is incompatible with the rename already recorded against this one.

The listing is unpriceable from that moment. Every fetch resolves the new exchange's symbol, and one
of two things happens: the provider has no such series and the day stores an errored row, or it has
one and the candle's currency is not the listing's, which the documented cross-check refuses — also
an errored row. Neither is wrong on its own terms; together they leave a holding that can never be
valued again and cannot be corrected in place.

Reproduced: `CBA` on XASX in AUD, `POST /closing_prices/fetch` for 2026-08-19 → `ok`, A$160.71 under
`CBA.AX`. `POST /listings/13/rename {"effective_date":"2026-08-20","ticker":"CBA","exchange_mic":
"XNYS"}` → `201`. The next fetch stored an errored row under `CBA`. `PUT /listings/13` correcting
`currency` to USD → `422`, "this listing's currency cannot change from AUD to USD once it has
recorded trades, income, or prices … Record a redenomination as a new listing in USD and transfer
the parcels to it".

Note the general form: `PUT /listings/:id` does not check a listing's `currency` against its
exchange's either, so a listing can be *created* in this state. The rename is where it matters
because it is the only way to reach it on a listing that already has priced history.

**Fix — Evan chose 2026-08-21: option (c), both — refuse on the rename path and warn on the rest.**

- [ ] Refuse a rename moving the listing to an exchange whose `currency` differs from the listing's
      (`422` naming both currencies and pointing at the new-listing-plus-transfer path, as the
      currency freeze already does).
- [ ] Add a `reports::health` check naming every listing whose `currency` is not its exchange's, so
      a listing created in that state by a plain `PUT` is visible too — and surface it in the UI
      banner beside the other health warnings.

**Options as put:**

- **(a) Refuse a rename that would move the listing to an exchange whose `currency` differs from the
  listing's** (`422`, naming both currencies and pointing at the new-listing-plus-transfer path, the
  way the currency freeze already does). Applies the existing rule at the one door that bypasses it.
  Risk: an exchange that genuinely quotes more than one currency (LSE quotes GBp, USD and EUR) would
  be refused a legitimate move, and `exchanges.currency` is a single column.
- **(b) Warn rather than refuse** — add a `reports::health` check naming every listing whose currency
  is not its exchange's, so the state is visible wherever it came from, including a plain `PUT`.
  Catches the created-in-this-state case (a) does not, and cannot be wrong about a multi-currency
  exchange.
- **(c) Both**: refuse on the rename path, warn on the rest.
- **(d) Document it** as a Known limitation — a cross-currency exchange change is not a rename; it is
  a new listing plus a transfer — and leave the write accepting it.

## SCENARIOS R-10: a ticker reused by a different company cannot be recorded at all

`UNIQUE(exchange_mic, ticker)` — and the partial unique index over exchange-less tickers — hold
across **all time**, while the rest of the model treats a listing's identity as time-varying. So when
an exchange reissues a delisted code to an unrelated company, the second company's listing is
refused for as long as the first holding exists, which for CGT purposes is forever: a disposed
holding's parcels, income and price history all stay.

Reproduced: listing 6 `AAA` on XASX with a 2005 Buy; `PUT /listings/7` for a second `AAA` on XASX →
`422`, "a record with these key values already exists (UNIQUE constraint failed:
listings.exchange_mic, listings.ticker)".

The only workaround available today is to record a rename that never happened — park the old listing
under an invented ticker to free the code — and that corrupts the two surfaces the rename chain
exists to serve. The invented ticker becomes the old listing's current identity, so its
current-identity price fetches resolve to a symbol that does not exist; and the fabricated event
enters the chain the Annual Tax Report reads as at each row's own date, so an archived FY document
for any year after the park prints a ticker the security never traded under. Nothing in
`docs/API.md`'s Known limitations says any of this.

**Fix — Evan chose 2026-08-21: option (a), document it as a Known limitation.**

- [ ] Add a Known limitation to `docs/API.md` stating that a listing's ticker is unique across its
      whole recorded history, so a reissued code cannot be entered while the earlier holding is on
      file — with the consequence spelled out and the fake-rename workaround explicitly ruled out
      (it falsifies the chain the Annual Tax Report reads and breaks the parked listing's price
      collection). Pin it with a `doc_checks.rs` test, as doc-only requirements are.

**Options as put:**

- **(a) Document it as a Known limitation** — a ticker is unique across the whole recorded history,
  so a reissued code cannot be entered while the earlier holding is on file; state the consequence
  and explicitly rule the fake-rename workaround out. Honest and cheap; the case stays unenterable.
- **(b) Make uniqueness time-aware** — scope it to listings whose recorded history overlaps, which
  means an end-of-life fact on the listing (the `unpriced_from` marker is nearly it) and a
  constraint SQLite cannot express as an index, so a write-time check plus a health check. Real
  modelling work, and it changes what "the listing with this ticker" means for every lookup that
  assumes one.
- **(c) Allow the collision explicitly** — drop the unique index in favour of a write-time check that
  can be overridden by a flag on the body, recording the reuse deliberately. Least machinery, but it
  removes the guard that stops an ordinary duplicate-entry mistake.

## SCENARIOS R-07: the listing activity ledger names counterpart listings at today's ticker, not the row's own date

`docs/API.md` states that reports show the current ticker throughout "except the Annual Tax Report
and the listing activity ledger, which resolve/show the ticker **as at** each row's own date", and
`domain/listing_identity.rs`'s module doc names `reports::activity` as one of the three callers of
`RenameHistory::ticker_as_at`. The activity ledger does not call it, and never has:
`reports::activity` loads `tickers: HashMap<i64, String>` from `SELECT id, ticker FROM listings` and
`describe_action` uses that map to name the scrip-exchange and demerger counterpart listings.
`RenameHistory` is not imported by the module.

Reproduced: a 2023-10-03 demerger of listing 8 into listing 9 (`SPINCO`), then a 2025-06-01 rename of
listing 9 to `SPUN`. `POST /portfolio/activity` for listing 8 prints the 2023 row as
`Demerger | 1 unit(s) of SPUN per 1 held; 30% of cost base` — a ticker that did not exist for
another two years. The Annual Tax Report, checked alongside, does resolve as at the row's date.

What the ledger *does* do is render each recorded rename as its own `Ticker/exchange change` row
(`renamed OLDCO -> NEWCO`), which may be all the docs meant to claim.

**Fix — decided 2026-08-21: make the ledger match the docs.** Two documents assert as-at
resolution and `reports::tax_report` already has the pattern to copy, so the code is what is wrong.

- [ ] Load `RenameHistory` on the ledger's existing read transaction and resolve each counterpart
      listing's ticker at the action's own date, mirroring `tax_report::ticker_as_at`.
- [ ] Re-read both doc claims afterwards (`docs/API.md`'s rename section, the
      `domain/listing_identity.rs` module doc) and make them describe what the ledger then does.

## SCENARIOS R-01/R-05: the rename feature has no web UI, and the listing form sends the user to an endpoint the UI does not offer

`POST /listings/:id/rename`, `GET /listings/:id/renames` and `DELETE /listings/:id/renames/:id` have
no screen. `config.js` mentions `listing_renames` in exactly one place — the Row History table
picker — and `ACTIONS` has no rename entry, so from the web UI a rename cannot be recorded, the
chain cannot be read, and an undo cannot be run.

The gap is self-announcing: editing a ticker on the Listings form for a listing with any recorded
trade answers `422` — "use POST /listings/:id/rename to record a ticker or exchange change on a
listing with recorded trades, income, or prices" — which the toast shows verbatim. The UI's own
error text names an HTTP endpoint the UI never calls.

Per CLAUDE.md this is the shape `ACTIONS` exists for: an owner-row action rendered by the generic
`viewAction`, like reinvest/exercise/participate/demerge.

- [ ] Add a `rename` entry to `ACTIONS` in `config.js` — owner `/listings`, fields
      `effective_date`, `ticker`, `exchange_mic`, `name`, `price_symbol`, `note` — so the refusal's
      remedy is reachable from the screen that raises it.
- [ ] Show the chain: a rename history view over `GET /listings/:id/renames` with the newest entry
      undoable, or the chain rendered on the listing's own row. Decide which rather than building
      both.
- [ ] Update the `docs/API.md` Web frontend paragraph, which lists the UI's screens and actions, once
      the entry exists.

## SCENARIOS R-06: the diagnostic written for a renamed symbol does not fire for a renamed symbol

`closing_price`'s empty-window branch stores a message that names the symbol and points at the fix —
"the symbol may be wrong, renamed, or delisted; set price_symbol on the listing or backfill with an
explicit symbol" — and its comment says it is there for "the classic wrong/renamed/delisted-symbol
case". It fires only when the provider answers **200 with zero candles**. Yahoo does not answer that
way for a symbol it does not know: it answers HTTP 400 or "Not found", which the fetcher surfaces as
a transport error, so those dates store the bare provider string instead.

Measured against the live provider:

| Case | Provider answer | Message stored |
| --- | --- | --- |
| `FB`, retired by the FB → META rename | `Unexpected response status: 400` | bare provider error |
| `ZZQQNOTREAL`, no such symbol | `Not found` | bare provider error |
| `META` over a range before its 2012 IPO | `Unexpected response status: 400` | bare provider error |
| `CBA` on XNYS, a symbol Yahoo knows but serves nothing for | 200, zero candles | the fix-pointing message |

So the renamed case — the one the message names first and the one this section is about — is the one
that misses it. A backfill straddling a rename fills the current span and leaves the pre-rename span
as a wall of raw HTTP errors, with nothing saying the two halves failed for different reasons.

The message's advice is also half wrong for that span: `price_symbol` applies to the **current**
identity only (`yahoo_symbol_for` checks `identity.from == market.current().from`), so setting it
cannot fix a pre-rename date. Only the backfill `symbol` override can, which the same sentence does
also mention.

- [ ] Detect the dead-symbol case on the error path too, not only the empty-candle path, so a
      pre-rename span says why it failed. The symbol is already in hand at that point
      (`fetched_symbol` records it on the errored row).
- [ ] Make the advice span-aware: name `price_symbol` only for a current-identity span, and the
      backfill `symbol` override for an earlier one.

## SCENARIOS R-03: a ticker collision on rename is refused with the raw SQLite constraint text

`docs/API.md` documents a colliding ticker as one of the rename's own `422` causes, alongside the
no-op rename, the out-of-order date and the two Crypto rules — each of which has a written message
(`RenameError`). The collision has none: it falls through to the UNIQUE constraint, and
`infra::http`'s classifier rephrases it as "a record with these key values already exists (UNIQUE
constraint failed: listings.exchange_mic, listings.ticker)". The exchange-less form is the same with
`listings.ticker`. Nothing names the listing collided with, which is the one fact needed to act on
it, and CLAUDE.md's own convention for this family of pairings is a message "rather than quoting the
constraint behind them".

Reproduced: listing 2 is `ZZZ` on XASX; `POST /listings/1/rename {"ticker":"ZZZ"}` gives the message
above. Behaviour is otherwise correct — the transaction rolls back whole, leaving neither a rename
row nor a changed listing, verified on both indexes.

- [ ] Add a `RenameError::TickerCollision` checked before the write, naming the listing that holds
      the ticker (id and name), and cover the exchange-less case in the same check.
- [ ] `PUT /listings/:id` answers the same raw text for an ordinary duplicate; decide whether it
      shares the new message or stays as it is.
