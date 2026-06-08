---
name: develop-next
description: Pick next item to implement from TODO - do this when the user asks you to "do the next thing"
---
TODO.md holds only open / in-flight work; completed and decided sections are archived in DONE.md (see CLAUDE.md). So the next item is just the next `- [ ]` in TODO.md — no need to wade past a long history.

Steps:
1. Check TODO.md for the next item that needs to be implemented
2. If it makes sense to do so and multiple TODOs are logically together, do them together
3. Once implemented and all new/existing tests pass, check off the completed TODO items
4. When a `## ` section is fully closed (all items `[x]`, or any remaining `[ ]` are explicitly N/A / out of scope), move the whole section verbatim — heading, items, and notes — from TODO.md to DONE.md
5. If required, update CLAUDE.md