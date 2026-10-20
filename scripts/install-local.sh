#!/usr/bin/env bash
# Build Bespoke for macOS, wrap it in Bespoke.app, sign it, and replace
# /Applications/Bespoke.app with it.
#
# Signs with $BESPOKE_SIGNING_IDENTITY (default "Bespoke Local Signing", made
# by scripts/create-signing-identity.sh). A stable identity keeps the
# Accessibility grant across reinstalls. Pass --no-build to reinstall the last
# build without rebuilding.
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="Bespoke"
IDENTITY="${BESPOKE_SIGNING_IDENTITY:-Bespoke Local Signing}"
STAGED_APP="target/release/bundle/$APP_NAME.app"
INSTALLED_APP="/Applications/$APP_NAME.app"

if [[ "${1:-}" != "--no-build" ]]; then
  CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo build --release
fi

if [[ ! -x target/release/bespoke ]]; then
  echo "No build found at target/release/bespoke" >&2
  exit 1
fi

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

rm -rf "$STAGED_APP"
mkdir -p "$STAGED_APP/Contents/MacOS" "$STAGED_APP/Contents/Resources"
cp target/release/bespoke "$STAGED_APP/Contents/MacOS/bespoke"
cp icons/Bespoke.icns "$STAGED_APP/Contents/Resources/Bespoke.icns"
# The app looks for its sounds, icons and VAD model in Contents/Resources/resources.
ditto resources "$STAGED_APP/Contents/Resources/resources"
cp Info.plist "$STAGED_APP/Contents/Info.plist"
/usr/libexec/PlistBuddy \
  -c "Set :CFBundleShortVersionString $VERSION" \
  -c "Set :CFBundleVersion $VERSION" \
  "$STAGED_APP/Contents/Info.plist"

codesign --force --options runtime --entitlements Entitlements.plist \
  --sign "$IDENTITY" "$STAGED_APP"
codesign --verify --strict "$STAGED_APP"

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
