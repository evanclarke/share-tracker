# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–S are driven and every finding they raised is closed** in the `DONE/*.md`
archive. Section **S. Settlement, holidays, and dates** was driven 2026-08-22 (`d501408`) and its
four findings closed by `67c3096` (a trade dated in the future), `30d0e96` (a trade dated on a day
its exchange was shut), `4a7ef1a` (a stored settlement date that is not a trading day) and
`e453f21` (the settlement dates a completed calendar changes) — all four archived in
[`DONE/trades-income.md`](DONE/trades-income.md), and summarised with the rest of the pass under
[Section S findings](SCENARIOS.md#section-s-findings). Every section's row in SCENARIOS.md's
[Verification status](SCENARIOS.md#verification-status) table names the pass that drove it and where
its findings went; that table is the record of what has been looked at.

Section **T. Jobs, backup, and operations** (12 scenarios) was driven on **2026-08-22** against
throwaway databases (a small one for the HTTP surface, a 265 MB one to catch a backup mid-write) and
raised **six findings**, and all six are now closed — the three jobs that recorded their failure as a
Rust `Debug` string (T-06), the startup "no schedule entry" warning that cried wolf on the two
deliberately-manual jobs (T-09/schedule), `POST /jobs/:name`'s bare-status-code failures, now
bodied 404/500 replies with an unknown query parameter refused rather than ignored (T-10), the
run interrupted by a restart that left no record and an unverified file wearing a backup's name
(T-11), now a run row opened at the start and a backup staged under `.partial` until it verifies,
the job that stops running and is never noticed (T-11/T-02/T-12), now a `job_schedule` table the
scheduler rewrites every iteration with a health `overdue_jobs` list and a **next run** column on the
Jobs screen over it, and the currency import that skipped the whole ISO 24165 half and reported an
unqualified success (T-09), now a per-feed import summary and a `job_runs.note` the Jobs screen shows
beside a still-`ok` status — all six archived in [`DONE/infra.md`](DONE/infra.md).
Everything else in the section came back correct: the
per-job lock serialises overlapping triggers (T-01), the run history bounds itself at 20 while
keeping a fail-then-succeed sequence readable (T-02), a corrupt backup is quarantined (T-03),
retention pruning over 20 backups spanning 18 months keeps exactly the newest 8 plus the first of
each of the 12 most recent months and touches nothing else in the directory (T-04), the DST-pinned
price-import entries fire at 17:30 local on both sides of every transition in both hemispheres and
handle the skipped and repeated hours exactly as the README states (T-05), the manual CSV retry
imports what the unreachable feed would have (T-06), an expiring MIC flips the validation report to
`expired` without blocking anything (T-08), and a stale price date, a stale FX month and a failed job
all surface at once and independently on the health report and its banner (T-12).

Section **U. Audit trail and history** (8 scenarios) was driven on **2026-08-22** against a
throwaway database, with two live backups read read-only to check each finding against real data.
**The trail's machinery came back correct**: every one of the 22 audited tables' triggers records
every column of the live schema (U-01, checked by diffing `PRAGMA table_info` against each trigger's
`json_object` keys, which is stronger than the name-list pin the tests carry); `row_history` is
append-only and no `REPLACE INTO` path exists anywhere in the tree, while migration 0025 — the one
migration that rewrites an audited table's data in place — deliberately drops the triggers first, so
no migration forges an entry (U-02); a cascade-deleted attachment is recorded like a directly deleted
row (U-04); a superseded manual closing price's `sourced_from`, `reason` and `origin` are fully
recoverable exactly as `docs/API.md` claims (U-06); a non-audited table is refused 422 naming the
audited list (U-07); and the report is unbounded by design, which is the safe direction for an audit
trail — 10,000 entries served in 0.42 s, against a live maximum of 2 entries on any single row
(U-08).

It raised **three findings, and all three are now closed** — the id handed out again that inherits
the deleted row's trail (U-a), which was live in the real database and took three commits: the trail
now says whose history it is (`7b915cf`), migration 0045 gives 17 audited tables `AUTOINCREMENT` ids
(`4a3a257`), and every server-created id now comes from the database rather than `MAX(id) + 1`
(`1a0f821`); the multi-row operation readable only by ids the user never saw, now a browse form over
the whole trail (`be64d3d`); and the trigger-column rule enforced only by hand-written per-migration
assertions, now a generic guard derived from the live schema (`57502eb`) — all three archived in
[`DONE/infra.md`](DONE/infra.md).

**Section V's four findings are open below.**

After U, the next SCENARIOS pass is section **V. Back-dated and out-of-order entry** (10 scenarios),
driven the way S, T and U were: run every scenario against a throwaway database, apply the standing
probes to each, and log what each raises as a `## SCENARIOS V-nn` section here with the option Evan
chose. The lessons worth carrying forward are in the handover memory; U added three. First, **the
standing probes find what the scenario list does not name** — id reuse is not one of U's eight
scenarios; it fell out of asking "what else moved that shouldn't have" about U-01 and U-04, and it is
the section's most serious finding. Second, and again: **check the live database read-only** — U-a
would have read as a theoretical hazard about `INTEGER PRIMARY KEY` if the live DB had not shown it
already firing twice, on a trade Evan actually entered. Third, sharper than either: **re-derive a
fix's mechanism before building it, not just its arithmetic**. U-a's chosen option was
`AUTOINCREMENT`, and `AUTOINCREMENT` governs only the ids SQLite picks — nine call sites bound their
own `MAX(id) + 1` and would have sailed through the whole 17-table migration unchanged, leaving the
headline case live. The same trap sprang twice more in one section: the finding's claim that `'now'`
is constant across a transaction was false (it is per *statement*), and the migration's first draft
justified its `sqlite_sequence` seeding with arithmetic that did not hold. Each was caught by
measuring against the database instead of reasoning about it.

---

## SCENARIOS V-a — a misspelt field name in a request body is silently ignored

Raised driving **V-01 / V-09** (a year of history entered in one session). Every HTTP request
body in the tree deserialises with serde's default behaviour, so a key the struct does not
recognise is **dropped**, and the field it was meant to set takes its `#[serde(default)]` value.
A **required** field is already safe — omitting it is a `422` naming it — but almost every
*money* field on the tax-bearing entities is optional-with-default, so a one-character typo
writes a legitimate-looking row with a zero in it and answers `204`.

Measured against a throwaway database:

| Request | Sent | Stored | Response |
| --- | --- | --- | --- |
| `PUT /amma_statements/9` | `franked_dividend: "5000"`, `frankingcredits: "2142"` | `franked_dividends: 0`, `franking_credits: 0` | `204` |
| `PUT /trades/7` | `settlment_date: "2025-04-09"` | `settlement_date: 2025-04-03` (`computed`) | `204` |
| `PUT /trades/7` | `contract_note: "CN123"` | `contract_note_ref: null` | `204` |
| `POST /reports/row_history` | `table_name: "parcel_allocations"` | filter ignored — whole trail returned | `200` |

The AMMA row is the one that matters: A$7,142 of a lodgeable tax figure vanished with nothing
anywhere saying so. `income` (every component `#[serde(default)]`), `interest_income` (`amount`
itself defaults, as does `foreign_source`, which routes the row between 10L and 20E) and
`investment_expense` have the same shape.

**The project already holds the opposite convention, for the two bodies that are not HTTP.**
`infra/config.rs` and `scheduler::JobParams` both carry `#[serde(deny_unknown_fields)]` with the
reasoning written out beside it — *"`deny_unknown_fields` makes a misspelt parameter a rejection
rather than a silently-ignored default"* — and **T-10** made an unrecognised *query* parameter on
`POST /jobs/:name` a `422` naming it for exactly this reason (`` cannot read the query string:
sufix: unknown field `sufix` ``). The HTTP request bodies are the gap. 233 `Deserialize` derives
in `src`, none of them denying.

Options offered:

1. `#[serde(deny_unknown_fields)]` on every HTTP request-body struct, with a test that
   enumerates the bodies reachable from a handler so a new one cannot be added without it.
2. The same, but on the **write** bodies only (entity `PUT`/`POST`), leaving report request
   bodies permissive.
3. Leave it and document it as a known limitation.

**Evan chose option 1** — `deny_unknown_fields` on *every* HTTP request-body struct, report
bodies included, with a test enumerating the bodies reachable from a handler so a new one cannot
be added without it.

- [ ] Add `#[serde(deny_unknown_fields)]` to every HTTP request-body struct, with the enumerating
      test and a `docs/API.md` note that an unrecognised body field is refused.

## SCENARIOS V-b — reinvesting a DRP distribution out of order builds the residual chain backwards

Raised driving **V-08** (a DRP enrolment period entered retroactively over distributions already
recorded as cash — after which the user reinvests them, and not necessarily in date order).

`entities::drp_reinvestment::db_reinvest` reads the residual brought forward as

```sql
SELECT t.residual_carried_forward … ORDER BY t.date DESC, t.id DESC LIMIT 1
```

over the enrolment period's DRP trades, with **no bound requiring that trade to be dated before
the one being created**. Its comment says *"'most recent' is the payment order the cash actually
moved in"*, which holds only when reinvestments are entered in that order — and this is the
section about the times they are not.

Measured: one listing, one open `CarryForward` period, two distributions — 2024-03-28 paying
A$105 and 2024-09-30 paying A$107. Reinvesting the **September** one first (at A$10) then the
**March** one (at A$9):

| DRP trade | date | quantity | brought fwd | carried fwd |
| --- | --- | ---: | ---: | ---: |
| 4 | 2024-03-28 | **12** | **7** | 4 |
| 3 | 2024-09-30 | **10** | **0** | 7 |

The March parcel brought forward A$7 from a reinvestment six months **later**, and the September
one never picked up March's own leftover. The correct chain is March 105 → 11 units @ 9 (carry 6),
September 107 + 6 = 113 → 11 units @ 10 (carry 3). Both parcels carry the wrong quantity, and A$7
of cash is spent twice. Nothing surfaces it: the health report is silent and no cross-check reads
the chain.

Note the asymmetry with **undo**, which already enforces the ordering for this exact reason:
`DELETE /income/:id/reinvest` refuses while a later DRP trade exists, because *"the residual chain
reads each reinvestment's brought-forward cash back from the most recent prior DRP trade, so
removing a mid-chain trade would falsify the later trade's residuals"*. Creating one mid-chain is
the same falsification and is not refused.

Options offered:

1. **Refuse out-of-order creation**, mirroring undo: bound the lookup to trades dated strictly
   before the new one, and reject a reinvestment that is not the period's latest — "reinvest in
   payment order".
2. **Re-derive the period's whole chain** on every reinvest write (walk its trades in date order,
   recomputing brought-forward, carried-forward and quantity). Correct under any entry order, but
   a new reinvestment then rewrites the *quantity* of DRP parcels already entered, which Sell
   allocations and AMIT adjustments may already draw on.
3. Bound the lookup to earlier trades only, and add a cross-check row for a period whose chain
   does not reconcile in date order.

**Evan chose option 1** — refuse out-of-order creation, mirroring the undo rule: bound the lookup
to trades dated strictly before the new one, and reject a reinvestment that is not the period's
latest.

- [ ] Bound the residual lookup and refuse a non-latest reinvestment, with a test entering two
      reinvestments in reverse order and `docs/API.md` carrying the new `422`.

## SCENARIOS V-c — a trade entered twice is the one duplication the health report does not look for

Raised driving **V-09** (import a whole portfolio's history in one session and reconcile the final
holdings against a registry statement).

`GET /reports/health` carries a `duplicate_*` check for every other user-entered fact table —
`duplicate_income`, `duplicate_interest`, `duplicate_expenses`, `duplicate_amma_statements`,
`duplicate_ess_statements`, `duplicate_inheritances`, `duplicate_actions`, `duplicate_price_series`
— and none for **trades**, which during a bulk back-entry is the row most likely to be keyed twice
and the most expensive to get wrong.

Measured: two identical Buys of one listing — same date, holding account, price, quantity **and the
same `contract_note_ref: "CN-8891"`** — were both accepted, and health reported nothing. Two
identical income rows entered in the same session were flagged immediately.

A duplicated Buy inflates the holding and the cost base; a duplicated Sell inflates realised gains
and its allocations quietly consume a second parcel. Either is invisible until the holdings are
reconciled against a registry statement, which is the whole of V-09.

Options offered:

1. `duplicate_trades`, keyed the way `duplicate_income` is — listing, holding account, date,
   `trade_type`, `average_price`, `quantity` — over all trade types.
2. The same, but restricted to **user-entered** trades: exclude the rows a derived path creates
   (rollover/transfer/buy-back/rights/ESS-vest/inheritance-linked and reinvest-created DRP), which
   can legitimately repeat.
3. Key on a repeated non-null `contract_note_ref` alone — no false positives at all, but it only
   catches imports that record the broker reference.

**Evan chose option 3** — key on a repeated non-null `contract_note_ref`. No false positives, and
a broker reference repeated across two trades is unambiguous evidence of a double entry.

- [ ] Add the `duplicate_trades` health check keyed on `contract_note_ref`, with a test, the
      `docs/API.md` health entry, and the UI health banner wording.

## SCENARIOS V-d — a parcel dated before an already-run whole-holding operation is never consumed

Raised driving **V-03 / V-06** (a corporate action, and a back-dated acquisition, entered after
the facts they should have reached).

Three operations consume **every** open parcel of their listing as a matter of law, not choice:
the scrip-for-scrip **exchange**, the **demerge**, and the worthless-shares **recognise**. Each is
refused if the listing traded on or after its date, and `docs/API.md`'s *Recording one of the three
read-time events behind a rollover that has already run* refuses a `ReturnOfCapital`, `ShareSplit`
or `BonusIssue` dated on or before one — on the stated grounds that otherwise *"the same facts
entered in a different order would report a different cost base"*.

A **parcel** dated before one is not guarded. Measured, each accepted `204`:

| Operation (already executed) | Back-dated write | Result |
| --- | --- | --- |
| Exchange OLD → NEW 1-for-1, 2024-06-10 | Buy 50 OLD, 2024-02-05 | 50 units of a security that no longer exists; 50 NEW units missing |
| Exchange OLD → NEW 1-for-1, 2024-06-10 | Inheritance of 25 OLD, died 2024-03-01 | same, via the inheritance parcel path |
| Demerge HEAD 1-for-5, 2024-06-11 | Buy 50 HEAD, 2024-03-05 | no SPIN units issued; the parcel keeps 100% of its cost base instead of 90% |
| Recognise DEAD worthless, 2024-06-13 | Buy 40 DEAD, 2024-03-05 | 40 units still open on a company already written off |

Nothing surfaces any of them. `GET /reports/rollover_consistency` is blind by construction — it
compares what the **consumed** units are worth now against the replacements' stored figures, and
these units were never consumed — and the health report says nothing.

Not affected, and correctly so: a **transfer** and a **buy-back participation** move a *chosen*
quantity, so a parcel left behind is a legitimate outcome.

Options offered:

1. **Refuse at write time**: a parcel-creating write (Buy/DRP `PUT /trades`, inheritance, ESS
   vest, rights exercise) dated on or before an executed exchange/demerge/recognise on that
   listing answers `422` naming the operation and its date, with the recovery its sibling refusal
   already gives — delete the operation, enter the parcel, run it again.
2. **Report it**: extend `rollover_consistency` (and so the annual tax report's completeness
   section) with an *unconsumed parcel* problem naming every parcel open on the listing at the
   operation's date that the operation did not consume. Advisory; nothing is refused.
3. **Both** — refuse the write, and report any state that predates the guard, which is the pattern
   the AMIT-adjustment / rollover pair already follows.

**Evan chose option 3** — refuse *and* report: `422` at write time naming the operation and its
date, plus an *unconsumed parcel* problem on `rollover_consistency` for any state that predates
the guard.

- [ ] Refuse a parcel-creating write dated on or before an executed exchange/demerge/recognise,
      with a test per affected operation and per parcel-creating path.
- [ ] Add the unconsumed-parcel problem to `rollover_consistency` (and so the annual tax report's
      completeness section), with a test and the `docs/API.md` entry.
