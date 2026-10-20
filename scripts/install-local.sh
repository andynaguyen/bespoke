#!/usr/bin/env bash
# Build Handy for macOS and replace /Applications/Handy.app with the result.
#
# Signs with the "Handy Local Signing" identity from BUILD.md so the
# Accessibility grant survives the reinstall. Pass --no-build to reinstall the
# last build without rebuilding.
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="Handy"
BUILT_APP="src-tauri/target/release/bundle/macos/$APP_NAME.app"
INSTALLED_APP="/Applications/$APP_NAME.app"

if [[ "${1:-}" != "--no-build" ]]; then
  APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:-Handy Local Signing}" \
    CMAKE_POLICY_VERSION_MINIMUM=3.5 \
    bun run tauri build --bundles app
fi

if [[ ! -d "$BUILT_APP" ]]; then
  echo "No build found at $BUILT_APP" >&2
  exit 1
fi

if pgrep -xq handy; then
  echo "Quitting $APP_NAME..."
  osascript -e "quit app \"$APP_NAME\"" || true
  for _ in {1..20}; do
    if ! pgrep -xq handy; then
      break
    fi
    sleep 0.25
  done
  if pgrep -xq handy; then
    pkill -x handy
    sleep 1
  fi
fi

echo "Installing to $INSTALLED_APP..."
rm -rf "$INSTALLED_APP"
ditto "$BUILT_APP" "$INSTALLED_APP"

open "$INSTALLED_APP"
echo "Installed and relaunched $APP_NAME."
