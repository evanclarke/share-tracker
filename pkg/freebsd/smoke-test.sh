#!/bin/sh -eu
# Install the freshly built package and smoke-test it before anything is
# released: the post-install script runs (service user + data dir), the rc
# script loads, the installed binary reports the expected version, and the
# server starts against the installed config and answers HTTP. Run as root on
# a FreeBSD host (CI: the release workflow's VM, right after build-pkg.sh).

# In the body, not only the shebang — `sh smoke-test.sh` ignores shebang flags.
set -eu

cd "$(dirname "$0")/../.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

pkg add "share-tracker-$VERSION.pkg"

# The installed binary runs (all libraries resolve) and agrees with Cargo.toml
# on the version.
/usr/local/bin/share-tracker --version | grep -qx "share-tracker $VERSION"

# The rc script parses and knows its rcvar.
/usr/local/etc/rc.d/share_tracker rcvar >/dev/null

# post-install created the service user and activated the sample configs
# (the manifest scripts re-implement @sample — see manifest.ucl). Without
# these checks a failed copy would go unnoticed: the server falls back to
# built-in defaults when the config file is absent and still answers HTTP.
pw usershow share_tracker >/dev/null
[ -f /usr/local/etc/share-tracker.toml ]
[ -f /usr/local/etc/share-tracker.cron ]
[ -f /usr/local/etc/newsyslog.conf.d/share-tracker.conf ]

# The server starts and answers HTTP. The @sample config installed to
# /usr/local/etc/share-tracker.toml is loaded for real (including the shipped
# schedule file); only the database path and port are overridden so the test
# never touches the service data directory.
# Teardown must be bounded: an unbounded `service onestop` once wedged the
# release VM for 20 minutes after a failed check. The pkill fallbacks kill
# the daemon(8) supervisor by pidfile even if rc has lost track of it.
stop_service() {
  timeout 30 service share_tracker onestop >/dev/null 2>&1 || true
  pkill -F /var/run/share_tracker/share_tracker.pid 2>/dev/null || true
  pkill -F /tmp/svc-diag.pid 2>/dev/null || true
}

/usr/local/bin/share-tracker --db /tmp/smoke.db --port 3999 &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true; stop_service' EXIT

ok=""
for _ in $(seq 1 30); do
  if fetch -qo /dev/null http://127.0.0.1:3999/reports/health 2>/dev/null; then
    ok=1
    break
  fi
  sleep 1
done
[ -n "$ok" ] || { echo "server did not answer /reports/health" >&2; exit 1; }

# The service also starts through the rc script for real — the direct run
# above can't catch rc plumbing (v0.4.0 shipped a pidfile daemon(8) couldn't
# write after -u dropped to the service user). onestart runs without
# enabling the service in rc.conf; the installed config serves port 3000.
service share_tracker onestart
ok=""
for _ in $(seq 1 30); do
  if fetch -qo /dev/null http://127.0.0.1:3000/reports/health 2>/dev/null; then
    ok=1
    break
  fi
  sleep 1
done
if [ -z "$ok" ]; then
  # The server's output went to syslog (daemon -S); surface everything we
  # can into this log, then re-run the exact service invocation with output
  # to a file so its startup error is visible here.
  echo "service did not answer /reports/health; diagnostics:" >&2
  service share_tracker onestatus || true
  ps -axww | grep -v grep | grep -e share-tracker -e daemon || true
  ls -la /var/db/share-tracker /var/run/share_tracker || true
  tail -n 40 /var/log/share-tracker.log 2>/dev/null || true
  grep -h share-tracker /var/log/messages /var/log/daemon.log 2>/dev/null | tail -n 40 || true
  /usr/sbin/daemon -u share_tracker -P /tmp/svc-diag.pid -o /tmp/svc-diag.log \
    /usr/local/bin/share-tracker --config /usr/local/etc/share-tracker.toml || true
  sleep 5
  echo "--- server output under daemon(8) as the service user:" >&2
  cat /tmp/svc-diag.log >&2 2>/dev/null || true
  exit 1
fi
# daemon(8) wrote the supervisor pidfile, and stop must genuinely stop it —
# unbounded (it once hung CI) and silent "not running" are both failures.
[ -s /var/run/share_tracker/share_tracker.pid ]

# The server's tracing output lands in its own log file (daemon -o), and
# newsyslog rotation must reopen it without restarting the server: force a
# rotation, make the server log fresh lines (a manual job run), and require
# them in the new file — while a fresh startup banner there would mean the
# rotation SIGHUP killed the child and -r restarted it.
[ -s /var/log/share-tracker.log ]
grep -q "share-tracker v$VERSION started" /var/log/share-tracker.log
# Plain text only: tracing must switch ANSI color off when stdout is
# daemon(8)'s pipe rather than a terminal.
if grep -q "$(printf '\033')" /var/log/share-tracker.log; then
  echo "ANSI escape codes in log file" >&2
  exit 1
fi
newsyslog -F /var/log/share-tracker.log
ls /var/log/share-tracker.log.0* >/dev/null
printf 'POST /jobs/backup HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n' \
  | nc -N 127.0.0.1 3000 >/dev/null
ok=""
for _ in $(seq 1 15); do
  if grep -q "job started" /var/log/share-tracker.log 2>/dev/null; then
    ok=1
    break
  fi
  sleep 1
done
[ -n "$ok" ] || {
  echo "no job lines in rotated log (daemon -H reopen failed?)" >&2
  exit 1
}
if grep -q "started, db" /var/log/share-tracker.log; then
  echo "server restarted on rotation (SIGHUP hit the child)" >&2
  exit 1
fi

timeout 30 service share_tracker onestop
# stop must take the server down with the supervisor — daemon(8) forwards
# SIGTERM to the child; a survivor means supervision is miswired.
sleep 1
if pgrep -f '/usr/local/bin/share-tracker --config' >/dev/null; then
  echo "server survived onestop" >&2
  exit 1
fi

echo "smoke test passed: share-tracker $VERSION installs and serves"
