#!/usr/bin/env python3
"""Insert missing struct fields after each literal's opening line, driven by
cargo's E0063 errors (struct-literal sites; field order is free in Rust).

Usage: cargo check --all-targets --message-format=json | scripts/add-missing-fields.py

The fields to insert and their default expressions are hardcoded below for
the change at hand. Re-run cargo check afterwards; repeat until clean.
"""
import json
import sys
from collections import defaultdict

FIELDS = {
    "brokerage_includes_gst": "brokerage_includes_gst: false,",
    "statement_total": "statement_total: None,",
}

# file -> set of (line, missing-field-names)
sites = defaultdict(set)
for raw in sys.stdin:
    try:
        msg = json.loads(raw)
    except json.JSONDecodeError:
        continue
    m = msg.get("message") or {}
    if (m.get("code") or {}).get("code") != "E0063":
        continue
    missing = [f for f in FIELDS if f"`{f}`" in m.get("message", "")]
    if not missing:
        continue
    for span in m.get("spans", []):
        if span.get("is_primary"):
            sites[span["file_name"]].add((span["line_start"], tuple(sorted(missing))))

total = 0
for path, entries in sites.items():
    with open(path) as f:
        lines = f.readlines()
    # bottom-up so earlier line numbers stay valid
    for line_no, missing in sorted(entries, reverse=True):
        opening = lines[line_no - 1]
        # indent two spaces past the literal's opening line
        indent = (len(opening) - len(opening.lstrip())) * " " + "    "
        insert = [indent + FIELDS[f] + "\n" for f in missing]
        lines[line_no:line_no] = insert
        total += len(insert)
    with open(path, "w") as f:
        f.writelines(lines)

print(f"inserted {total} fields across {len(sites)} files")
