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

