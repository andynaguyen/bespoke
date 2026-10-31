#!/usr/bin/env bash
# Store the "Bespoke Local Signing" identity in the releases repo's Actions
# secrets, BESPOKE_SIGNING_P12 and BESPOKE_SIGNING_P12_PASSWORD, for
# .github/workflows/release.yml. macOS asks for your login password once for
# each private key in the keychain.
set -euo pipefail

NAME="${BESPOKE_SIGNING_IDENTITY:-Bespoke Local Signing}"
REPO="${BESPOKE_RELEASES_REPO:-andynaguyen/bespoke}"
KEYCHAIN="${BESPOKE_KEYCHAIN:-$HOME/Library/Keychains/login.keychain-db}"
# LibreSSL writes the 3DES .p12 that `security import` reads.
OPENSSL=/usr/bin/openssl

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
PASSWORD=$(uuidgen)

# security export can't pick one identity, so export them all and keep NAME's
# certificate and the key that shares its localKeyID.
security export -k "$KEYCHAIN" -t identities -f pkcs12 -P "$PASSWORD" -o "$WORK/all.p12"
"$OPENSSL" pkcs12 -in "$WORK/all.p12" -passin "pass:$PASSWORD" -nokeys -clcerts \
  -out "$WORK/certs.pem"
"$OPENSSL" pkcs12 -in "$WORK/all.p12" -passin "pass:$PASSWORD" -nocerts -nodes \
  -out "$WORK/keys.pem"

# Prints each bag with a line that contains both `field` and `value`. Bags
# start at "Bag Attributes".
pick() {
  awk -v field="$1" -v value="$2" '
    /^Bag Attributes/ { if (keep) printf "%s", bag; bag = ""; keep = 0 }
    { bag = bag $0 "\n" }
    index($0, field) && index($0, value) { keep = 1 }
    END { if (keep) printf "%s", bag }
  ' "$3"
}

CERT=$(pick "subject=" "CN=$NAME" "$WORK/certs.pem")
if [[ -z "$CERT" ]]; then
  CERT=$(pick "subject=" "CN = $NAME" "$WORK/certs.pem")
fi
KEY_ID=$(printf '%s\n' "$CERT" | sed -n 's/^ *localKeyID: *//p' | head -1)
if [[ -z "$KEY_ID" ]]; then
  echo "No \"$NAME\" identity in $KEYCHAIN. Run scripts/create-signing-identity.sh first." >&2
  exit 1
fi
printf '%s\n' "$CERT" > "$WORK/cert.pem"
pick "localKeyID:" "$KEY_ID" "$WORK/keys.pem" > "$WORK/key.pem"
if ! grep -q "PRIVATE KEY" "$WORK/key.pem"; then
  echo "Found the \"$NAME\" certificate but not its private key." >&2
  exit 1
fi

"$OPENSSL" pkcs12 -export -in "$WORK/cert.pem" -inkey "$WORK/key.pem" -name "$NAME" \
  -passout "pass:$PASSWORD" -out "$WORK/identity.p12"
base64 -i "$WORK/identity.p12" | gh secret set BESPOKE_SIGNING_P12 --repo "$REPO"
printf '%s' "$PASSWORD" | gh secret set BESPOKE_SIGNING_P12_PASSWORD --repo "$REPO"
echo "Stored \"$NAME\" in $REPO's Actions secrets."
