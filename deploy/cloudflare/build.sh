#!/usr/bin/env bash
# Build the deployable Worker: wasm bridge, generated bindings, and the admin UI.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
cd "$repo"

# ring compiles a few C files for wasm32. Without a target-specific compiler,
# cc-rs falls back to the host gcc and produces x86 objects that rust-lld
# cannot link. Use the unwrapped clang from the dev shell when available.
export CC_wasm32_unknown_unknown="${HARMION_WASM_CC:-clang}"
export CFLAGS_wasm32_unknown_unknown="${HARMION_WASM_CFLAGS:---target=wasm32-unknown-unknown -Wno-unused-command-line-argument}"
export AR_wasm32_unknown_unknown="${HARMION_WASM_AR:-ar}"

cargo build -p harmion-worker --target wasm32-unknown-unknown --release --offline
rm -rf "$here/src/pkg"
wasm-bindgen --target web --out-dir "$here/src/pkg" --out-name harmion_worker_do \
  target/wasm32-unknown-unknown/release/harmion_worker.wasm
# The generated module imports the host boundary as a bare specifier; make it a
# relative module so wrangler and workerd both resolve it from src/pkg.
cp "$here/src/host-api.js" "$here/src/pkg/host-api.js"
sed -i "s|from 'host-api'|from './host-api.js'|; s|from \"host-api\"|from \"./host-api.js\"|" "$here/src/pkg/harmion_worker_do.js"
admin_ui="$(nix build .#admin-ui --no-link --print-out-paths)"
# The Nix output is read-only; a previous copy would otherwise make rm/cp fail.
if [ -d "$here/public" ]; then chmod -R u+w "$here/public"; fi
rm -rf "$here/public"
mkdir -p "$here/public"
cp -R "$admin_ui"/. "$here/public/"
echo "built $here/public and $here/src/pkg"
