#!/usr/bin/env bash
# Build and sign one Android app from android/release-apps.toml.
#
# Usage: android/scripts/release-app.sh <tag>
#   e.g. node-v0.1.0 / mote-v0.1.0
#
# Run inside `nix develop .#android`. Signing material comes from 1Password
# unless HARMION_KEYSTORE is set.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

tag="${1:-${TAG:-}}"
[[ -n "$tag" ]] || {
    echo "usage: $0 <tag>" >&2
    exit 2
}

manifest="android/release-apps.toml"
[[ -f "$manifest" ]] || {
    echo "missing $manifest" >&2
    exit 1
}

# Parse the manifest for the app whose tag_prefix matches.
manifest_env="$(
    python3 - "$tag" "$manifest" <<'PY'
import shlex
import sys
import tomllib

tag, path = sys.argv[1], sys.argv[2]
with open(path, "rb") as handle:
    data = tomllib.load(handle)

for app in data.get("app", []):
    prefix = app.get("tag_prefix", "")
    if prefix and tag.startswith(prefix):
        fields = {
            "APP_MODULE": app["module"],
            "APP_PREFIX": prefix,
            "APP_APPLICATION_ID": app.get("application_id", ""),
            "APP_VERSION_FILE": app["version_file"],
            "APP_OP_ITEM": app.get("op_item", ""),
            "APP_ARTIFACT": app.get("artifact", app["module"]),
            "APP_SHIP": "true" if app.get("ship", False) else "false",
        }
        for key, value in fields.items():
            print(f"{key}={shlex.quote(str(value))}")
        break
else:
    raise SystemExit(f"no app entry matches tag {tag}")
PY
)"
eval "$manifest_env"

[[ "$APP_SHIP" == "true" ]] || {
    echo "$tag is registered with ship = false" >&2
    exit 1
}

version="${tag#"$APP_PREFIX"}"
[[ -n "$version" ]] || {
    echo "tag has no version suffix: $tag" >&2
    exit 1
}

read_prop() {
    sed -n "s/^$1=//p" "$2" | tail -n 1
}

version_code="$(read_prop versionCode "$APP_VERSION_FILE")"
version_name="$(read_prop versionName "$APP_VERSION_FILE")"
[[ "$version_code" =~ ^[0-9]+$ ]] || {
    echo "invalid versionCode in $APP_VERSION_FILE" >&2
    exit 1
}
[[ -n "$version_name" ]] || {
    echo "missing versionName in $APP_VERSION_FILE" >&2
    exit 1
}
[[ "$version_name" == "$version" ]] || {
    echo "tag version $version does not match $APP_VERSION_FILE ($version_name)" >&2
    exit 1
}

# versionCode must increase across releases of the same app.
prev_tag="$(git tag --list "${APP_PREFIX}*" --sort=-v:refname | grep -vx "$tag" | head -n 1 || true)"
if [[ -n "$prev_tag" ]]; then
    prev_props="$(git show "$prev_tag:$APP_VERSION_FILE" 2>/dev/null || true)"
    prev_code="$(sed -n 's/^versionCode=//p' <<<"$prev_props" | tail -n 1)"
    if [[ -n "$prev_code" && "$version_code" -le "$prev_code" ]]; then
        echo "versionCode $version_code must be greater than $prev_code from $prev_tag" >&2
        exit 1
    fi
fi

work="$(mktemp -d)"
cleanup() { rm -rf "$work"; }
trap cleanup EXIT

vault="${OP_VAULT:-harmion-release}"
op_item="${APP_OP_ITEM:-}"
alias_name="${HARMION_KEY_ALIAS:-harmion}"
stored_cert=""

