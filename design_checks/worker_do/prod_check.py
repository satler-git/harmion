#!/usr/bin/env python3
"""Smoke-test the deployable Worker module and its read-only auth in workerd."""
import http.client
import os
from pathlib import Path
import sys
import tempfile
import urllib.error
import urllib.request
from urllib.parse import urlsplit

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(HERE.parent / "worker_sql"))
from check import require, runtime_default  # noqa: E402
import importlib.util  # noqa: E402
_spec = importlib.util.spec_from_file_location("worker_do_check", HERE / "check.py")
_module = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_module)
AssetsRuntime = _module.AssetsRuntime


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
    os.umask(0o077)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with tempfile.TemporaryDirectory(prefix="harmion-prod-") as temporary:
        directory = Path(temporary)
        data = directory / "data"
        data.mkdir(mode=0o700)
        with (directory / "workerd.log").open("w+") as log:
            runtime = AssetsRuntime(executable, data, log, HERE / "prod-config.capnp")
            try:
                runtime.start()
                with opener.open(runtime.url + "/", timeout=5) as response:
                    require(response.status == 200, "UI index")
                with opener.open(runtime.url + "/state", timeout=5) as response:
                    require(response.status == 200, "state liveness")
                try:
                    opener.open(runtime.url + "/ready", timeout=2)
                except urllib.error.HTTPError as error:
                    require(error.code == 503, f"unprovisioned ready expected 503, got {error.code}")
                    error.close()
                else:
                    raise AssertionError("unprovisioned Worker reported ready")
                request = urllib.request.Request(
                    runtime.url + "/admin/overview", data=b"{}", method="POST"
                )
                try:
                    opener.open(request, timeout=2)
                except urllib.error.HTTPError as error:
                    require(error.code == 401, f"admin expected 401, got {error.code}")
                    error.close()
                else:
                    raise AssertionError("admin accepted a request without a bearer")
                request = urllib.request.Request(
                    runtime.url + "/provision",
                    data=b"{}",
                    method="POST",
                    headers={"x-harmion-bootstrap": "wrong"},
                )
                try:
                    opener.open(request, timeout=2)
                except urllib.error.HTTPError as error:
                    require(error.code == 401, f"provision expected 401, got {error.code}")
                    error.close()
                else:
                    raise AssertionError("provision accepted a wrong bootstrap token")
                request = urllib.request.Request(
                    runtime.url + "/provision",
                    data=b"{}",
                    method="POST",
                    headers={"x-harmion-bootstrap": "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"},
                )
                try:
                    opener.open(request, timeout=2)
                except urllib.error.HTTPError as error:
                    require(
                        error.code == 403,
                        f"provision without ALLOW_PROVISION expected 403, got {error.code}",
                    )
                    error.close()
                else:
                    raise AssertionError("provision ignored ALLOW_PROVISION")
                # The edge must reject a declared oversized body before the
                # Worker buffers it. No body is sent; the status alone proves
                # Content-Length is checked first.
                parsed = urlsplit(runtime.url)
                connection = http.client.HTTPConnection(
                    parsed.hostname, parsed.port, timeout=2
                )
                connection.putrequest("POST", "/peer/v2/challenge")
                connection.putheader("Content-Length", str(9 * 1024 * 1024))
                connection.endheaders()
                response = connection.getresponse()
                response.read()
                require(
                    response.status == 413,
                    f"oversized body expected 413, got {response.status}",
                )
                connection.close()
                print("PASS: deployable Worker loads, serves the UI and rejects unauthenticated calls", flush=True)
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
    print("ALL PROD SMOKE CHECKS PASSED", flush=True)


if __name__ == "__main__":
    main()
