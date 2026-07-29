# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

## Authentication if ever exposed beyond localhost (honourable mention)
Currently handled correctly: the server binds `127.0.0.1` by default and exposing it is an explicit
`--host` opt-in documented as unauthenticated. Only matters if the server is ever exposed.
- [ ] If/when exposure is wanted: add an auth layer (e.g. a bearer token/basic-auth middleware over the whole router) before recommending `--host 0.0.0.0` for anything but a trusted LAN; until then this section records the decision that localhost-only is the accepted posture

## HTTP test boilerplate (2026-07-29 Rust review)

`test_support.rs` solved the *data* half of test setup (builders for the wide structs) but not the
HTTP half: 274 `Request::builder()` calls across 55 files and 130 copies of
`.collect().await.unwrap().to_bytes()`. Only `entities/closing_price.rs` has local `post_json`
/`put_json` helpers (`:2314`, `:2424`); every other module open-codes the request and the body
decode. Since tests are ~60% of the tree (41.6k of 69.8k lines), this is where line-count reduction
is largest — but it is lower value than the sections above, so it should not jump the queue.

- [ ] Add the HTTP half to `test_support.rs`: an `ApiClient` wrapping `app::router(pool, registry, fetcher)` (or the narrower `router().with_state(pool)` where a test doesn't need the registry/fetcher) with `get_json::<T>(path)`, `put_json(path, &body) -> StatusCode`, `post_json::<T>(path, &body)`, and a `status_and_body(path)` for the rejection tests that assert on the 422 text
- [ ] Migrate test modules onto it opportunistically — when a module is already being touched for one of the sections above, rather than as one large mechanical commit
- [ ] Tests: the migrated tests are the test; the gate is the full suite passing unchanged after each module's migration

