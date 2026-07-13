#!/usr/bin/env bash
#
# ui-smoke.sh — headless smoke check that key web-UI routes actually render.
#
# Drives scripts/ui-check.sh (ephemeral server on a temp DB seeded from the
# demo fixture, headless Chrome --dump-dom) once per route and asserts the
# rendered DOM contains markers that only appear when the SPA booted and the
# view drew real data through the JSON API: a broken /static module route or
# a load-time JS exception leaves the app mount empty and fails every marker,
# which is exactly the failure class the served-bundle string assertions in
# src/web.rs cannot catch. No dependencies beyond ui-check.sh's (Chrome,
# python3, curl, the built server binary); CI runs it on the runner's
# preinstalled Chrome. Run it locally the same way: scripts/ui-smoke.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

# CI runners (and other locked-down kernels) restrict unprivileged user
# namespaces, which breaks Chrome's sandbox; the render is of our own local
# server, so dropping the sandbox there is fine.
if [ -n "${CI:-}" ]; then
  export CHROME_FLAGS="${CHROME_FLAGS:-} --no-sandbox"
fi

# route|marker|marker… — each route asserts a view heading (the SPA's JS
# executed and routed) plus a value from the demo fixture rendered into the
# view (the API round-trip and table renderer worked). Routes are deliberately
# ones that need no live price fetch, so the check never touches the network.
checks=(
  '#/e/trades|<h2>Trades</h2>|filter-row|1234.5678'
  '#/e/income|<h2>Income</h2>|2,757.30'
  '#/r/open-parcels|<h2>Open Parcels</h2>|XASX:VAS'
  '#/r/tax-summary|<h2>Tax Summary</h2>|filter-row'
)

failures=0
for check in "${checks[@]}"; do
  route="${check%%|*}"
  echo "ui-smoke: rendering $route" >&2
  dom="$("$root/scripts/ui-check.sh" --seed demo "$route")"
  IFS='|' read -r -a parts <<<"$check"
  for marker in "${parts[@]:1}"; do
    if ! grep -qF -- "$marker" <<<"$dom"; then
      echo "ui-smoke: FAIL $route did not render marker: $marker" >&2
      failures=$((failures + 1))
      # The app mount's contents are the most useful failure context.
      printf '%s\n' "$dom" | grep -o '<div id="app">.*' | head -c 2000 >&2 || true
      echo >&2
    fi
  done
done

if [ "$failures" -gt 0 ]; then
  echo "ui-smoke: $failures marker(s) missing" >&2
  exit 1
fi
echo "ui-smoke: all routes rendered"
