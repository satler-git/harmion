#!/usr/bin/env python3
"""Run the Harmion DO bridge in local workerd with a real Rust Peer client."""
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(HERE.parent / "worker_sql"))
from check import Runtime, require, runtime_default  # noqa: E402


class AssetsRuntime(Runtime):
    def start(self):
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        self.url = f"http://127.0.0.1:{listener.getsockname()[1]}"
        command = [
            str(self.executable),
            "serve",
            str(self.config),
            f"--socket-fd=probe={listener.fileno()}",
            f"--directory-path=storage={self.data}",
            "--experimental",
        ]
        try:
            self.process = subprocess.Popen(
                command, cwd=HERE, pass_fds=(listener.fileno(),),
                stdout=self.log, stderr=subprocess.STDOUT,
            )
        finally:
            listener.close()
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f"workerd exited early: {self.process.returncode}")
            try:
                return self.get("/state")
            except (OSError, urllib.error.URLError):
                time.sleep(0.05)
        raise TimeoutError("workerd readiness timed out")


def client(url: Path, verify=False):
    command = [
        "cargo",
        "run",
        "--quiet",
        "--offline",
        "-p",
        "harmion-worker",
        "--example",
        "probe_client",
        "--",
        str(url),
    ]
    if verify:
        command.append("--verify")
    environment = dict(os.environ, RUSTC_WRAPPER="")
    result = subprocess.run(
        command, cwd=REPO, text=True, capture_output=True, env=environment
    )
    if result.returncode != 0:
        print(result.stdout, flush=True)
        print(result.stderr, flush=True)
        raise SystemExit("probe client failed")
    for line in result.stdout.splitlines():
        print(line, flush=True)


def unauthorized_admin(url: str):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    request = urllib.request.Request(url + "/admin/overview", data=b"{}", method="POST")
    try:
        opener.open(request, timeout=2)
    except urllib.error.HTTPError as error:
        require(error.code == 401, f"expected 401, received {error.code}")
        error.close()
        return
    raise AssertionError("admin overview accepted a request without a bearer")


def unauthorized_internal(url: str):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    request = urllib.request.Request(
        url + "/internal/sync", data=b"00" * 32, method="POST"
    )
    try:
        opener.open(request, timeout=2)
    except urllib.error.HTTPError as error:
        require(error.code == 401, f"expected 401, received {error.code}")
        error.close()
        return
    raise AssertionError("outbound sync accepted a request without a bearer")


def static_assets(url: str):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(url + "/", timeout=5) as response:
        require(response.status == 200, "index status")
        html = response.read().decode()
    match = re.search(r'src="(/assets/[^"]+\.js)"', html)
    require(match, "index references a built JS asset")
    with opener.open(url + match.group(1), timeout=5) as response:
        require(response.status == 200, "asset status")
        require("javascript" in response.headers.get("content-type", ""), "asset type")
    print("PASS: Worker serves the built admin UI assets", flush=True)
    request = urllib.request.Request(url + "/capabilities", data=b"{}", method="POST")
    try:
        opener.open(request, timeout=2)
    except urllib.error.HTTPError as error:
        require(error.code == 401, f"expected 401, received {error.code}")
        error.close()
        print("PASS: capabilities requires the app-layer bearer", flush=True)
        return
    raise AssertionError("capabilities was served without authentication")


def unauthorized(url: str):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    request = urllib.request.Request(
        url + "/provision",
        data=b"{}",
        method="POST",
        headers={"x-harmion-bootstrap": "wrong"},
    )
    try:
        opener.open(request, timeout=2)
    except urllib.error.HTTPError as error:
        require(error.code == 401, f"expected 401, received {error.code}")
        error.close()
        return
    raise AssertionError("provision accepted a wrong bootstrap token")


def main():
    executable = runtime_default().resolve()
    require(executable.is_file(), "run the locked npm ci in design_checks/worker_sql first")
    ui = HERE / "ui"
    if ui.exists():
        shutil.rmtree(ui)
    source = REPO / "apps/admin/ui/dist"
    require(source.is_dir(), "build the admin UI first: npm run build in apps/admin/ui")
    shutil.copytree(source, ui)
    for artifact in ("harmion_worker_do.js", "harmion_worker_do_bg.wasm"):
        require(
            (HERE / "pkg" / artifact).is_file(),
            "build the worker wasm and run wasm-bindgen first (see README)",
        )
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix="harmion-do-") as temporary:
        directory = Path(temporary)
        data = directory / "data"
        data.mkdir(mode=0o700)
        with (directory / "workerd.log").open("w+") as log:
            runtime = AssetsRuntime(executable, data, log, HERE / "config.capnp")
            try:
                runtime.start()
                unauthorized(runtime.url)
                print("PASS: provision rejects a wrong bootstrap token", flush=True)
                unauthorized_admin(runtime.url)
                print("PASS: admin requires the app-layer bearer", flush=True)
                unauthorized_internal(runtime.url)
                print("PASS: outbound sync requires the app-layer bearer", flush=True)
                static_assets(runtime.url)
                client(runtime.url)
                runtime.stop()
                runtime.start()
                print("PASS: workerd restarted with the same durable storage", flush=True)
                client(runtime.url, verify=True)
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
    print("ALL DO PROBE CHECKS PASSED", flush=True)


if __name__ == "__main__":
    main()
