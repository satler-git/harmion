#!/usr/bin/env bash
# Generate the committed third-party notices and Android license metadata.
#
# Usage:
#   scripts/generate-third-party-notices.sh          # rewrite the checked-in files
#   scripts/generate-third-party-notices.sh --check  # fail if they are stale
#
# Run inside the default dev shell, which provides cargo-about and python3:
#
#   nix develop --command scripts/generate-third-party-notices.sh --check
#
# The Rust side uses cargo-about, the standard cargo license-notice generator.
# Android uses AboutLibraries at build time for the Gradle dependencies; this
# script only adds the Rust native libraries to the same AboutLibraries data.
# Cargo.lock is the input, so the output is reproducible for a given lockfile.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [[ -z "$repo_root" ]]; then
    repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fi
cd "$repo_root"

mode="write"
case "${1:-}" in
    --check) mode="check" ;;
    -h|--help)
        sed -n '2,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    "") ;;
    *)
        echo "unknown argument: $1" >&2
        exit 2
        ;;
esac

require() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "missing $1; run this inside 'nix develop'" >&2
        exit 1
    }
}
require cargo
require cargo-about
require python3

# cargo-about reads crate sources from the local cargo cache. Fetch first so
# generation can run offline and does not depend on a network service for
# license-text lookup.
cargo fetch --locked

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

generate() {
    local manifest="$1" output="$2" template="${3:-about.hbs}"
    mkdir -p "$(dirname "$output")"
    cargo about generate \
        --config "$repo_root/about.toml" \
        --manifest-path "$manifest" \
        --output-file "$output" \
        --locked \
        --offline \
        "$repo_root/$template"
}

version="$(python3 - <<'PY'
import tomllib
with open("Cargo.toml", "rb") as handle:
    print(tomllib.load(handle)["workspace"]["package"]["version"])
PY
)"

generate_artifact() {
    local name="$1" manifest="$2"
    generate "$manifest" "$stage/licenses/$name/THIRD-PARTY-NOTICES.txt"
}

generate_artifact harmion apps/cli/Cargo.toml
generate_artifact harmiond apps/daemon/Cargo.toml
generate_artifact harmion-admin apps/admin/Cargo.toml
generate_artifact wiki-gui apps/wiki-gui/Cargo.toml

# The Android APKs contain two JNI libraries: the node (crates/android) and the
# app SDK (crates/client-ffi). Both are native dependencies of the app, so the
# same file is bundled into the node and Mote AboutLibraries data.
generate crates/android/Cargo.toml "$stage/android-node.txt" about-component.hbs
generate crates/client-ffi/Cargo.toml "$stage/android-sdk.txt" about-component.hbs
mkdir -p "$stage/licenses/android-native"
{
    echo "Harmion Android native third-party notices"
    echo "=========================================="
    echo
    echo "The Harmion Android apps include native libraries built from the Rust"
    echo "crates below. Their Gradle dependencies are listed separately by"
    echo "AboutLibraries in the same screen."
    echo
    echo "crates/android"
    echo "--------------"
    cat "$stage/android-node.txt"
    echo
    echo "crates/client-ffi"
    echo "-----------------"
    cat "$stage/android-sdk.txt"
} > "$stage/licenses/android-native/THIRD-PARTY-NOTICES.txt"

# Add the Rust notices to the AboutLibraries data as one searchable entry. The
# Android Gradle dependencies are collected by AboutLibraries itself.
mkdir -p "$stage/android/licenses/licenses" "$stage/android/licenses/libraries"
python3 - "$stage/licenses/android-native/THIRD-PARTY-NOTICES.txt" "$stage/android/licenses" "$version" <<'PY'
import html
import json
import pathlib
import sys

notice_path, out_dir, version = sys.argv[1:]
out_dir = pathlib.Path(out_dir)
text = pathlib.Path(notice_path).read_text(encoding="utf-8")
# AboutLibraries renders license content as HTML on Android. Escape the plain
# text so angle brackets in "Used by" URLs are not parsed as tags.
content = html.escape(text, quote=False)
(out_dir / "licenses" / "harmion-native.json").write_text(
    json.dumps(
        {
            "hash": "harmion-native",
            "name": "Harmion native dependencies",
            "content": content,
        },
        ensure_ascii=False,
        indent=2,
    )
    + "\n",
    encoding="utf-8",
)
(out_dir / "libraries" / "harmion-native.json").write_text(
    json.dumps(
        {
            "uniqueId": "dev.satler.harmion:native",
            "artifactVersion": version,
            "name": "Harmion native",
            "licenses": ["harmion-native"],
        },
        ensure_ascii=False,
        indent=2,
    )
    + "\n",
    encoding="utf-8",
)
PY

if [[ "$mode" == "check" ]]; then
    failed=0
    if ! diff -rq licenses "$stage/licenses"; then
        failed=1
    fi
    if ! diff -rq android/licenses "$stage/android/licenses"; then
        failed=1
    fi
    if ((failed)); then
        echo "third-party notices are stale; run scripts/generate-third-party-notices.sh" >&2
        exit 1
    fi
    echo "third-party notices are current"
    exit 0
fi

rm -rf licenses android/licenses
mkdir -p licenses android/licenses
cp -R "$stage/licenses/." licenses/
cp -R "$stage/android/licenses/." android/licenses/
echo "wrote licenses and android/licenses"
