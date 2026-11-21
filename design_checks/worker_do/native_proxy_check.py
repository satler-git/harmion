#!/usr/bin/env python3
"""E2E: native sync loop -> TLS proxy -> workerd Durable Object."""
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(HERE.parent / "worker_sql"))
from check import require, runtime_default  # noqa: E402
import importlib.util  # noqa: E402

_spec = importlib.util.spec_from_file_location("worker_do_check", HERE / "check.py")
_module = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_module)
AssetsRuntime = _module.AssetsRuntime

CONFIG = HERE / "native-config.capnp"


def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def write_config(port, path):
    path.write_text(
        f'''using Workerd = import "/workerd/workerd.capnp";
const config :Workerd.Config = (
  services = [
    (name = "hub", worker = (
      modules = [
        (name = "worker.js", esModule = embed "../../deploy/cloudflare/src/worker.js"),
        (name = "pkg/host-api.js", esModule = embed "../../deploy/cloudflare/src/pkg/host-api.js"),
        (name = "pkg/harmion_worker_do.js", esModule = embed "../../deploy/cloudflare/src/pkg/harmion_worker_do.js"),
        (name = "pkg/harmion_worker_do_bg.wasm", wasm = embed "../../deploy/cloudflare/src/pkg/harmion_worker_do_bg.wasm")
      ],
      compatibilityDate = "2026-11-01",
      bindings = [
        (name = "NODE", durableObjectNamespace = "Node"),
        (name = "CLUSTER", text = "0101010101010101010101010101010101010101010101010101010101010101"),
        (name = "NODE_ID", text = "0303030303030303030303030303030303030303030303030303030303030303"),
        (name = "EPOCH", text = "0303030303030303030303030303030303030303030303030303030303030303"),
        (name = "SEED", text = "0303030303030303030303030303030303030303030303030303030303030303"),
        (name = "ORIGIN", text = "https://localhost:{port}"),
        (name = "ADMIN_TOKEN", text = "local-test-admin-token"),
        (name = "BOOTSTRAP_TOKEN", text = "local-test-bootstrap-token"),
        (name = "SYNC_INTERVAL_MS", text = "3600000")
      ],
      durableObjectNamespaces = [
        (className = "Node", uniqueKey = "harmion-do-native-e2e-v1", enableSql = true)
      ],
      durableObjectStorage = (localDisk = "storage")
    )),
    (name = "assets", assets = (
      path = "../../deploy/cloudflare/public",
      config = "{{\\"run_worker_first\\":[\\"/state\\",\\"/peer/*\\",\\"/admin/*\\",\\"/provision\\",\\"/capabilities\\",\\"/overview\\",\\"/members\\",\\"/peers\\",\\"/provision-status\\",\\"/registration-code\\",\\"/internal/*\\"],\\"not_found_handling\\":\\"404-page\\"}}",
      userWorker = (name = "hub")
    )),
    (name = "storage", disk = (writable = true)),
    (name = "internet", network = (allow = []))
  ],
  sockets = [(name = "probe", http = (), service = "assets")]
);
''',
        encoding="utf-8",
    )


def main():
    executable = runtime_default().resolve()
    require(executable.is_file(), "run npm ci in design_checks/worker_sql first")
    for path in (
        REPO / "deploy/cloudflare/src/pkg/harmion_worker_do.js",
        REPO / "deploy/cloudflare/src/pkg/harmion_worker_do_bg.wasm",
        REPO / "deploy/cloudflare/src/pkg/host-api.js",
        REPO / "deploy/cloudflare/public/index.html",
    ):
        require(path.is_file(), f"run deploy/cloudflare/build.sh first: {path}")
    port = free_port()
    write_config(port, CONFIG)
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix="harmion-native-") as temporary:
        directory = Path(temporary)
        data = directory / "data"
        data.mkdir(mode=0o700)
        with (directory / "workerd.log").open("w+") as log:
            runtime = AssetsRuntime(executable, data, log, HERE / "native-config.capnp")
            try:
                runtime.start()
                environment = dict(os.environ, RUSTC_WRAPPER="")
                result = subprocess.run(
                    [
                        "cargo",
                        "run",
                        "--quiet",
                        "--offline",
                        "-p",
                        "harmion-net-native",
                        "--example",
                        "tls_forward_sync",
                        "--",
                        runtime.url,
                        str(port),
                    ],
                    cwd=REPO,
                    text=True,
                    capture_output=True,
                    env=environment,
                    timeout=180,
                )
                print(result.stdout, end="", flush=True)
                if result.returncode != 0:
                    print(result.stderr, flush=True)
                    raise SystemExit("native sync example failed")
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
                try:
                    runtime.stop()
                except Exception:
                    pass
    CONFIG.unlink(missing_ok=True)
    print("ALL NATIVE PROXY CHECKS PASSED", flush=True)


if __name__ == "__main__":
    main()
