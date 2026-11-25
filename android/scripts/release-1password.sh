#!/usr/bin/env bash
# Build a signed release APK with signing material from 1Password.
#
# Required 1Password items (override names with env vars):
#   Document: Harmion release keystore  -> OP_KEYSTORE_ITEM
#   Password: Harmion release signing   -> OP_SIGNING_ITEM
#     password     = keystore password
#     key_password = key password (optional, defaults to password)
#     alias        = key alias (optional, defaults to "harmion")
#
# Local override (for testing, never commit a real keystore):
#   HARMION_KEYSTORE=/path/to.jks HARMION_KEYSTORE_PASSWORD=... \
#   HARMION_KEY_ALIAS=harmion HARMION_KEY_PASSWORD=... \
#   ./android/scripts/release-1password.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"

OP_VAULT="${OP_VAULT:-Private}"
OP_KEYSTORE_ITEM="${OP_KEYSTORE_ITEM:-Harmion release keystore}"
OP_SIGNING_ITEM="${OP_SIGNING_ITEM:-Harmion release signing}"
android_home="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"

work="$(mktemp -d)"
cleanup() { rm -rf "${work}"; }
trap cleanup EXIT

if [[ -z "${HARMION_KEYSTORE:-}" ]]; then
  if ! command -v op >/dev/null 2>&1; then
    echo "1Password CLI (op) is required, or set HARMION_KEYSTORE explicitly." >&2
    exit 1
  fi
  echo "Fetching signing material from 1Password (${OP_VAULT})…" >&2
  op document get "${OP_KEYSTORE_ITEM}" --vault "${OP_VAULT}" \
    --out-file "${work}/harmion-release.jks"
  HARMION_KEYSTORE="${work}/harmion-release.jks"
  HARMION_KEYSTORE_PASSWORD="$(op read "op://${OP_VAULT}/${OP_SIGNING_ITEM}/password")"
  HARMION_KEY_ALIAS="$(op read "op://${OP_VAULT}/${OP_SIGNING_ITEM}/alias" 2>/dev/null || echo harmion)"
  HARMION_KEY_PASSWORD="$(
    op read "op://${OP_VAULT}/${OP_SIGNING_ITEM}/key_password" 2>/dev/null \
      || printf '%s' "${HARMION_KEYSTORE_PASSWORD}"
  )"
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
