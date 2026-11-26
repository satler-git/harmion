#!/usr/bin/env bash
# Build a signed release APK with signing material from 1Password.
#
# Default item layout (matching the stored item):
#   Vault:  dev
#   Item:   Harmion release keystore (Document, harmion-release.jks)
#   Fields: pass, key psas, alias, cert_sha256
#
# Override with OP_VAULT / OP_ITEM / OP_*_LABEL. For local testing set
# HARMION_KEYSTORE (and passwords); 1Password is then not contacted.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"

OP_VAULT="${OP_VAULT:-harmion}"
OP_ITEM="${OP_ITEM:-Harmion release keystore}"
OP_PASSWORD_LABEL="${OP_PASSWORD_LABEL:-pass}"
OP_KEY_PASSWORD_LABEL="${OP_KEY_PASSWORD_LABEL:-key psas}"
OP_ALIAS_LABEL="${OP_ALIAS_LABEL:-alias}"
OP_CERT_LABEL="${OP_CERT_LABEL:-cert_sha256}"
android_home="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"

work="$(mktemp -d)"
cleanup() { rm -rf "${work}"; }
trap cleanup EXIT

stored_cert=""
if [[ -z "${HARMION_KEYSTORE:-}" ]]; then
  if ! command -v op >/dev/null 2>&1; then
    echo "1Password CLI (op) is required, or set HARMION_KEYSTORE explicitly." >&2
    exit 1
  fi
  echo "Fetching signing material from 1Password (${OP_VAULT}/${OP_ITEM})…" >&2
  op document get "${OP_ITEM}" --vault "${OP_VAULT}" \
    --out-file "${work}/harmion-release.jks"
  HARMION_KEYSTORE="${work}/harmion-release.jks"

  item_json="$(op item get "${OP_ITEM}" --vault "${OP_VAULT}" --format=json)"
  read_field() {
    jq -r --arg label "$1" \
      'first(.fields[] | select(.label == $label) | .value) // empty' \
      <<<"${item_json}"
  }
  HARMION_KEYSTORE_PASSWORD="$(read_field "${OP_PASSWORD_LABEL}")"
  HARMION_KEY_ALIAS="$(read_field "${OP_ALIAS_LABEL}")"
  [[ -n "${HARMION_KEY_ALIAS}" ]] || HARMION_KEY_ALIAS="harmion"
  stored_cert="$(read_field "${OP_CERT_LABEL}")"
  HARMION_KEY_PASSWORD="$(read_field "${OP_KEY_PASSWORD_LABEL}")"
  if [[ -z "${HARMION_KEY_PASSWORD}" ]]; then
    for fallback in key_password "key pass" "key password"; do
      HARMION_KEY_PASSWORD="$(read_field "${fallback}")"
      [[ -n "${HARMION_KEY_PASSWORD}" ]] && break
    done
  fi
  if [[ -z "${HARMION_KEY_PASSWORD}" ]]; then
    HARMION_KEY_PASSWORD="${HARMION_KEYSTORE_PASSWORD}"
  fi
fi

for name in HARMION_KEYSTORE_PASSWORD HARMION_KEY_ALIAS HARMION_KEY_PASSWORD; do
  if [[ -z "${!name:-}" ]]; then
    echo "${name} is empty" >&2
    exit 1
  fi
done

chmod 600 "${HARMION_KEYSTORE}"

# The SDK pins the node app's signing certificate. Compute the digest from the
# key before building so the pin and the APK are produced from the same key.
cert="${work}/release-cert.der"
keytool -exportcert \
  -keystore "${HARMION_KEYSTORE}" \
  -alias "${HARMION_KEY_ALIAS}" \
  -storepass "${HARMION_KEYSTORE_PASSWORD}" \
  -file "${cert}" >/dev/null
cert_sha256="$(sha256sum "${cert}" | awk '{print $1}')"

if [[ -n "${stored_cert}" && "${stored_cert}" != "${cert_sha256}" ]]; then
  echo "cert_sha256 in 1Password does not match the keystore" >&2
  echo "  1Password: ${stored_cert}" >&2
  echo "  keystore:  ${cert_sha256}" >&2
  exit 1
fi

echo "Signing certificate SHA-256: ${cert_sha256}" >&2

mkdir -p target/release
(
  cd android
  HARMION_KEYSTORE="${HARMION_KEYSTORE}" \
  HARMION_KEYSTORE_PASSWORD="${HARMION_KEYSTORE_PASSWORD}" \
  HARMION_KEY_ALIAS="${HARMION_KEY_ALIAS}" \
  HARMION_KEY_PASSWORD="${HARMION_KEY_PASSWORD}" \
    gradle --no-daemon :app:assembleRelease \
      -PharmionNodeCertSha256="${cert_sha256}"
)

apk="android/app/build/outputs/apk/release/app-release.apk"
if [[ ! -f "${apk}" ]]; then
  echo "release APK was not produced at ${apk}" >&2
  exit 1
fi

out="target/release/harmion-release.apk"
cp "${apk}" "${out}"

if [[ -n "${android_home}" ]]; then
  apksigner="${android_home}/build-tools/37.0.0/apksigner"
else
  apksigner="$(command -v apksigner || true)"
fi
if [[ -x "${apksigner}" ]]; then
  "${apksigner}" verify --print-certs "${out}" | tee "${work}/verify.txt"
  if ! grep -qi "${cert_sha256}" "${work}/verify.txt"; then
    echo "APK certificate does not match the signing keystore" >&2
    exit 1
  fi
else
  echo "warning: apksigner not found; signature was not verified" >&2
fi

echo "signed release: ${out}" >&2
