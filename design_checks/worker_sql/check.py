#!/usr/bin/env python3
"""Supervise a loopback-only workerd SQL probe in fresh temporary storage."""
import argparse
import json
import os
from pathlib import Path
import platform
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def runtime_default():
    suffix = {"x86_64": "64", "aarch64": "arm64"}.get(platform.machine())
    if platform.system() != "Linux" or suffix is None:
        raise RuntimeError("pass --workerd with a native executable on this platform")
    return ROOT / "node_modules" / "@cloudflare" / f"workerd-linux-{suffix}" / "bin" / "workerd"


class Runtime:
    def __init__(self, executable, data, log, config=None):
        self.executable, self.data, self.log = executable, data, log
        self.config = config or ROOT / "config.capnp"
        self.process = None
        self.url = None
        # Disable ambient HTTP proxy variables for all local test requests.
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def start(self):
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        self.url = f"http://127.0.0.1:{listener.getsockname()[1]}"
        command = [str(self.executable), "serve", str(self.config),
                   f"--socket-fd=probe={listener.fileno()}",
                   f"--directory-path=storage={self.data}"]
        try:
            self.process = subprocess.Popen(
                command, cwd=ROOT, pass_fds=(listener.fileno(),),
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
                time.sleep(0.05)  # bounded local readiness retry, not a harness-job poll
        raise TimeoutError("workerd readiness timed out")

    def get(self, route):
        with self.opener.open(self.url + route, timeout=2) as response:
            return json.load(response)

    def forbidden(self, route="/state", headers=None, method="GET"):
        request = urllib.request.Request(self.url + route, headers=headers or {}, method=method)
        try:
            with self.opener.open(request, timeout=2):
                raise AssertionError("unexpected acceptance of forbidden test request")
        except urllib.error.HTTPError as error:
            require(error.code == 403, f"expected 403, received {error.code}")
            error.close()

    def stop(self, crash=False):
        if self.process is None:
            return
        process, self.process = self.process, None
        if process.poll() is None:
            process.kill() if crash else process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
            raise RuntimeError("workerd did not stop within 10 seconds")
        # Signals are intentional; any other nonzero exit requires diagnosis.
        allowed = {0, -9} if crash else {0, -15}
        require(process.returncode in allowed, f"unexpected workerd exit {process.returncode}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workerd", type=Path, help="native workerd binary, not Node wrapper")
    args = parser.parse_args()
    executable = (args.workerd or runtime_default()).resolve()
    require(executable.is_file(), f"missing runtime: {executable}; run npm ci in {ROOT}")
    os.umask(0o077)
    version = subprocess.run([str(executable), "--version"], text=True, capture_output=True, check=True)
    print(version.stdout.strip(), flush=True)
    with tempfile.TemporaryDirectory(prefix="harmion-worker-sql-") as temporary:
        directory = Path(temporary)
        data = directory / "data"
        data.mkdir(mode=0o700)
        with (directory / "workerd.log").open("w+") as log:
            runtime = Runtime(executable, data, log)
            try:
                initial = runtime.start()
                require(initial["activations"] == "1", "unexpected initial activation count")
                runtime.forbidden("/unknown")
                runtime.forbidden("/state?query=1")
                runtime.forbidden(headers={"Origin": "https://example.invalid"})
                runtime.forbidden(headers={"Host": "example.invalid"})
                runtime.forbidden(method="POST")
                print("PASS: unknown/query/Origin/Host/method rejected (local test guard, not authentication)", flush=True)
                result = runtime.get("/run")
                for passed in result["passed"]:
                    print(f"PASS: {passed}", flush=True)
                baseline = result["state"]
                require(initial["node_id"] == baseline["node_id"], "identity changed in one process")
                print(f"Observed unsafe Number: {result['unsafe_number_observation']} (not exact integer)", flush=True)
                # Test both graceful process stop and abrupt kill AFTER sync/response.
                for expected, crash in [("2", False), ("3", True)]:
                    runtime.stop(crash=crash)
                    reopened = runtime.start()
                    require(reopened["activations"] == expected, "activation count did not persist")
                    for field in ("node_id", "cursor", "records", "integers"):
                        require(reopened[field] == baseline[field], f"restart changed {field}")
                    print(f"PASS: {'kill' if crash else 'TERM'} restart preserves identity/page/cursor/64bit rows", flush=True)
                require(baseline["cursor"] == "2" and baseline["records"] == 1, "unexpected final page state")
                print("PASS: local workerd SQL probe complete; no production Rust/auth/cloud claims", flush=True)
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
