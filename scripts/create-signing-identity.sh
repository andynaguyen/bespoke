#!/usr/bin/env bash
# Create the self-signed "Bespoke Local Signing" code-signing identity in the
# login keychain. scripts/install-local.sh signs with it, so macOS keeps the
# Accessibility and Microphone grants across rebuilds. Run it once per Mac.
set -euo pipefail

NAME="${BESPOKE_SIGNING_IDENTITY:-Bespoke Local Signing}"

if security find-identity -p codesigning | grep -q "\"$NAME\""; then
  echo "\"$NAME\" already exists."
  exit 0
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

cat > "$WORK/cfg" <<CFG
[req]
distinguished_name=dn
x509_extensions=ext
prompt=no
[dn]
CN=$NAME
[ext]
basicConstraints=critical,CA:false
keyUsage=critical,digitalSignature
extendedKeyUsage=critical,codeSigning
CFG

# /usr/bin/openssl is LibreSSL, whose PKCS#12 output the keychain accepts.
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout "$WORK/key.pem" -out "$WORK/cert.pem" -config "$WORK/cfg" 2>/dev/null
/usr/bin/openssl pkcs12 -export -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
  -out "$WORK/id.p12" -passout pass:temp
security import "$WORK/id.p12" -k ~/Library/Keychains/login.keychain-db \
  -P temp -T /usr/bin/codesign

# The keychain lists the identity as CSSMERR_TP_NOT_TRUSTED. codesign does not
# need it trusted.
echo "Created \"$NAME\"."
