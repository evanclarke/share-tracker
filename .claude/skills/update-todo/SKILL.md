---
name: update-todo
description: Project TODO updating - use when user updates REQUIREMENTS.md and asks to update TODO.md
---
Open / in-flight work lives in TODO.md; completed and decided sections are archived in the topical `DONE/*.md` files, indexed by DONE.md (see CLAUDE.md). Cross-reference both so already-done or already-decided requirements aren't re-added as new work.

Steps:
1. Analyse the REQUIREMENTS.md file and cross reference it with TODO.md (open work) and the DONE/*.md files (already completed / decided out of scope — start from DONE.md's index to find the relevant file(s))
2. If new requirements are found, break them down into appropriate items and add them to TODO.md
3. If completed items (in DONE/*.md or TODO.md) are not present in the requirements, it's likely the functionality is not needed anymore so add a TODO.md item for removing it
