# Done & decided — Overview

Archive of completed and explicitly out-of-scope sections moved out of
[TODO.md](TODO.md) to keep the active list small. Nothing here is deleted —
items are relocated verbatim under their original `##` section headings, with
the implementation/decision notes attached when each was closed. The
authoritative record remains the code, `docs/`, and git history; this archive
is the project's task-level changelog and decision log. Items still marked
`[ ]` in an archived section were decided out of scope or judged not
reproducible (kept for the rationale) — they are closed, not pending.

Split into topical files (mirrors the `docs/ato/` pattern of many small files
+ one index) so no single file grows without bound:

| File | Covers | Sections |
| --- | --- | --- |
| [`DONE/infra.md`](DONE/infra.md) | Infrastructure setup, FX/MIC/currency reference-data imports, backups, scheduler, packaging/CI, authentication | 20 |
| [`DONE/reference-data.md`](DONE/reference-data.md) | Exchanges, listings, accounts, holding accounts, ticker/exchange-code renames, price-collection gaps | 7 |
| [`DONE/trades-income.md`](DONE/trades-income.md) | Trade/income entry, AMMA, DRP, parcel allocations, attachments, cost-base adjustments | 22 |
| [`DONE/reporting.md`](DONE/reporting.md) | Portfolio/gains/tax reports, snapshots, performance metrics, tax-return export | 27 |
| [`DONE/tax-domain.md`](DONE/tax-domain.md) | ATO-cited CGT/tax calculation rules — discount, cost base, corporate actions, FITO, franking | 38 |
| [`DONE/crypto.md`](DONE/crypto.md) | Crypto-asset holdings and wallet-to-wallet transfers | 2 |
| [`DONE/web-frontend.md`](DONE/web-frontend.md) | Web UI screens, config-driven refactors, readability/UX fixes | 14 |
| [`DONE/reviews.md`](DONE/reviews.md) | Code/design review findings and their resolutions | 29 |

**Archiving a closed `TODO.md` section**: move it verbatim (heading + items +
notes) into whichever file above matches its subject — usually obvious from
the heading (an entity/report name → its file; a dated "review" finding →
`reviews.md`). Append at the end of the file, keeping chronological order. If
a section doesn't fit an existing file well, that's a sign a new topical file
is warranted — add it and a row to the table above.
