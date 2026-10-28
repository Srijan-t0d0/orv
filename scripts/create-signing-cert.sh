#!/bin/sh
# Create a self-signed code-signing identity in the login keychain (once per machine).
# Signing every build with the same identity keeps macOS privacy permissions
# (Accessibility, Input Monitoring, Microphone) across rebuilds; ad-hoc builds
# get a new identity each time and lose them.
set -eu
NAME="${1:-ORV Local Signing}"
if security find-certificate -c "$NAME" >/dev/null 2>&1; then
  echo "'$NAME' already exists"
  exit 0
fi
DIR=$(mktemp -d)
trap 'rm -rf "$DIR"' EXIT
cat >"$DIR/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
# macOS LibreSSL writes PKCS#12 the keychain can import.
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -config "$DIR/cert.cnf" -keyout "$DIR/key.pem" -out "$DIR/cert.pem" 2>/dev/null
/usr/bin/openssl pkcs12 -export -inkey "$DIR/key.pem" -in "$DIR/cert.pem" \
  -name "$NAME" -passout pass:orv -out "$DIR/id.p12"
security import "$DIR/id.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
  -P orv -T /usr/bin/codesign
echo "Created '$NAME'. Build with: npm run build:mac"
