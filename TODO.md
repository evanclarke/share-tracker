# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

## Authentication if ever exposed beyond localhost (honourable mention)
Currently handled correctly: the server binds `127.0.0.1` by default and exposing it is an explicit
`--host` opt-in documented as unauthenticated. Only matters if the server is ever exposed.
- [ ] If/when exposure is wanted: add an auth layer (e.g. a bearer token/basic-auth middleware over the whole router) before recommending `--host 0.0.0.0` for anything but a trusted LAN; until then this section records the decision that localhost-only is the accepted posture

## DRP trades show the funding distribution's attachments (REQUIREMENTS 2026-07-15)
Every DRP statement in the archive is attached to the income row it was entered from (the Reinvest
action creates the DRP trade *from* that row, and the one advice documents both the distribution
and the reinvestment), so a DRP trade's own Attachments view is always empty today — the paperwork
exists but is not discoverable from the trade.
- [ ] A DRP trade's Attachments view also lists the linked income row's attachments (traversing `reinvestment_trade_id`), clearly labelled as the income row's documents: download works from there, upload from the trade's view still attaches to the trade, delete stays on the owning record's view. Attachments stay single-owner — a read-time traversal (web UI or a list-endpoint option, design-open), no data-model change
- [ ] The same rule for the other provenance-created trades whose source record owns attachments (an ESS vest Buy shows its `ess_statements` row's attachments; a buy-back Sell its income row's) — enumerate the provenance links at implementation time
- [ ] Docs: `docs/API.md` if the list endpoint gains the linked-owner option; the Attachments feature text mentions linked documents
