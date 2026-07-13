#!/bin/sh -eu
# Build the FreeBSD package. Runs on a FreeBSD host — CI runs it inside a
# FreeBSD VM (.github/workflows/release.yml) — with rust and protobuf
# installed (pkg install -y rust protobuf; protoc is needed by yfinance-rs's
# build script).
#
# Usage: pkg/freebsd/build-pkg.sh [output-dir]   (default: repo root)

cd "$(dirname "$0")/../.."
OUT="${1:-.}"

# Single source of truth for the version number: Cargo.toml.
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
[ -n "$VERSION" ] || { echo "could not read version from Cargo.toml" >&2; exit 1; }

# Keep build artifacts out of the workspace: CI rsyncs the workspace back to
# the runner when the VM step finishes, and target/ would make that copy huge.
export CARGO_TARGET_DIR=/tmp/share-tracker-target

cargo build --release

STAGE=/tmp/share-tracker-stage
rm -rf "$STAGE"
mkdir -p "$STAGE/usr/local/bin" "$STAGE/usr/local/etc/rc.d"
install -m 0755 "$CARGO_TARGET_DIR/release/share-tracker" "$STAGE/usr/local/bin/"
install -m 0755 pkg/freebsd/share_tracker "$STAGE/usr/local/etc/rc.d/"
install -m 0644 pkg/freebsd/share-tracker.toml.sample "$STAGE/usr/local/etc/"
# The default cron schedule ships as an editable @sample config file; the
# sample share-tracker.toml points `schedule` at the installed copy.
install -m 0644 schedule.cron "$STAGE/usr/local/etc/share-tracker.cron.sample"

sed "s/__VERSION__/$VERSION/" pkg/freebsd/manifest.ucl > /tmp/+MANIFEST
pkg create -M /tmp/+MANIFEST -p pkg/freebsd/plist -r "$STAGE" -o "$OUT"

echo "built $OUT/share-tracker-$VERSION.pkg"
