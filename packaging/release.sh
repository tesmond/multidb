#!/bin/sh
# Build the macOS release and publish it to GitHub through the REST API.
# Run from `make release` on a Mac. Replaces the old GitHub Actions workflow.
#
# Usage: release.sh <version>   (normally via `make release VERSION=x.y.z`)
# Sets Cargo.toml's package version (and Cargo.lock) to <version>, commits
# "Release v<version>", pushes it, then builds and publishes v<version>.
# Refuses to run if tag v<version> already exists or the tree is dirty.
#
# Token: $GITHUB_TOKEN (or $GH_TOKEN), falling back to `gh auth token`.
# Needs repo "contents: write" (fine-grained) or "repo" (classic) scope.
#
# DRY_RUN=1 bumps, builds, packages and prints the changelog, then reverts the
# bump; nothing is committed, pushed or published.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

REPO=${REPO:-tesmond/multidb}
API=https://api.github.com
DRY_RUN=${DRY_RUN:-0}

die() { echo "release: $*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "must run on macOS (the build is a .app bundle)"
command -v jq >/dev/null || die "jq is required (brew install jq)"
command -v curl >/dev/null || die "curl is required"

VERSION=${1:-}
VERSION=${VERSION#v}
[ -n "$VERSION" ] || die "usage: release.sh <version>  (e.g. make release VERSION=1.2.3)"
echo "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' \
  || die "version '$VERSION' is not semver (x.y.z)"
TAG="v$VERSION"
CURRENT=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)
[ -n "$CURRENT" ] || die "could not read version from Cargo.toml"

# ─── Pre-flight ──────────────────────────────────────────────────────────────

[ -z "$(git status --porcelain --untracked-files=no)" ] \
  || die "working tree has uncommitted changes - commit or stash them first"

git fetch --quiet --tags origin
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  die "tag $TAG already exists - pick a newer version"
fi

if [ "$DRY_RUN" != 1 ] && [ -z "$(git branch -r --contains HEAD 2>/dev/null)" ]; then
  die "HEAD is not on origin - push it first"
fi

if [ "$DRY_RUN" != 1 ]; then
  TOKEN=${GITHUB_TOKEN:-${GH_TOKEN:-}}
  if [ -z "$TOKEN" ] && command -v gh >/dev/null; then
    TOKEN=$(gh auth token 2>/dev/null || true)
  fi
  [ -n "$TOKEN" ] || die "set GITHUB_TOKEN (or log in with gh) to publish"
fi

# ─── Version bump ────────────────────────────────────────────────────────────

if [ "$CURRENT" != "$VERSION" ]; then
  echo "Bumping version $CURRENT -> $VERSION"
  if [ "$DRY_RUN" = 1 ]; then
    # Put Cargo.toml/Cargo.lock back however the script exits.
    trap 'git checkout -- Cargo.toml Cargo.lock 2>/dev/null || true' EXIT
  fi
  # Only the [package] version (the first version line), not dependencies.
  awk -v v="$VERSION" '!done && /^version *= *"/ { print "version = \"" v "\""; done=1; next } { print }' \
    Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml
  cargo update --workspace --offline --quiet 2>/dev/null || cargo update --workspace --quiet
  if [ "$DRY_RUN" != 1 ]; then
    git add Cargo.toml Cargo.lock
    git commit --quiet -m "Release $TAG"
    git push --quiet origin HEAD || die "pushing the version bump failed"
  fi
else
  echo "Cargo.toml is already at $VERSION"
fi
SHA=$(git rev-parse HEAD)

# ─── Changelog ───────────────────────────────────────────────────────────────

PREV=$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)
RANGE=${PREV:+$PREV..}HEAD

# Commit subjects since the previous tag, minus version-bump noise.
CHANGES=$(git log --no-merges --reverse --pretty='format:%s (%h)' "$RANGE" \
  | grep -viE '^(version|bump version|version bump|bump to v?[0-9.]+|release v?[0-9.]+|v?[0-9]+\.[0-9]+\.[0-9]+)( \(|$)' \
  | sed 's/^/- /' || true)
