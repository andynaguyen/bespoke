#!/usr/bin/env bash
# Build the Bespoke.app you share: a release build with Sparkle embedded,
# zipped, with a Sparkle appcast for it. Writes dist/<version>/.
#
#   scripts/release.sh             build dist/<version>/ only
#   scripts/release.sh --publish   also create release v<version> on GitHub
#   scripts/release.sh --no-build  reuse the last cargo build
#
# Bump `version` in Cargo.toml first: Sparkle offers an update only when the
# feed's version is newer than the running app's. release-notes/<version>.md,
# if present, becomes the notes in Sparkle's update window.
#
# The release goes to the public repo $BESPOKE_RELEASES_REPO (default
# andynaguyen/bespoke-releases). Every release attaches appcast.xml, so
# Info.plist's SUFeedURL, .../releases/latest/download/appcast.xml, always
# serves the newest one.
#
# Sparkle signs each zip with the EdDSA key in your login keychain, made by
# Sparkle's generate_keys. Back it up with
# `target/sparkle/<version>/bin/generate_keys -x <file>`. Without it, installed
# copies reject every future update.
set -euo pipefail

cd "$(dirname "$0")/.."

SPARKLE_VERSION="2.10.1"
SPARKLE_SHA256="1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27"
RELEASES_REPO="${BESPOKE_RELEASES_REPO:-andynaguyen/bespoke-releases}"

PUBLISH=false
BUILD=true
for arg in "$@"; do
  case "$arg" in
    --publish) PUBLISH=true ;;
    --no-build) BUILD=false ;;
    *) echo "Unknown argument: $arg" >&2; exit 1 ;;
  esac
done

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TAG="v$VERSION"
ZIP_NAME="Bespoke-$VERSION.zip"
DIST="dist/$VERSION"
DOWNLOAD_BASE="${BESPOKE_DOWNLOAD_BASE:-https://github.com/$RELEASES_REPO/releases/download/$TAG}"

if $PUBLISH; then
  if [[ -n "$(git status --porcelain)" ]]; then
    echo "Commit or stash your changes before publishing, so $TAG matches a commit." >&2
    exit 1
  fi
  if gh release view "$TAG" --repo "$RELEASES_REPO" >/dev/null 2>&1; then
    echo "$RELEASES_REPO already has $TAG. Bump the version in Cargo.toml." >&2
    exit 1
  fi
fi

# Sparkle's framework and signing tools, downloaded once per version.
SPARKLE_DIR="target/sparkle/$SPARKLE_VERSION"
if [[ ! -d "$SPARKLE_DIR/Sparkle.framework" ]]; then
  echo "Downloading Sparkle $SPARKLE_VERSION..."
  mkdir -p "$SPARKLE_DIR"
  ARCHIVE="$SPARKLE_DIR.tar.xz"
  curl -fsSL -o "$ARCHIVE" \
    "https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/Sparkle-$SPARKLE_VERSION.tar.xz"
  echo "$SPARKLE_SHA256  $ARCHIVE" | shasum -a 256 -c - >/dev/null
  tar -xf "$ARCHIVE" -C "$SPARKLE_DIR"
  rm "$ARCHIVE"
fi

# Updates signed with any other key fail on every installed copy.
KEYCHAIN_KEY=$("$SPARKLE_DIR/bin/generate_keys" -p)
PLIST_KEY=$(/usr/libexec/PlistBuddy -c "Print :SUPublicEDKey" Info.plist)
if [[ "$KEYCHAIN_KEY" != "$PLIST_KEY" ]]; then
  echo "The keychain's Sparkle key doesn't match SUPublicEDKey in Info.plist." >&2
  exit 1
fi

if $BUILD; then
  CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo build --release
fi
scripts/bundle-app.sh --sparkle "$SPARKLE_DIR/Sparkle.framework"

rm -rf "$DIST"
mkdir -p "$DIST"
ditto -c -k --sequesterRsrc --keepParent target/release/bundle/Bespoke.app "$DIST/$ZIP_NAME"
# Prints `sparkle:edSignature="..." length="..."`.
SIGNATURE=$("$SPARKLE_DIR/bin/sign_update" "$DIST/$ZIP_NAME")
MIN_SYSTEM=$(/usr/libexec/PlistBuddy -c "Print :LSMinimumSystemVersion" Info.plist)

NOTES_FILE="release-notes/$VERSION.md"
DESCRIPTION=""
if [[ -f "$NOTES_FILE" ]]; then
  # Sparkle can't load the notes' images, which What's New embeds.
  NOTES=$(grep -v '^!\[' "$NOTES_FILE" | sed 's/]]>/]]]]><![CDATA[>/g')
  DESCRIPTION="<description sparkle:format=\"markdown\"><![CDATA[$NOTES]]></description>"
fi

cat > "$DIST/appcast.xml" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Bespoke</title>
    <item>
      <title>Bespoke $VERSION</title>
      <pubDate>$(LC_ALL=C date -u "+%a, %d %b %Y %H:%M:%S +0000")</pubDate>
      <sparkle:version>$VERSION</sparkle:version>
      <sparkle:shortVersionString>$VERSION</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>$MIN_SYSTEM</sparkle:minimumSystemVersion>
      $DESCRIPTION
      <enclosure url="$DOWNLOAD_BASE/$ZIP_NAME" type="application/octet-stream" $SIGNATURE />
    </item>
  </channel>
</rss>
EOF

echo "Wrote $DIST/$ZIP_NAME and $DIST/appcast.xml"

if ! $PUBLISH; then
  echo "Run again with --publish to release $TAG on $RELEASES_REPO."
  exit 0
fi

GH_NOTES="$DIST/notes.md"
if [[ -f "$NOTES_FILE" ]]; then
  cp "$NOTES_FILE" "$GH_NOTES"
else
  echo "Bespoke $VERSION" > "$GH_NOTES"
fi
cat >> "$GH_NOTES" <<'EOF'

### Installing

1. Download the zip, open it, and drag Bespoke to Applications.
2. Open Bespoke. macOS says it can't check the app for malicious software.
3. Open System Settings > Privacy & Security, scroll down, and click Open Anyway.

You only do this once. Bespoke installs later versions itself.
EOF

gh release create "$TAG" \
  --repo "$RELEASES_REPO" \
  --title "Bespoke $VERSION" \
  --notes-file "$GH_NOTES" \
  "$DIST/$ZIP_NAME" "$DIST/appcast.xml"
echo "Published $TAG on $RELEASES_REPO."