if [[ -z "${HARMION_KEYSTORE:-}" ]]; then
    [[ -n "$op_item" ]] || {
        echo "$APP_MODULE has no op_item in $manifest" >&2
        exit 1
    }
    command -v op >/dev/null 2>&1 || {
        echo "1Password CLI (op) is required, or set HARMION_KEYSTORE" >&2
        exit 1
    }
    echo "Fetching signing material from 1Password ($vault/$op_item)..." >&2
    op document get "$op_item" --vault "$vault" --out-file "$work/release.jks"
    HARMION_KEYSTORE="$work/release.jks"

    item_json="$(op item get "$op_item" --vault "$vault" --format=json)"
    read_field() {
        jq -r --arg label "$1" \
            'first(.fields[] | select(.label == $label) | .value) // empty' \
            <<<"$item_json"
    }
    HARMION_KEYSTORE_PASSWORD="$(read_field "${OP_PASSWORD_LABEL:-pass}")"
    alias_name="$(read_field "${OP_ALIAS_LABEL:-alias}")"
    [[ -n "$alias_name" ]] || alias_name="harmion"
    stored_cert="$(read_field "${OP_CERT_LABEL:-cert_sha256}")"
    HARMION_KEY_PASSWORD="$(read_field "${OP_KEY_PASSWORD_LABEL:-key psas}")"
    if [[ -z "$HARMION_KEY_PASSWORD" ]]; then
        for fallback in key_password "key pass" "key password"; do
            HARMION_KEY_PASSWORD="$(read_field "$fallback")"
            [[ -n "$HARMION_KEY_PASSWORD" ]] && break
        done
    fi
    if [[ -z "$HARMION_KEY_PASSWORD" ]]; then
        HARMION_KEY_PASSWORD="$HARMION_KEYSTORE_PASSWORD"
    fi
fi

for name in HARMION_KEYSTORE_PASSWORD HARMION_KEY_PASSWORD; do
    [[ -n "${!name:-}" ]] || {
        echo "$name is empty" >&2
        exit 1
    }
done
chmod 600 "$HARMION_KEYSTORE"

cert="$work/release-cert.der"
keytool -exportcert \
    -keystore "$HARMION_KEYSTORE" \
    -alias "$alias_name" \
    -storepass "$HARMION_KEYSTORE_PASSWORD" \
    -file "$cert" >/dev/null
cert_sha256="$(sha256sum "$cert" | awk '{print $1}')"

if [[ -n "$stored_cert" && "$stored_cert" != "$cert_sha256" ]]; then
    echo "cert_sha256 in 1Password does not match the keystore" >&2
    echo "  1Password: $stored_cert" >&2
    echo "  keystore:  $cert_sha256" >&2
    exit 1
fi
echo "Signing certificate SHA-256: $cert_sha256" >&2

release_abis="${HARMION_RELEASE_ABIS:-arm64-v8a}"
(
    cd android
    HARMION_KEYSTORE="$HARMION_KEYSTORE" \
    HARMION_KEYSTORE_PASSWORD="$HARMION_KEYSTORE_PASSWORD" \
    HARMION_KEY_ALIAS="$alias_name" \
    HARMION_KEY_PASSWORD="$HARMION_KEY_PASSWORD" \
        gradle --no-daemon ":$APP_MODULE:assembleRelease" \
            -PharmionNodeCertSha256="$cert_sha256" \
            -PharmionRustAbis="$release_abis"
)

apk="$(find "android/$APP_MODULE/build/outputs/apk/release" -maxdepth 1 -name '*.apk' -print -quit)"
[[ -n "$apk" ]] || {
    echo "release APK was not produced for $APP_MODULE" >&2
    exit 1
}

out_dir="${HARMION_ARTIFACT_DIR:-target/release}"
mkdir -p "$out_dir"
stem="${APP_ARTIFACT}-v${version}"
out="$out_dir/${stem}.apk"
cp "$apk" "$out"

mapping="android/$APP_MODULE/build/outputs/mapping/release/mapping.txt"
if [[ -f "$mapping" ]]; then
    cp "$mapping" "$out_dir/${stem}-mapping.txt"
fi
printf '%s\n' "$cert_sha256" > "$out_dir/${stem}-cert-sha256.txt"

apksigner="${ANDROID_HOME:-}/build-tools/37.0.0/apksigner"
if [[ ! -x "$apksigner" ]]; then
    apksigner="$(command -v apksigner || true)"
fi
if [[ -n "$apksigner" && -x "$apksigner" ]]; then
    "$apksigner" verify --print-certs "$out" | tee "$work/verify.txt"
    grep -qi "$cert_sha256" "$work/verify.txt" || {
        echo "APK certificate does not match the signing keystore" >&2
        exit 1
    }
else
    echo "warning: apksigner not found; signature was not verified" >&2
fi

(
    cd "$out_dir"
    sha256sum "$(basename "$out")" > "${stem}-SHA256SUMS"
)

echo "APK=$out" >&2
echo "VERSION_CODE=$version_code" >&2
echo "CERT_SHA256=$cert_sha256" >&2
