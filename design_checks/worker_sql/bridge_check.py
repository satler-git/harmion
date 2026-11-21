#!/usr/bin/env python3
"""Run generated Rust/Wasm transactionSync callbacks in local workerd."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile

from check import ROOT, Runtime, require, runtime_default


def report(result):
    for passed in result["passed"]:
        print(f"PASS: {passed}", flush=True)
    return result["state"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workerd", type=Path)
    parser.add_argument("--legacy-trap", action="store_true", help="compare old separate one-shot panic instead of generic shim panic")
    args = parser.parse_args()
    executable = (args.workerd or runtime_default()).resolve()
    require(executable.is_file(), "run the locked npm ci first")
    for artifact in ("bridge.js", "bridge_bg.wasm"):
        require((ROOT / "rust_bridge" / "pkg" / artifact).is_file(),
                "build the isolated Rust workspace and run wasm-bindgen first (see README)")
    generated = (ROOT / "rust_bridge" / "pkg" / "bridge.js").read_text(encoding="utf-8")
    wrapper = generated.split("export function run_shim_transaction(", 1)[1].split("\n}\n", 1)[0]
    require("wasm.run_shim_transaction(storage, through, mode)" in wrapper,
            "unexpected generated shim export shape")
    require(not any(token in wrapper for token in ("finally", "catch", "__wbindgen_free", "__wbindgen_malloc")),
            "generated shim root must not reenter Wasm for cleanup after a trap")
    require('from "bridge-shim"' in generated, "generated bare shim module import missing")
    print("PASS: generated shim root has no post-trap finally/free or string-argument allocation", flush=True)
    os.umask(0o077)
    version = subprocess.run([str(executable), "--version"], text=True, capture_output=True, check=True)
    print(version.stdout.strip(), flush=True)
    with tempfile.TemporaryDirectory(prefix="harmion-rust-bridge-") as temporary:
        directory = Path(temporary)
        data = directory / "data"
        data.mkdir(mode=0o700)
        with (directory / "workerd.log").open("w+") as log:
            runtime = Runtime(executable, data, log, ROOT / "bridge-config.capnp")
            try:
                initial = runtime.start()
                require(initial["activations"] == "1", "expected first activation")
                runtime.forbidden("/unknown")
                runtime.forbidden("/state?query=1")
                runtime.forbidden(headers={"Origin": "https://example.invalid"})
                runtime.forbidden(headers={"Host": "example.invalid"})
                runtime.forbidden(method="POST")
                print("PASS: bridge local test guard rejects unknown/query/Origin/Host/method", flush=True)
                baseline = report(runtime.get("/run"))
                require(baseline["node_id"] == initial["node_id"], "identity changed")
                require(baseline["cursor"] == "18446744073709551615", "u64 max lost precision")
                if not args.legacy_trap:
                    baseline = report(runtime.get("/shim-run"))
                trapped = runtime.get("/panic" if args.legacy_trap else "/shim-panic")
                after_trap = report(trapped)
                print(f"Observed trap boundary: {trapped['diagnostics']}", flush=True)
                require(after_trap == baseline, "trap changed state")
                # No further Rust export invocation in this process. Read state
                # via plain JS, terminate, then instantiate the Wasm module anew.
                require(runtime.get("/state") == baseline, "post-trap JS SQL read changed state")
                runtime.stop()
                reopened = runtime.start()
                require(reopened["activations"] == "2", "restart activation missing")
                for field in ("node_id", "records", "cursor"):
                    require(reopened[field] == baseline[field], f"restart changed {field}")
                report(runtime.get("/fresh"))
                print("PASS: Rust/Wasm error+trap rollback/fail-closed boundary; automatic isolate disposal/unwind still unverified", flush=True)
            except BaseException:
                try:
                    runtime.stop()
                except Exception as cleanup_error:
                    print(f"runtime cleanup error: {cleanup_error}", flush=True)
                log.flush()
                log.seek(0)
                print("workerd log:\n" + log.read(), flush=True)
                raise
            finally:
                runtime.stop()


if __name__ == "__main__":
    main()
