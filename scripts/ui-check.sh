#!/usr/bin/env bash
#
# ui-check.sh — render a share-tracker web-UI route in headless Chrome.
#
# Spins up an ephemeral server (temp DB, auto-picked free port), optionally
# seeds it from a JSON fixture, then runs headless Chrome against one or more
# hash routes and prints the rendered DOM (or saves a screenshot). The server
# and all temp files are torn down on exit. No dependencies beyond a local
# Chrome, python3, curl, and the built server binary — it does NOT add a
# browser test harness; it just automates the start/seed/render/teardown dance
# for manual spot-checks (the automated tests stay bundle-assertion based).
#
# Usage:
#   scripts/ui-check.sh [--seed NAME] [--screenshot] [--out DIR] [--budget MS] ROUTE...
#
# ROUTE is a hash route, e.g. '#/e/trades', '#/r/open-parcels'. Quote it so the
# shell doesn't treat '#' as a comment.
#
# Examples:
#   scripts/ui-check.sh '#/e/trades'
#   scripts/ui-check.sh --seed demo '#/e/trades' '#/r/open-parcels'
#   scripts/ui-check.sh --seed demo --screenshot --out shots '#/r/overview'
#
# Env overrides: CHROME (browser binary path), ST_BIN (server binary path).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
seed=""
mode="dom"          # dom | shot
out="ui-shots"
budget=8000
routes=()

while [ $# -gt 0 ]; do
  case "$1" in
    --seed) seed="$2"; shift 2;;
    --screenshot) mode="shot"; shift;;
    --out) out="$2"; shift 2;;
    --budget) budget="$2"; shift 2;;
    -h|--help) sed -n '2,30p' "$0"; exit 0;;
    --) shift; while [ $# -gt 0 ]; do routes+=("$1"); shift; done;;
    -*) echo "ui-check: unknown option: $1" >&2; exit 2;;
    *) routes+=("$1"); shift;;
  esac
done

if [ "${#routes[@]}" -eq 0 ]; then
  echo "ui-check: give at least one route, e.g. '#/e/trades'" >&2
  exit 2
fi

# --- locate Chrome -----------------------------------------------------------
chrome="${CHROME:-}"
if [ -z "$chrome" ]; then
  for c in \
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
    "/Applications/Chromium.app/Contents/MacOS/Chromium" \
    "$(command -v google-chrome 2>/dev/null || true)" \
    "$(command -v chromium 2>/dev/null || true)"; do
    if [ -n "$c" ] && [ -x "$c" ]; then chrome="$c"; break; fi
  done
fi
[ -n "$chrome" ] || { echo "ui-check: Chrome not found — set CHROME=/path/to/chrome" >&2; exit 1; }

# --- locate / build the server binary ---------------------------------------
bin="${ST_BIN:-$root/target/debug/share-tracker}"
if [ ! -x "$bin" ]; then
  echo "ui-check: building share-tracker (debug)…" >&2
  ( cd "$root" && cargo build ) >&2
fi

# --- ephemeral workspace + free port ----------------------------------------
work="$(mktemp -d)"
db="$work/st.db"
profile="$work/chrome"
port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
base="http://127.0.0.1:$port"
server_pid=""

cleanup() {
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

# --- start the server and wait for it to answer -----------------------------
"$bin" --db "$db" --port "$port" >"$work/server.log" 2>&1 &
server_pid=$!

ready=""
for _ in $(seq 1 50); do
  if curl -fsS -o /dev/null "$base/" 2>/dev/null; then ready=1; break; fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "ui-check: server exited during startup:" >&2; cat "$work/server.log" >&2; exit 1
  fi
  sleep 0.2
done
[ "$ready" = 1 ] || { echo "ui-check: server never became ready:" >&2; cat "$work/server.log" >&2; exit 1; }

# --- seed (optional) ---------------------------------------------------------
if [ -n "$seed" ]; then
  fixture="$root/scripts/fixtures/$seed.json"
  [ -f "$fixture" ] || { echo "ui-check: no fixture at $fixture" >&2; exit 1; }
  python3 - "$base" "$fixture" >&2 <<'PY'
import json, sys, urllib.request, urllib.error
base, path = sys.argv[1], sys.argv[2]
for r in json.load(open(path)):
    method = r.get("method", "PUT")
    p = r["path"]
    body = r.get("body")
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        base + p, data=data, method=method,
        headers={"Content-Type": "application/json"} if data is not None else {})
    try:
        with urllib.request.urlopen(req) as resp:
            print(f"seed {method} {p} -> {resp.status}")
    except urllib.error.HTTPError as e:
        print(f"seed {method} {p} -> {e.code}: {e.read().decode()[:300]}")
        sys.exit(1)
PY
fi

# --- render each route -------------------------------------------------------
chrome_common=(--headless=new --disable-gpu --no-first-run --no-default-browser-check
               --user-data-dir="$profile" --virtual-time-budget="$budget")

[ "$mode" = shot ] && mkdir -p "$out"
multi=0; [ "${#routes[@]}" -gt 1 ] && multi=1

for route in "${routes[@]}"; do
  url="$base/$route"
  if [ "$mode" = dom ]; then
    [ "$multi" = 1 ] && echo "===== $route ====="
    "$chrome" "${chrome_common[@]}" --dump-dom "$url" 2>/dev/null
  else
    safe="$(printf '%s' "$route" | tr -c 'A-Za-z0-9' '_')"
    file="$out/${safe}.png"
    "$chrome" "${chrome_common[@]}" --window-size=1280,1600 --screenshot="$file" "$url" 2>/dev/null
    echo "saved $file"
  fi
done
