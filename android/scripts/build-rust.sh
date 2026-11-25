#!/usr/bin/env bash
# Build the Harmion Node and SDK cdylibs for one or more Android ABIs.
#
# Run inside the flake shell that provides the SDK, NDK and the Android Rust
# targets:
#
#   nix develop .#android --command ./android/scripts/build-rust.sh [abi...]
#
# ABI names follow jniLibs/ (arm64-v8a, armeabi-v7a, x86_64). The resulting
# libraries are placed under android/app and android/sdk jniLibs directories.
set -euo pipefail

if [[ -z "${ANDROID_NDK_ROOT:-}" ]]; then
  echo "ANDROID_NDK_ROOT is not set; run inside 'nix develop .#android'" >&2
  exit 2
fi

# 29 is the minimum supported API level of the app (Android 10).
API=29
HOST_TAG=linux-x86_64
TOOLCHAIN="${ANDROID_NDK_ROOT}/toolchains/llvm/prebuilt/${HOST_TAG}/bin"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
node_out="${repo_root}/android/app/src/main/jniLibs"
sdk_out="${repo_root}/android/sdk/src/main/jniLibs"

abis=("$@")
if [[ ${#abis[@]} -eq 0 ]]; then
  abis=(arm64-v8a)
fi

abi_target() {
  case "$1" in
    arm64-v8a) echo "aarch64-linux-android" ;;
    armeabi-v7a) echo "armv7-linux-androideabi" ;;
    x86_64) echo "x86_64-linux-android" ;;
    x86) echo "i686-linux-android" ;;
    *) echo "unknown ABI: $1" >&2; return 1 ;;
  esac
}

build_crate() {
  local crate="$1" target="$2" abi="$3" source="$4" destination="$5"
  local clang="${TOOLCHAIN}/${target}${API}-clang"
  if [[ ! -x "${clang}" ]]; then
    case "${target}" in
      armv7-linux-androideabi) clang="${TOOLCHAIN}/armv7a-linux-androideabi${API}-clang" ;;
      i686-linux-android) clang="${TOOLCHAIN}/i686-linux-android${API}-clang" ;;
    esac
  fi
  if [[ ! -x "${clang}" ]]; then
    echo "missing NDK compiler: ${clang}" >&2
    exit 1
  fi
  export "CC_${target//-/_}=${clang}"
  export "AR_${target//-/_}=${TOOLCHAIN}/llvm-ar"
  export "CARGO_TARGET_$(echo "${target}" | tr 'a-z-' 'A-Z_')_LINKER=${clang}"

  echo "building ${crate} for ${target} (${abi}, API ${API})"
  ( cd "${repo_root}" && cargo build --release -p "${crate}" --target "${target}" )

  mkdir -p "${destination}"
  cp "${repo_root}/target/${target}/release/${source}" "${destination}/"
  echo "wrote ${destination}/${source}"
}

for abi in "${abis[@]}"; do
  target="$(abi_target "${abi}")"
  build_crate harmion-android "${target}" "${abi}" \
    libharmion_android.so "${node_out}/${abi}"
  build_crate harmion-sdk-native "${target}" "${abi}" \
    libharmion_sdk_native.so "${sdk_out}/${abi}"
done
