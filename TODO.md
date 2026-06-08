# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

## Useful error messages in the web UI
(REQUIREMENTS "New Requirements — Useful error messages in the web UI", added 2026-06-07. A rejected write must say *why*: today most handlers return a bare `StatusCode`, so the toast reads just "HTTP 422" — e.g. reinvesting a distribution for an account not DRP-enrolled (`drp_reinvestment` `NotEnrolled`) gives no hint that enrolment is the problem. The toast plumbing already displays a response body when present (`api()` in `app.js` appends it), so the work is server-side; the per-share (`per_share_detail`) and statement-total (`statement_total_detail`) 422 bodies are the model.)
- [ ] Audit every handler that returns a bare `StatusCode` for user-triggerable 422/409/404-with-a-cause responses; inventory each endpoint's rejection causes (the entity upserts' `write_error_status` constraint mapping, the Sell/transfer/operation invariants, DRP reinvestment, corporate-action freezes, delete-while-referenced refusals, …)
- [ ] Attach a short, human-readable, plain-text body to each: which invariant failed, with the actual values involved (e.g. "allocations sum to 95 but the sell quantity is 100", "account 'Broker' is not enrolled in a DRP for VDHG at 2026-03-04 — enrol it on the DRP enrolments screen first"); messages name entities by name/ticker, never by raw foreign-key id (per the no-raw-ids section above)
- [ ] 404s from a stale UI say what wasn't found (a bare "not found" body is fine where there is nothing more to say); 5xx responses stay generic — internal details go to the server log, not the toast
- [ ] Tests: each validated rejection asserts the body text (or a distinctive fragment of it), not just the status code — extend the existing 422 tests as each handler gains its detail
- [ ] Docs sync: `docs/API.md` Response codes section documents the plain-text error-body convention; each endpoint's non-obvious 422 causes listed where they aren't already
