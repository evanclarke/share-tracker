#!/usr/bin/env python3
"""Fetch a page from ato.gov.au and print it as plain text.

ato.gov.au returns 403 to non-browser clients (including Claude's WebFetch
tool), so this script fetches with curl using a browser User-Agent and strips
the HTML down to readable text. Use it when mirroring ATO guidance into docs/ato/
or checking whether a mirrored doc has gone stale.

Usage:
    scripts/ato-fetch.py URL              # print the whole page as text
    scripts/ato-fetch.py URL --examples   # print only the worked-example sections

Tips:
- The ATO legal-database print view renders a whole publication in one page:
  https://www.ato.gov.au/law/view/print?DocID=<DOCID>&PiT=99991231235958
  (e.g. DocID=SAV%2FYAYS%2F00001 for "You and your shares").
- Worked examples sit between an "Example..." heading and "End of example".
"""

import html
import re
import subprocess
import sys

USER_AGENT = (
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36"
)


def fetch_text(url: str) -> str:
    """Fetch `url` with a browser UA and reduce the HTML to plain text lines."""
    result = subprocess.run(
        ["curl", "-sL", "--max-time", "45", "-A", USER_AGENT, url],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        sys.exit(f"curl failed ({result.returncode}): {result.stderr.strip()}")
    raw = re.sub(r"(?s)<(script|style)[^>]*>.*?</\1>", " ", result.stdout)
    text = html.unescape(re.sub(r"(?s)<[^>]+>", "\n", raw))
    lines = [line.strip() for line in text.split("\n") if line.strip()]
    if not lines:
        sys.exit("no text extracted — page may be empty or blocked")
    if "404" in lines[0] and "not found" in lines[0].lower():
        sys.exit(f"page not found: {lines[0]}")
    return "\n".join(lines)


def example_sections(text: str) -> str:
    """The worked-example sections: each 'Example...' heading to 'End of example'."""
    sections = re.findall(
        r"(?s)^(Example[^\n]*\n.*?)(?:^End of example$)", text, flags=re.MULTILINE
    )
    if not sections:
        sys.exit("no 'Example ... End of example' sections found; rerun without --examples")
    return "\n\n---\n\n".join(s.strip() for s in sections)


def main() -> None:
    args = [a for a in sys.argv[1:] if a != "--examples"]
    if len(args) != 1:
        sys.exit(__doc__)
    text = fetch_text(args[0])
    print(example_sections(text) if "--examples" in sys.argv else text)


if __name__ == "__main__":
    main()