COUNT=$(printf '%s\n' "$CHANGES" | grep -c '^- ' || true)

NOTES_FILE=$(mktemp)
if [ "$DRY_RUN" = 1 ] && [ "$CURRENT" != "$VERSION" ]; then
  trap 'rm -f "$NOTES_FILE"; git checkout -- Cargo.toml Cargo.lock 2>/dev/null || true' EXIT
else
  trap 'rm -f "$NOTES_FILE"' EXIT
fi
{
  echo "## What's changed"
  echo
  if [ -n "$CHANGES" ]; then
    printf '%s\n' "$CHANGES"
  else
    echo "- Maintenance release"
  fi
  echo
  if [ -n "$PREV" ]; then
    echo "$COUNT change(s) since $PREV."
    echo
    echo "**Full changelog**: https://github.com/$REPO/compare/$PREV...$TAG"
  fi
} > "$NOTES_FILE"

echo "── Release $TAG (${PREV:-first release}) ──"
cat "$NOTES_FILE"
echo

# ─── Build & package ─────────────────────────────────────────────────────────

cargo build --release ${CARGO_FLAGS:-}
VERSION="$VERSION" sh packaging/macos/bundle.sh

ARCH=$(uname -m); [ "$ARCH" = "x86_64" ] || ARCH=arm64
ZIP_NAME="MultiDB-$VERSION-macos-$ARCH.zip"
DIST="$ROOT/target/release"
[ -f "$DIST/$ZIP_NAME" ] || die "$DIST/$ZIP_NAME was not produced"

# The in-app updater refuses to install a zip without its .sha256.
(cd "$DIST" && shasum -a 256 "$ZIP_NAME" > "$ZIP_NAME.sha256")
echo "Packaged $DIST/$ZIP_NAME (+ .sha256)"

if [ "$DRY_RUN" = 1 ]; then
  echo "DRY_RUN=1 - not publishing."
  exit 0
fi

# ─── Publish ─────────────────────────────────────────────────────────────────

gh_api() { # method url [curl args...]
  m=$1; u=$2; shift 2
  curl -fsS -X "$m" "$u" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Accept: application/vnd.github+json" \
    -H "X-GitHub-Api-Version: 2022-11-28" "$@"
}

# Created as a draft so the updater (which reads releases/latest) never sees
# a release whose assets are still uploading; published once they're in.
PAYLOAD=$(jq -n --arg tag "$TAG" --arg sha "$SHA" --arg name "multidb $TAG" \
  --rawfile body "$NOTES_FILE" \
  '{tag_name:$tag, target_commitish:$sha, name:$name, body:$body, draft:true, prerelease:false}')

RELEASE=$(gh_api POST "$API/repos/$REPO/releases" -d "$PAYLOAD") \
  || die "creating the release failed (check the token's permissions)"
RELEASE_ID=$(printf '%s' "$RELEASE" | jq -r .id)
UPLOAD_URL=$(printf '%s' "$RELEASE" | jq -r .upload_url | sed 's/{.*}//')
echo "Created draft release $RELEASE_ID"

for f in "$ZIP_NAME" "$ZIP_NAME.sha256"; do
  case $f in *.zip) ct=application/zip ;; *) ct=text/plain ;; esac
  echo "Uploading $f…"
  gh_api POST "$UPLOAD_URL?name=$f" -H "Content-Type: $ct" \
    --data-binary "@$DIST/$f" -o /dev/null \
    || die "uploading $f failed - draft release $RELEASE_ID left for inspection"
done

URL=$(gh_api PATCH "$API/repos/$REPO/releases/$RELEASE_ID" -d '{"draft":false}' | jq -r .html_url) \
  || die "publishing draft release $RELEASE_ID failed"

git fetch --quiet --tags origin
echo "Published $URL"
