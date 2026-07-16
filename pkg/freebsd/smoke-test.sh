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

# The server starts and answers HTTP. The @sample config installed to
# /usr/local/etc/share-tracker.toml is loaded for real (including the shipped
# schedule file); only the database path and port are overridden so the test
# never touches the service data directory.
/usr/local/bin/share-tracker --db /tmp/smoke.db --port 3999 &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true; service share_tracker onestop >/dev/null 2>&1 || true' EXIT

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
[ -n "$ok" ] || { echo "service did not answer /reports/health" >&2; exit 1; }
# daemon(8) wrote the supervisor pidfile, and the rc script can stop by it.
[ -s /var/run/share_tracker/share_tracker.pid ]
service share_tracker onestop

echo "smoke test passed: share-tracker $VERSION installs and serves"
