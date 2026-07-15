# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

## Wash-sale report excludes crypto transfer network-fee disposals (REQUIREMENTS 2026-07-15)
A transfer's network-fee disposal is an ordinary loss-realising Sell (no `transfer_id`, so the
gains reports count it — correctly), so the wash-sales report flags it whenever a Buy of the same
crypto lands inside the ±30-day window. TR 2008/1 is purposive: the fee disposal is compelled by
the transfer, timed by it, and the fee units are never re-acquired — no Part IVA fact pattern.
Symmetric with the report's existing Buy-side provenance exclusions.
- [ ] `db_wash_sales` never treats a Sell referenced by `transfers.fee_sale_trade_id` as a wash-sale candidate; the fee disposal's loss still counts in realised-gains / net-capital-gain / performance, unchanged
- [ ] Genuine Sells keep flagging: an ordinary loss Sell of the same listing near a re-buy still alerts (including crypto)
- [ ] Tests: a fee-bearing transfer whose fee disposal realises a loss + a Buy of the listing inside the window → no alert; an ordinary loss Sell in the same window → alert; fee-Sell loss still present in the realised-gains report
- [ ] Docs: the exclusion + TR 2008/1 rationale in `docs/ato/wash-sales.md` "How this maps to the project", the `reports/wash_sales.rs` module docs, and `docs/API.md`'s wash-sales section

## Authentication if ever exposed beyond localhost (honourable mention)
Currently handled correctly: the server binds `127.0.0.1` by default and exposing it is an explicit
`--host` opt-in documented as unauthenticated. Only matters if the server is ever exposed.
- [ ] If/when exposure is wanted: add an auth layer (e.g. a bearer token/basic-auth middleware over the whole router) before recommending `--host 0.0.0.0` for anything but a trusted LAN; until then this section records the decision that localhost-only is the accepted posture

