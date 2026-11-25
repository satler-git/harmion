#!/usr/bin/env bash
# Owner approval end-to-end on the host: node cdylib enrolls an Ed25519 owner key,
# the SDK requests a challenge, the node signs it, and the SDK submits the
# assertion. Verification runs in-process, so this is the Android path.
#
#   nix develop .#android --command ./android/scripts/owner-smoke.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"

cargo build -p harmion-android -p harmion-sdk-native

work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

javac -d "${work}/classes" \
  android/ffi-smoke/org/harmion/android/core/HarmionNative.java \
  android/ffi-smoke/org/harmion/sdk/core/HarmionNative.java \
  android/ffi-smoke/OwnerSmoke.java

java_bin="${JAVA_HOME:+${JAVA_HOME}/bin/}java"
"${java_bin}" -Djava.library.path="${repo_root}/target/debug" -cp "${work}/classes" \
  OwnerSmoke "${work}/node"
