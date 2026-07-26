#!/bin/sh -eu
# Upgrade the installed share-tracker package to the latest GitHub release,
# or to a specific version if given. Manual, host-side counterpart to
# .github/workflows/release.yml: that workflow publishes a release + .pkg on
# every Cargo.toml version bump; this script pulls one down and installs it.
# There is no cron/timer for this — run it by hand (e.g. `doas
# pkg/freebsd/update.sh`) whenever you want to pick up a new release.
#
# Usage: update.sh [version]   (default: latest GitHub release; version is
#                                e.g. "0.5.0", no "v" prefix)

# Set in the body, not only the shebang: `sh update.sh` ignores shebang flags.
set -eu

REPO="evanclarke/share-tracker"

WANT="${1:-}"
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
