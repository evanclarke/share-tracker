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

**Eight findings; R-01, R-02, R-04/R-08, R-06 and R-10 are closed (all archived in
`DONE/reference-data.md`), R-07 is closed (archived in `DONE/reviews.md`) and R-01/R-05's web-UI gap
is closed (archived in `DONE/web-frontend.md`), one remains open** — a refusal message. It is the
`## ` section below.

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
