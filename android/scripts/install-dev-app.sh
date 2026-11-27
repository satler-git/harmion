#!/usr/bin/env bash
# Build debuggable APKs, sign them with their production release keys, and
# install them on a device. This is the "production dev build" workflow: the
# APK stays debuggable, but its signature matches the production install, so
# `adb install -r` updates an existing production-signed app without wiping it.
#
# Run inside `nix develop .#android`.
#
# Usage:
#   ./android/scripts/install-dev-app.sh [options]
#
# Options:
#   --app NAME[,NAME...]   App modules from android/release-apps.toml
#                          (default: app). Repeatable.
#   --all                  All apps with ship = true
#   --device SERIAL        adb serial (default: $HARMION_DEVICE, $ANDROID_SERIAL,
#                          or the only connected device)
#   --no-install           Build and sign only
#   --no-build             Sign the existing debug APK instead of rebuilding
#   --skip-rust            Skip the buildRust task (reuse existing jniLibs)
#   --no-launch            Do not launch the app after install
#   --output-dir DIR       Signed APK directory (default: target/prod-signing)
#   --vault NAME           1Password vault (default: $OP_VAULT or harmion)
#   -h, --help             Show this help
#
# Examples:
#   ./android/scripts/install-dev-app.sh
#   ./android/scripts/install-dev-app.sh --app mote --device 192.168.0.8:40345
#   ./android/scripts/install-dev-app.sh --app app,mote --skip-rust --no-launch
#
# Requires an unlocked 1Password desktop app with CLI integration, plus adb,
# jq, and python3 (tomllib). The keystore is fetched to a 0700 temp directory
# and deleted on exit. Passwords are never printed.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"
manifest="android/release-apps.toml"

die() { echo "$*" >&2; exit 1; }

usage() {
    sed -n '2,/^set -euo pipefail$/p' "${BASH_SOURCE[0]}" | sed '$d'
}

apps=()
use_all=false
device="${HARMION_DEVICE:-${ANDROID_SERIAL:-}}"
do_install=true
do_build=true
skip_rust=false
do_launch=true
out_dir="target/prod-signing"
vault="${OP_VAULT:-harmion}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --app)
            [[ -n "${2:-}" ]] || die "--app needs a module name"
            IFS=',' read -r -a values <<<"$2"
            apps+=("${values[@]}")
            shift 2
            ;;
        --app=*)
            IFS=',' read -r -a values <<<"${1#*=}"
            apps+=("${values[@]}")
            shift
            ;;
        --all)
            use_all=true
            shift
            ;;
        --device)
            [[ -n "${2:-}" ]] || die "--device needs an adb serial"
            device="$2"
            shift 2
            ;;
        --device=*)
            device="${1#*=}"
            shift
            ;;
        --no-install)
            do_install=false
            do_launch=false
            shift
            ;;
        --no-build)
            do_build=false
            shift
            ;;
        --skip-rust)
            skip_rust=true
            shift
            ;;
        --no-launch)
            do_launch=false
            shift
            ;;
        --output-dir)
            [[ -n "${2:-}" ]] || die "--output-dir needs a path"
            out_dir="$2"
            shift 2
            ;;
        --output-dir=*)
            out_dir="${1#*=}"
            shift
            ;;
        --vault)
            [[ -n "${2:-}" ]] || die "--vault needs a name"
            vault="$2"
            shift 2
            ;;
        --vault=*)
            vault="${1#*=}"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1 (try --help)"
            ;;
    esac
done

