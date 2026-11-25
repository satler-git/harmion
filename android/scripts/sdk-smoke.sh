#!/usr/bin/env bash
# End-to-end host smoke test: one JVM runs the node cdylib, a second JVM uses the
# SDK cdylib against its pinned App API. Two JVMs mirror the Android process model
# (the node app and each client app are separate processes).
#
#   nix develop .#android --command ./android/scripts/sdk-smoke.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"

cargo build -p harmion-android -p harmion-sdk-native

work="$(mktemp -d)"
stop_file="${work}/stop"
node_dir="${work}/node"
host_log="${work}/node-host.log"
cleanup() {
  touch "${stop_file}" 2>/dev/null || true
  if [[ -n "${host_pid:-}" ]]; then
    wait "${host_pid}" 2>/dev/null || true
  fi
  rm -rf "${work}"
}
trap cleanup EXIT

javac -d "${work}/classes" \
  android/ffi-smoke/org/harmion/android/core/HarmionNative.java \
  android/ffi-smoke/org/harmion/sdk/core/HarmionNative.java \
  android/ffi-smoke/NodeHost.java \
  android/ffi-smoke/SdkSmoke.java

library_path="${repo_root}/target/debug"
java_bin="${JAVA_HOME:+${JAVA_HOME}/bin/}java"

"${java_bin}" -Djava.library.path="${library_path}" -cp "${work}/classes" \
  NodeHost "${node_dir}" "${stop_file}" >"${host_log}" 2>&1 &
host_pid=$!

for _ in $(seq 1 100); do
  if grep -q "NODE READY" "${host_log}" 2>/dev/null; then
    break
  fi
  if ! kill -0 "${host_pid}" 2>/dev/null; then
    cat "${host_log}"
    exit 1
  fi
  sleep 0.2
done
grep -q "NODE READY" "${host_log}" || { cat "${host_log}"; exit 1; }

"${java_bin}" -Djava.library.path="${library_path}" -cp "${work}/classes" \
  SdkSmoke "${node_dir}"

touch "${stop_file}"
wait "${host_pid}"
grep -q "NODE STOPPED" "${host_log}"
