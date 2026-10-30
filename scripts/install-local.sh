#!/usr/bin/env bash
# Build Bespoke for macOS, wrap it in Bespoke.app, sign it, and replace
# /Applications/Bespoke.app with it.
#
# Signs with $BESPOKE_SIGNING_IDENTITY (default "Bespoke Local Signing", made
# by scripts/create-signing-identity.sh). A stable identity keeps the
# Accessibility grant across reinstalls. Pass --no-build to reinstall the last
# build without rebuilding. The app doesn't embed Sparkle, so it never updates
# itself; scripts/release.sh builds the copy you share.
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="Bespoke"
STAGED_APP="target/release/bundle/$APP_NAME.app"
INSTALLED_APP="/Applications/$APP_NAME.app"

if [[ "${1:-}" != "--no-build" ]]; then
  CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo build --release
fi

scripts/bundle-app.sh

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

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
ditto "$STAGED_APP" "$INSTALLED_APP"

open "$INSTALLED_APP"
echo "Installed and launched $APP_NAME $VERSION."
