# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

## Client-side pagination for large tables
(REQUIREMENTS "Client-side pagination for large tables", added 2026-06-08. Tables that can grow large — entity lists, the Sells list, report tables (trades, closing-price history, snapshots, parcels) — should paginate so a long result set isn't dumped as one table. Client-side at this stage: the JSON endpoints keep returning the full array and the web layer pages through it. Server-side API pagination is out of scope for now — record as a Known limitation.)
- [ ] The shared `filterableTable` gains pagination: a 50-row default page size with page navigation (next/prev and/or page numbers), so only one page of rows is in the DOM at a time; tables of 50 rows or fewer show no pagination control
- [ ] Pagination composes with filtering and sorting: filtering/sorting apply to the **whole** result set, then the result is paged (never page-then-filter); changing a filter resets to the first page; the count reflects the filtered total and the control shows e.g. "showing 1–50 of 320"
- [ ] Applied uniformly through `filterableTable` so every table benefits without bespoke per-table paging
- [ ] Docs sync: record server-side API pagination as a Known limitation (`docs/API.md` / README) — the full set is still fetched, so this addresses rendering/usability, not payload size
- [ ] Tests (served-bundle convention): assert the paging controls/behaviour ship in the bundle and that filtering still reflects the full result set
