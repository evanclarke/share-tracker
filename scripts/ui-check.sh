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
# Env overrides: CHROME (browser binary path), ST_BIN (server binary path),
# CHROME_FLAGS (extra whitespace-separated Chrome flags, e.g. --no-sandbox on
# CI runners that restrict unprivileged user namespaces).
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
  trap - EXIT INT TERM            # don't re-enter while cleaning up
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null || true
  # Reap the whole Chrome tree (browser + render/gpu/network helpers) by our
  # unique temp profile path — killing the launcher PID alone leaves helpers.
  pkill -9 -f "$profile" 2>/dev/null || true
  rm -rf "$work"
}
# EXIT covers normal/errexit termination; INT/TERM turn a signal into an exit so
# the EXIT trap still runs (a Ctrl-C or a harness `kill` no longer leaks).
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

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
# Deliberately unquoted: CHROME_FLAGS is whitespace-separated flags.
# shellcheck disable=SC2206
[ -n "${CHROME_FLAGS:-}" ] && chrome_common+=(${CHROME_FLAGS})

# Hard ceiling for a single render, as a genuine-failure backstop: the virtual-
# time budget plus slack for startup. A render that blocks (e.g. a stalled live-
# price fetch keeping the page from going idle) is killed rather than hanging the
# whole script — which is what previously stranded the server and Chrome past the
# cleanup trap.
chrome_timeout=$(( budget / 1000 + 15 ))

dump="$work/dump"
shot_file=""

# Chrome's teardown after a headless render is unreliable on macOS (measured on
# Chrome 150, --headless=new): across repeated identical runs the artifact was
# written every time and byte-for-byte identical, while the process itself then
# exited promptly only about half the time and otherwise sat there until it was
# killed. Waiting on the *process* therefore charged the full ceiling on those
# runs and reported a timeout for a render that had in fact completed — every
# route of scripts/ui-smoke.sh printed one while passing all its assertions.
#
# So wait for the *artifact* instead: `ready` succeeds once the output is
# complete (the closing </html> of a dumped document, a PNG's IEND trailer), at
# which point Chrome has done its job and is killed. Chrome exiting on its own
# ends the wait just the same. Only the ceiling elapsing with no complete
# artifact is a real failure, and that is now the only case the caller reports.
#
# Both searches are over raw bytes, so both force the C locale and -a: under a
# UTF-8 locale BSD grep finds no match in a chunk that isn't valid UTF-8, which
# silently sank the PNG check (its trailer is IEND followed by ae 42 60 82) —
# the render then always ran to the ceiling and reported a false failure.
dom_ready() { tail -c 64 "$dump" 2>/dev/null | LC_ALL=C grep -qa '</html>'; }
shot_ready() { tail -c 8 "$shot_file" 2>/dev/null | LC_ALL=C grep -qa 'IEND'; }

# chrome_run READY_FN CHROME_ARG... — 0 once the artifact is complete, 1 if
# Chrome exited without producing one, 124 if the ceiling elapsed first.
chrome_run() {
  local ready="$1"; shift
  "$chrome" "${chrome_common[@]}" "$@" >"$dump" 2>/dev/null &
  local cpid=$!
  local deadline=$(( SECONDS + chrome_timeout ))
  local rc=124
  while :; do
    if "$ready"; then rc=0; break; fi
    if ! kill -0 "$cpid" 2>/dev/null; then
      # Exited on its own: complete (a race against the poll) or a real failure.
      if "$ready"; then rc=0; else rc=1; fi
      break
    fi
    [ "$SECONDS" -lt "$deadline" ] || break
    sleep 0.1
  done
  # Reap the launcher and the render/gpu/network helpers it leaves behind. The
  # braces' stderr redirect swallows the shell's own "Killed: 9" job notice for
  # the SIGKILLed launcher, which is expected here, not a fault worth printing.
  { kill -9 "$cpid" 2>/dev/null || true
    wait "$cpid" 2>/dev/null || true
    pkill -9 -f "$profile" 2>/dev/null || true
  } 2>/dev/null
  return $rc
}

# 124 is the ceiling elapsing; anything else is Chrome giving up on its own.
render_failed() {
  if [ "$2" = 124 ]; then
    echo "ui-check: render of $1 did not complete within ${chrome_timeout}s" >&2
  else
    echo "ui-check: Chrome exited without completing the render of $1" >&2
  fi
}

[ "$mode" = shot ] && mkdir -p "$out"
multi=0; [ "${#routes[@]}" -gt 1 ] && multi=1

for route in "${routes[@]}"; do
  url="$base/$route"
  if [ "$mode" = dom ]; then
    [ "$multi" = 1 ] && echo "===== $route ====="
    rc=0; chrome_run dom_ready --dump-dom "$url" || rc=$?
    [ "$rc" = 0 ] || render_failed "$route" "$rc"
    # Emitted either way: a partial document is the most useful failure context
    # (ui-smoke.sh prints the app mount from it when a marker is missing).
    cat "$dump"
  else
    safe="$(printf '%s' "$route" | tr -c 'A-Za-z0-9' '_')"
    shot_file="$out/${safe}.png"
    rm -f "$shot_file"
    rc=0; chrome_run shot_ready --window-size=1280,1600 --screenshot="$shot_file" "$url" || rc=$?
    if [ "$rc" = 0 ]; then
      echo "saved $shot_file"
    else
      render_failed "$route" "$rc"
    fi
  fi
done
