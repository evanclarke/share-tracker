#!/bin/sh
# Markdown release notes for v<version>: the commit subjects between the
# previous release tag and HEAD (newest first, each with its abbreviated SHA),
# plus a full-changelog compare link. GitHub's --generate-notes builds notes
# from merged pull requests, which a commit-directly-to-main workflow doesn't
# have — so the notes come from the commits themselves.
#
# Used by .github/workflows/release.yml at release time (needs the full
# history: the workflow checks out with fetch-depth 0). Preview locally with
#   scripts/release-notes.sh 0.3.0
set -eu

VERSION="${1:?usage: release-notes.sh <version>}"
TAG="v$VERSION"
REPO_URL="${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-evanclarke/share-tracker}"

# Nearest tag reachable from HEAD, excluding this release's own tag — a re-run
# after the tag was already pushed must not diff the release against itself.
PREV=$(git describe --tags --abbrev=0 --exclude "$TAG" 2>/dev/null || true)

if [ -n "$PREV" ]; then
  echo "## Changes since $PREV"
  echo
  git log --no-merges --format="- %s (%h)" "$PREV..HEAD"
  echo
  echo "**Full changelog**: $REPO_URL/compare/$PREV...$TAG"
else
  echo "Initial release."
  echo
  git log --no-merges --format="- %s (%h)"
fi
