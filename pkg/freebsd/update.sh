#!/bin/sh -eu
# Upgrade the installed share-tracker package to the latest GitHub release,
# or to a specific version if given. Manual, host-side counterpart to
# .github/workflows/release.yml: that workflow publishes a release + .pkg on
# every Cargo.toml version bump; this script pulls one down and installs it.
# There is no cron/timer for this — run it by hand (e.g. `doas
# pkg/freebsd/update.sh`) whenever you want to pick up a new release.
#
# Usage: update.sh [-n|--no-backup] [version]
#   version      e.g. "0.5.0", no "v" prefix (default: latest GitHub release)
#   -n/--no-backup  skip the pre-upgrade backup (see below)
#
# Before installing, if the service is running, this takes a one-off backup
# suffixed pre-<version> via POST /jobs/backup?suffix=pre-<version> (see
# docs/API.md#jobs) — a rollback point taken right before the upgrade, on top
# of the weekly scheduled backup which may be up to a week stale. A backup
# failure aborts the upgrade before pkg add touches anything. If the service
# is not running (e.g. first install), there is nothing to back up and the
# step is skipped with a warning — pass -n/--no-backup to skip it
# deliberately.

# Set in the body, not only the shebang: `sh update.sh` ignores shebang flags.
set -eu

REPO="evanclarke/share-tracker"

NO_BACKUP=0
WANT=""
for arg in "$@"; do
  case "$arg" in
    -n|--no-backup) NO_BACKUP=1 ;;
    -*) echo "unknown option: $arg" >&2; exit 1 ;;
    *) WANT="$arg" ;;
  esac
done

if [ -z "$WANT" ]; then
  API_JSON=$(fetch -qo - "https://api.github.com/repos/$REPO/releases/latest") \
    || { echo "could not reach the GitHub API" >&2; exit 1; }
  TAG=$(printf '%s\n' "$API_JSON" | grep -m1 '"tag_name"' \
    | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')
  [ -n "$TAG" ] || { echo "could not parse a tag_name from the GitHub API response" >&2; exit 1; }
  WANT="${TAG#v}"
fi

CURRENT=$(pkg query %v share-tracker 2>/dev/null || true)

if [ "$CURRENT" = "$WANT" ]; then
  echo "share-tracker $CURRENT is already the requested version"
  exit 0
fi

echo "installed: ${CURRENT:-none}; installing: $WANT"

PKGFILE="/tmp/share-tracker-$WANT.pkg"
fetch -o "$PKGFILE" \
  "https://github.com/$REPO/releases/download/v$WANT/share-tracker-$WANT.pkg"

# Was the service running before the upgrade? Restart afterwards only if so
# — a disabled/never-started service should stay that way. onestatus's exit
# code is the running/not-running signal (rc.subr's, not this script's).
WAS_RUNNING=0
service share_tracker onestatus >/dev/null 2>&1 && WAS_RUNNING=1

# Read a `key = value` line from the active config (value quoted, as `host`
# is, or bare, as the numeric `port` is), ignoring commented-out lines — the
# same file/precedence the running server itself uses (only --config is ever
# passed on the command line; see pkg/freebsd/share_tracker). Anchored at the
# start of the line, so `# port = 9999` never matches.
toml_value() {
  # $1 = key, $2 = file
  sed -n -E "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\"?([^\"[:space:]#]+)\"?.*/\1/p" "$2" | head -n1
}

pre_upgrade_backup() {
  if [ "$NO_BACKUP" = 1 ]; then
    echo "skipping pre-upgrade backup (--no-backup)"
    return 0
  fi
  if [ "$WAS_RUNNING" != 1 ]; then
    echo "service not running; skipping pre-upgrade backup (nothing to back up yet)"
    return 0
  fi

  command -v curl >/dev/null 2>&1 || {
    echo "update.sh needs curl to trigger the pre-upgrade backup (POST can't be" >&2
    echo "done with fetch(1)); install it (pkg install -y curl) or re-run with" >&2
    echo "--no-backup" >&2
    exit 1
  }

  CONF=$(sysrc -n share_tracker_config 2>/dev/null || true)
  CONF="${CONF:-/usr/local/etc/share-tracker.toml}"
  HOST="127.0.0.1"
  PORT="3000"
  AUTH_HEADER=""
  if [ -f "$CONF" ]; then
    CONF_HOST=$(toml_value host "$CONF")
    CONF_PORT=$(toml_value port "$CONF")
    [ -n "$CONF_HOST" ] && HOST="$CONF_HOST"
    [ -n "$CONF_PORT" ] && PORT="$CONF_PORT"
    # [auth].api_token, if configured (see the README's "Authentication"
    # section) — toml_value matches by key name anywhere in the file, and
    # api_token appears only under [auth], so no section-awareness is needed.
    CONF_TOKEN=$(toml_value api_token "$CONF")
    [ -n "$CONF_TOKEN" ] && AUTH_HEADER="Authorization: Bearer $CONF_TOKEN"
  fi
  # 0.0.0.0 means "listen on every interface", not a reachable address —
  # talk to it over loopback like everything else on this host does.
  [ "$HOST" = "0.0.0.0" ] && HOST="127.0.0.1"

  echo "taking pre-upgrade backup (suffix pre-$WANT) via http://$HOST:$PORT ..."
  # Built as positional params (POSIX sh has no arrays) so the optional
  # -H "Authorization: Bearer ..." pair is passed as one argument to curl
  # rather than risking it being re-split by the shell.
  set -- curl -fsS -m 900 -X POST
  [ -n "$AUTH_HEADER" ] && set -- "$@" -H "$AUTH_HEADER"
  set -- "$@" "http://$HOST:$PORT/jobs/backup?suffix=pre-$WANT"
  if ! "$@" >/dev/null; then
    echo "pre-upgrade backup failed; aborting upgrade (database untouched)." >&2
    if [ -z "$AUTH_HEADER" ] && grep -q '^\[auth\]' "$CONF" 2>/dev/null; then
      echo "[auth] is configured but no api_token was found in it — add one" >&2
      echo "(share-tracker gen-token) so this script can authenticate, or" >&2
      echo "re-run with --no-backup." >&2
    fi
    echo "see /var/log/share-tracker.log and GET /jobs for the reason." >&2
    exit 1
  fi
  echo "pre-upgrade backup complete"
}

pre_upgrade_backup

# -f: pkg add refuses to reinstall an already-present package by default;
# upgrading in place needs it. Harmless on a fresh box too (nothing to force).
pkg add -f "$PKGFILE"
rm -f "$PKGFILE"

/usr/local/bin/share-tracker --version | grep -qx "share-tracker $WANT" \
  || { echo "installed binary does not report version $WANT" >&2; exit 1; }

if [ "$WAS_RUNNING" = 1 ]; then
  service share_tracker restart
  echo "share-tracker $WANT installed and service restarted"
else
  echo "share-tracker $WANT installed; service was not running, so it was not started" \
       "(see README \"Installing on FreeBSD\" for first-time setup)"
fi
