#!/usr/bin/env bash
# One-time release key creation. Store the generated file in 1Password and keep
# a backup; losing it means no future updates with the same application id.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"

out="${1:-target/release-keystore/harmion-release.jks}"
alias="${HARMION_KEY_ALIAS:-harmion}"
dname="${HARMION_DNAME:-CN=Harmion, O=Satler, C=JP}"
vault="${OP_VAULT:-dev}"

if [[ -e "${out}" ]]; then
  echo "refusing to overwrite ${out}" >&2
  exit 1
fi
mkdir -p "$(dirname "${out}")"

if [[ -n "${HARMION_KEYSTORE_PASSWORD:-}" ]]; then
  password="${HARMION_KEYSTORE_PASSWORD}"
else
  # od avoids the SIGPIPE that `tr | head` would cause under pipefail.
  password="$(od -An -tx1 -N20 /dev/urandom | tr -d ' \n')"
fi

keytool -genkeypair \
  -keystore "${out}" \
  -storetype PKCS12 \
  -alias "${alias}" \
  -keyalg RSA \
  -keysize 4096 \
  -validity 10000 \
  -dname "${dname}" \
  -storepass "${password}" \
  -keypass "${password}" >/dev/null

cert="$(mktemp)"
trap 'rm -f "${cert}"' EXIT
keytool -exportcert -keystore "${out}" -alias "${alias}" \
  -storepass "${password}" -file "${cert}" >/dev/null
sha="$(sha256sum "${cert}" | awk '{print $1}')"

cat >&2 <<MSG

Created ${out}
alias: ${alias}
password: ${password}
certificate SHA-256: ${sha}

Store it in 1Password now:
  op document create "${out}" --title "Harmion release keystore" --vault "${vault}"
  op item create --category=password --title "Harmion release signing" --vault "${vault}" \\
    "password=${password}" "key_password=${password}" "alias=${alias}" "cert_sha256=${sha}"

Keep an offline backup. This key signs every update for the application id.
MSG
