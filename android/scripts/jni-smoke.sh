#!/usr/bin/env bash
# Host-side JNI smoke test: builds the cdylib for the host and loads it from a
# plain JVM. Run inside the Android dev shell (it provides the JDK):
#
#   nix develop .#android --command ./android/scripts/jni-smoke.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"

cargo build -p harmion-android

work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

javac -d "${work}/classes" \
  android/ffi-smoke/org/harmion/android/core/HarmionNative.java \
  android/ffi-smoke/Smoke.java

java -Djava.library.path="${repo_root}/target/debug" \
  -cp "${work}/classes" Smoke "${work}/node"
