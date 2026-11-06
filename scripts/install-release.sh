#!/usr/bin/env bash
# Replaces /Applications/Bespoke.app with a published release, such as after
# install-local.sh put a dev build there. Pass a tag (v1.0.4) for that release,
# or nothing for the latest.
set -euo pipefail

REPO="andynaguyen/bespoke"
APP_NAME="Bespoke"
INSTALLED_APP="/Applications/$APP_NAME.app"
TAG="${1:-}"

if [[ -z "$TAG" ]]; then
  TAG=$(gh release view -R "$REPO" --json tagName --jq .tagName)
fi
VERSION="${TAG#v}"
ASSET="$APP_NAME-$VERSION.zip"

DIGEST=$(gh release view "$TAG" -R "$REPO" --json assets \
  --jq ".assets[] | select(.name == \"$ASSET\") | .digest")
if [[ -z "$DIGEST" ]]; then
  echo "Release $TAG has no $ASSET" >&2
  exit 1
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

echo "Downloading $ASSET..."
gh release download "$TAG" -R "$REPO" -p "$ASSET" -D "$WORK"

ACTUAL="sha256:$(shasum -a 256 "$WORK/$ASSET" | cut -d' ' -f1)"
if [[ "$ACTUAL" != "$DIGEST" ]]; then
  echo "Digest mismatch for $ASSET: expected $DIGEST, got $ACTUAL" >&2
  exit 1
fi

ditto -x -k "$WORK/$ASSET" "$WORK/extracted"
codesign --verify --strict "$WORK/extracted/$APP_NAME.app"

if pgrep -xq bespoke; then
  echo "Quitting $APP_NAME..."
  osascript -e "quit app \"$APP_NAME\"" || true
  for _ in {1..20}; do
    pgrep -xq bespoke || break
    sleep 0.25
  done
  if pgrep -xq bespoke; then
    pkill -x bespoke
    sleep 1
  fi
fi

echo "Installing to $INSTALLED_APP..."
rm -rf "$INSTALLED_APP"
ditto "$WORK/extracted/$APP_NAME.app" "$INSTALLED_APP"

open "$INSTALLED_APP"
echo "Installed and launched $APP_NAME $VERSION from the $TAG release."