if ! $use_all && [[ ${#apps[@]} -eq 0 ]]; then
    apps=(app)
fi

[[ -f "$manifest" ]] || die "missing $manifest"
command -v python3 >/dev/null 2>&1 || die "python3 is required (run inside nix develop .#android)"
command -v jq >/dev/null 2>&1 || die "jq is required (run inside nix develop .#android)"
: "${ANDROID_HOME:?ANDROID_HOME is not set; run inside nix develop .#android}"

if [[ -n "${OP_BIN:-}" ]]; then
    op_bin="$OP_BIN"
elif [[ -x /run/wrappers/bin/op ]]; then
    # The system wrapper owns the desktop-app integration. The op binary from
    # nixpkgs can be first on PATH inside nix develop and fails to connect.
    op_bin=/run/wrappers/bin/op
else
    op_bin="$(command -v op || true)"
fi
[[ -n "$op_bin" ]] || die "1Password CLI not found; set OP_BIN"
[[ -x "$op_bin" ]] || die "1Password CLI not executable: $op_bin"

apksigner="$(find -L "$ANDROID_HOME/build-tools" -maxdepth 2 -name apksigner -type f 2>/dev/null |
    sort -V | tail -n 1)"
[[ -n "$apksigner" ]] || die "apksigner not found under $ANDROID_HOME/build-tools"

if $do_install; then
    command -v adb >/dev/null 2>&1 || die "adb is required for --install"
    if [[ -z "$device" ]]; then
        mapfile -t devices < <(adb devices | awk 'NR > 1 && $2 == "device" { print $1 }')
        [[ ${#devices[@]} -gt 0 ]] || die "no adb device connected; pass --device"
        [[ ${#devices[@]} -eq 1 ]] ||
            die "multiple adb devices; pass --device (${devices[*]})"
        device="${devices[0]}"
    fi
fi

# Emit module, application_id, 1Password item, and artifact name for the
# selected apps. The manifest stays the single source of truth.
manifest_rows="$(
    python3 - "$manifest" "$use_all" "${apps[@]}" <<'PY'
import sys
import tomllib

path, all_flag, *wanted = sys.argv[1:]
with open(path, "rb") as handle:
    data = tomllib.load(handle)
apps = data.get("app", [])
if all_flag == "true":
    selected = [app for app in apps if app.get("ship", False)]
else:
    by_module = {app["module"]: app for app in apps}
    selected = []
    for module in wanted:
        app = by_module.get(module)
        if app is None:
            raise SystemExit(f"unknown app module: {module}")
        if not app.get("ship", False):
            raise SystemExit(f"{module} is not a shippable app")
        if app not in selected:
            selected.append(app)
if not selected:
    raise SystemExit("no apps selected")
for app in selected:
    print(
        "\t".join(
            [
                app["module"],
                app["application_id"],
                app.get("op_item", ""),
                app.get("artifact", app["module"]),
            ]
        )
    )
PY
)"
[[ -n "$manifest_rows" ]] || die "no app rows parsed from $manifest"

node_op_item="$(
    python3 - "$manifest" <<'PY'
import sys
import tomllib
with open(sys.argv[1], "rb") as handle:
    data = tomllib.load(handle)
print(data.get("node", {}).get("op_item", ""))
PY
)"
[[ -n "$node_op_item" ]] || die "missing [node].op_item in $manifest"

work="$(mktemp -d "target/prod-signing-work.XXXXXX")"
chmod 700 "$work"
cleanup() { rm -rf "$work"; }
trap cleanup EXIT

read_field() {
    jq -r --arg label "$1" \
        'first(.fields[] | select(.label == $label) | .value) // empty' <<<"$2"
}

norm_sha() { tr '[:upper:]' '[:lower:]' <<<"${1//:/}"; }

fetch_item() {
    local item="$1" out="$2"
    timeout 120 "$op_bin" item get "$item" --vault "$vault" --format=json >"$out"
}

echo "Reading node signing certificate from 1Password ($vault/$node_op_item)..." >&2
fetch_item "$node_op_item" "$work/node-item.json"
node_cert="$(norm_sha "$(read_field cert_sha256 "$(cat "$work/node-item.json")")")"
[[ "$node_cert" =~ ^[0-9a-f]{64}$ ]] ||
    die "missing or invalid cert_sha256 for $node_op_item"
echo "Node certificate SHA-256: $node_cert" >&2

mkdir -p "$out_dir"
declare -a installed_apks=()

while IFS=$'\t' read -r module application_id op_item artifact; do
    [[ -n "$module" && -n "$application_id" && -n "$op_item" && -n "$artifact" ]] ||
        die "invalid manifest row for $module"
    echo "=== $module ($application_id) ===" >&2

    keystore="$work/$module.jks"
    item_json="$work/$module-item.json"
    echo "Fetching signing key from 1Password ($vault/$op_item)..." >&2
    timeout 180 "$op_bin" document get "$op_item" --vault "$vault" --out-file "$keystore" >/dev/null
    chmod 600 "$keystore"
    fetch_item "$op_item" "$item_json"

    store_pass="$(read_field pass "$(cat "$item_json")")"
    key_pass="$(read_field 'key psas' "$(cat "$item_json")")"
    if [[ -z "$key_pass" ]]; then
        for label in key_password "key pass" "key password"; do
            key_pass="$(read_field "$label" "$(cat "$item_json")")"
            [[ -n "$key_pass" ]] && break
        done
    fi
    alias_name="$(read_field alias "$(cat "$item_json")")"
    [[ -n "$alias_name" ]] || alias_name="harmion"
    expected_cert="$(norm_sha "$(read_field cert_sha256 "$(cat "$item_json")")")"
    [[ -n "$store_pass" ]] || die "missing pass field in $op_item"
    [[ -n "$key_pass" ]] || key_pass="$store_pass"
    [[ "$expected_cert" =~ ^[0-9a-f]{64}$ ]] ||
        die "missing or invalid cert_sha256 in $op_item"

    keytool -exportcert -keystore "$keystore" -alias "$alias_name" \
        -storepass "$store_pass" -file "$work/$module.der" >/dev/null
    cert="$(norm_sha "$(sha256sum "$work/$module.der" | cut -d' ' -f1)")"
    [[ "$cert" == "$expected_cert" ]] ||
        die "certificate mismatch for $op_item ($cert != $expected_cert)"

    if $do_build; then
        echo "Building :$module:assembleDebug..." >&2
        gradle_args=(
            ":$module:assembleDebug"
            "-PharmionNodeCertSha256=$node_cert"
            "--console=plain"
        )
        $skip_rust && gradle_args+=("-x" "buildRust")
        (cd android && gradle "${gradle_args[@]}")
    fi

    raw_apk="$(find "android/$module/build/outputs/apk/debug" -maxdepth 1 -name '*.apk' -print -quit 2>/dev/null)"
    [[ -n "$raw_apk" ]] ||
        die "debug APK not found for $module; drop --no-build or build :$module:assembleDebug"

    signed_apk="$out_dir/$artifact-debug-prod.apk"
    cp "$raw_apk" "$work/$module-raw.apk"
    echo "Signing $(basename "$signed_apk")..." >&2
    KSPASS="$store_pass" KEYPASS="$key_pass" \
        "$apksigner" sign --ks "$keystore" --ks-key-alias "$alias_name" \
        --ks-pass env:KSPASS --key-pass env:KEYPASS \
        --out "$signed_apk" "$work/$module-raw.apk" >/dev/null

    actual_cert="$(
        "$apksigner" verify --print-certs "$signed_apk" |
            awk -F': ' '/SHA-256 digest/ { print $3; exit }'
    )"
    [[ "$actual_cert" == "$cert" ]] ||
        die "signed APK certificate mismatch for $module ($actual_cert != $cert)"
    echo "$signed_apk: $actual_cert" >&2

    installed_apks+=("$signed_apk|$application_id")
done <<<"$manifest_rows"

if $do_install; then
    if [[ "$device" == *:* ]]; then
        adb connect "$device" >/dev/null || true
    fi
    adb -s "$device" wait-for-device
    for entry in "${installed_apks[@]}"; do
        apk="${entry%%|*}"
        application_id="${entry##*|}"
        echo "Installing $apk on $device..." >&2
        adb -s "$device" install -r "$apk"
        if $do_launch; then
            if adb -s "$device" shell monkey -p "$application_id" \
                -c android.intent.category.LAUNCHER 1 >/dev/null; then
                echo "Launched $application_id on $device." >&2
            else
                echo "Installed $application_id but could not launch it." >&2
            fi
        fi
    done
else
    echo "Signed APK(s) ready; installation skipped." >&2
fi
