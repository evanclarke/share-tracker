#!/usr/bin/env bash
#
# screenshots.sh — regenerate the README's screenshots into docs/screenshots/.
#
# Starts an ephemeral server (temp DB, auto-picked free port), seeds it from
# scripts/fixtures/showcase.json — a wholly fictional portfolio, so no real
# holding is ever pictured — then drives it with scripts/ui-drive.js to capture
# each view twice, once in the light scheme and once in the dark one. The
# server and all temp files are torn down on exit.
#
# Usage:  scripts/screenshots.sh [--out DIR]
#
# Env overrides: CHROME (browser binary), ST_BIN (server binary).
#
# Like ui-check.sh and ui-drive.js this automates a manual step and is not a
# test harness: nothing here runs in CI. Re-run it when a captured screen
# changes, and commit the PNGs it writes.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/docs/screenshots"

while [ $# -gt 0 ]; do
  case "$1" in
    --out) out="$2"; shift 2;;
    -h|--help) sed -n '2,16p' "$0"; exit 0;;
    *) echo "screenshots: unknown option: $1" >&2; exit 2;;
  esac
done

bin="${ST_BIN:-$root/target/debug/share-tracker}"
if [ ! -x "$bin" ]; then
  echo "screenshots: building share-tracker (debug)…" >&2
  ( cd "$root" && cargo build ) >&2
fi

mkdir -p "$out"
work="$(mktemp -d)"
port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
base="http://127.0.0.1:$port"
server_pid=""

cleanup() {
  trap - EXIT INT TERM
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

"$bin" --db "$work/st.db" --port "$port" >"$work/server.log" 2>&1 &
server_pid=$!

ready=""
for _ in $(seq 1 50); do
  if curl -fsS -o /dev/null "$base/" 2>/dev/null; then ready=1; break; fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "screenshots: server exited during startup:" >&2; cat "$work/server.log" >&2; exit 1
  fi
  sleep 0.2
done
[ "$ready" = 1 ] || { echo "screenshots: server never became ready:" >&2; cat "$work/server.log" >&2; exit 1; }

echo "screenshots: seeding the showcase fixture…" >&2
python3 - "$base" "$root/scripts/fixtures/showcase.json" >&2 <<'PY' || {
import json, sys, urllib.request, urllib.error
base, path = sys.argv[1], sys.argv[2]
recs = json.load(open(path))
for i, r in enumerate(recs, 1):
    method, p, body = r.get("method", "PUT"), r["path"], r.get("body")
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        base + p, data=data, method=method,
        headers={"Content-Type": "application/json"} if data is not None else {})
    try:
        urllib.request.urlopen(req).read()
    except urllib.error.HTTPError as e:
        print(f"seed {method} {p} -> {e.code}: {e.read().decode()[:300]}", file=sys.stderr)
        sys.exit(1)
    # The last record generates the whole snapshot series and is much the
    # slowest; without a progress line the run looks hung.
    if i % 50 == 0 or i == len(recs):
        print(f"  seeded {i}/{len(recs)} ({p})", flush=True)
PY
  echo "screenshots: seeding failed; server log:" >&2; cat "$work/server.log" >&2; exit 1
}

node "$root/scripts/ui-drive.js" --url "$base" "$root/scripts/screenshots.steps.js" "$out"
