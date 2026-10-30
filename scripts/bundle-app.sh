#!/usr/bin/env bash
# Wrap target/release/bespoke in target/release/bundle/Bespoke.app and sign it.
# Shared by install-local.sh and release.sh. Run `cargo build --release` first.
#
# Signs with $BESPOKE_SIGNING_IDENTITY (default "Bespoke Local Signing", made
# by scripts/create-signing-identity.sh). Pass --sparkle <Sparkle.framework>
# to embed Sparkle, which turns on self-updates (src/updater.rs).
# $BESPOKE_FEED_URL replaces Info.plist's SUFeedURL, for testing against a
# local feed.
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="Bespoke"
IDENTITY="${BESPOKE_SIGNING_IDENTITY:-Bespoke Local Signing}"
STAGED_APP="target/release/bundle/$APP_NAME.app"
SPARKLE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --sparkle) SPARKLE="$2"; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

if [[ ! -x target/release/bespoke ]]; then
  echo "No build found at target/release/bespoke" >&2
  exit 1
fi

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

rm -rf "$STAGED_APP"
mkdir -p "$STAGED_APP/Contents/MacOS" "$STAGED_APP/Contents/Resources"
cp target/release/bespoke "$STAGED_APP/Contents/MacOS/bespoke"
# Annotate's Swift helper (speech, keyframe decoding, permission checks), built by
# vendor/scrawl/build.rs beside the binary.
cp target/release/scrawl-helper "$STAGED_APP/Contents/MacOS/scrawl-helper"
cp icons/Bespoke.icns "$STAGED_APP/Contents/Resources/Bespoke.icns"
# The app looks for its sounds, icons and VAD model in Contents/Resources/resources.
ditto resources "$STAGED_APP/Contents/Resources/resources"
cp Info.plist "$STAGED_APP/Contents/Info.plist"
/usr/libexec/PlistBuddy \
  -c "Set :CFBundleShortVersionString $VERSION" \
  -c "Set :CFBundleVersion $VERSION" \
  "$STAGED_APP/Contents/Info.plist"
if [[ -n "${BESPOKE_FEED_URL:-}" ]]; then
  /usr/libexec/PlistBuddy -c "Set :SUFeedURL $BESPOKE_FEED_URL" "$STAGED_APP/Contents/Info.plist"
fi

sign() {
  codesign --force --options runtime --sign "$IDENTITY" "$@"
}

# Nested code is signed first, or the app's signature fails --strict.
if [[ -n "$SPARKLE" ]]; then
  FRAMEWORK="$STAGED_APP/Contents/Frameworks/Sparkle.framework"
  mkdir -p "$STAGED_APP/Contents/Frameworks"
  ditto "$SPARKLE" "$FRAMEWORK"
  # The XPC services are only for sandboxed apps, and Bespoke isn't one.
  rm -rf "$FRAMEWORK/Versions/B/XPCServices" "$FRAMEWORK/XPCServices"
  sign "$FRAMEWORK/Versions/B/Autoupdate"
  sign "$FRAMEWORK/Versions/B/Updater.app"
  sign "$FRAMEWORK"
fi
sign --entitlements Entitlements.plist \
  --identifier com.andynguyen.bespoke.scrawl-helper \
  "$STAGED_APP/Contents/MacOS/scrawl-helper"
sign --entitlements Entitlements.plist "$STAGED_APP"
codesign --verify --strict "$STAGED_APP"

echo "Bundled $APP_NAME $VERSION at $STAGED_APP"
