#!/usr/bin/env python3
"""E2E: real harmiond daemons <-> local workerd DO through TLS.

Local-only. It provisions a Workerd-hosted replica with a manifest exported from
a real daemon, pushes a record through the Worker, and pulls it back through a
second daemon. This is the product-binary counterpart to the native probe.
"""
import importlib.util
import json
import os
from pathlib import Path
import selectors
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
sys.path.insert(0, str(HERE))
from two_process_sync import Node, DAEMON, CLI  # noqa: E402

WORKER_DO = HERE / "worker_do"
CONFIG = WORKER_DO / "daemon-config.capnp"
FORWARD = REPO / "target/debug/examples/tls_forward"
ADMIN = "0123456789abcdef" * 4
BOOTSTRAP = "fedcba9876543210" * 4


def _module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_check = _module("worker_do_check", WORKER_DO / "check.py")
AssetsRuntime = _check.AssetsRuntime
_worker_sql = _module("worker_sql_check", HERE / "worker_sql" / "check.py")
require = _worker_sql.require
runtime_default = _worker_sql.runtime_default


def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def write_config(origin, path, cluster):
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
        (name = "CLUSTER", text = "{cluster}"),
        (name = "NODE_ID", text = "0303030303030303030303030303030303030303030303030303030303030303"),
        (name = "EPOCH", text = "0303030303030303030303030303030303030303030303030303030303030303"),
        (name = "SEED", text = "0303030303030303030303030303030303030303030303030303030303030303"),
        (name = "ORIGIN", text = "{origin}"),
        (name = "ADMIN_TOKEN", text = "{ADMIN}"),
        (name = "BOOTSTRAP_TOKEN", text = "{BOOTSTRAP}"),
        (name = "ALLOW_PROVISION", text = "1"),
        (name = "SYNC_INTERVAL_MS", text = "0")
      ],
      durableObjectNamespaces = [
        (className = "Node", uniqueKey = "harmion-do-daemon-e2e-v1", enableSql = true)
      ],
      durableObjectStorage = (localDisk = "storage")
    )),
    (name = "assets", assets = (
      path = "../../deploy/cloudflare/public",
      config = "{{\\"run_worker_first\\":[\\"/state\\",\\"/ready\\",\\"/peer/*\\",\\"/admin/*\\",\\"/provision\\",\\"/capabilities\\",\\"/overview\\",\\"/members\\",\\"/peers\\",\\"/provision-status\\",\\"/registration-code\\",\\"/internal/*\\"],\\"not_found_handling\\":\\"404-page\\"}}",
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


def sync(node, peer):
    """Run a manual sync, tolerating the provisional per-requester burst.

    A restart sequence can outrun the signed-HTTPS challenge bucket (16 burst,
    2/s refill). The daemon's periodic loop retries the same way; this probe
    waits for the bucket instead of treating capacity as a transport defect.
    """
    for attempt in range(5):
        result = subprocess.run(
            [str(CLI), "--node", str(node.path), "sync", peer],
            capture_output=True,
            text=True,
            timeout=60,
        )
        if result.returncode == 0:
            return json.loads(result.stdout)
        # 429 is the explicit capacity signal; 502 covered the older mapping.
        if not any(code in result.stderr for code in ("Http(429)", "Http(502)")) or attempt == 4:
            raise AssertionError(result.stderr)
        time.sleep(6)
    raise AssertionError("sync retries exhausted")


def request(url, data, headers):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    req = urllib.request.Request(url, data=data, method="POST")
    for name, value in headers.items():
        req.add_header(name, value)
    try:
        with opener.open(req, timeout=60) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def get_request(url):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    req = urllib.request.Request(url, method="GET")
    try:
        with opener.open(req, timeout=10) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def require_artifacts():
    require(DAEMON.is_file(), "build harmiond first")
    require(CLI.is_file(), "build harmion first")
    require(FORWARD.is_file(), "build the tls_forward example first")
    for path in (
        REPO / "deploy/cloudflare/src/pkg/harmion_worker_do.js",
        REPO / "deploy/cloudflare/src/pkg/harmion_worker_do_bg.wasm",
        REPO / "deploy/cloudflare/src/pkg/host-api.js",
        REPO / "deploy/cloudflare/public/index.html",
    ):
        require(path.is_file(), f"run deploy/cloudflare/build.sh first: {path}")


def main():
    require_artifacts()
    executable = runtime_default().resolve()
    require(executable.is_file(), "run npm ci in design_checks/worker_sql first")
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix="harmion-daemon-worker-") as temporary:
        directory = Path(temporary)
        data = directory / "data"
        data.mkdir(mode=0o700)
        proxy_port = free_port()
        origin = f"https://localhost:{proxy_port}"
        a = Node(directory / "a")
        nodes = [a]
        cluster = a.identity["cluster_id"]
        write_config(origin, CONFIG, cluster)
        ca = directory / "ca.der"
        forwarder = None
        with (directory / "workerd.log").open("w+") as workerd_log, (
            directory / "forward.log"
        ).open("w+") as forward_log:
            runtime = AssetsRuntime(executable, data, workerd_log, CONFIG)
            try:
                runtime.start()
                forwarder = subprocess.Popen(
                    [str(FORWARD), runtime.url, str(proxy_port), str(ca)],
                    stdout=subprocess.PIPE,
                    stderr=forward_log,
                    text=True,
                )
                with selectors.DefaultSelector() as selector:
                    selector.register(forwarder.stdout, selectors.EVENT_READ)
                    assert selector.select(15), "TLS forwarder readiness timed out"
                    ready = json.loads(forwarder.stdout.readline())
                assert ready["origin"] == origin, ready
                require(ca.is_file(), "TLS forwarder did not write the CA")

                status, body = request(
                    runtime.url + "/registration-code",
                    b"{}",
                    {"authorization": f"Bearer {ADMIN}"},
                )
                require(status == 200, f"registration-code failed: {status}")
                registration = json.loads(body)["registration"]
                registration_path = directory / "worker-registration.json"
                registration_path.write_text(json.dumps(registration))
                worker_node = registration["node_id"]
                require(
                    registration["cluster_id"] == cluster,
                    "the Worker did not use the founder cluster",
                )

                a.start(extra_options=("--signed-https-root", str(ca)))

                a.cli("approve", registration_path)
                manifest_path = directory / "manifest.json"
                written = a.cli(
                    "provision-manifest", registration_path, "--out", manifest_path
                )
                require(written.get("written") == str(manifest_path), written)
                manifest = json.loads(manifest_path.read_text())
                require(manifest["own"]["node"] == worker_node, "manifest own")

                status, body = get_request(runtime.url + "/ready")
                require(status == 503, f"unprovisioned ready was not 503: {status}")
                status, body = request(
                    runtime.url + "/provision",
                    manifest_path.read_bytes(),
                    {"x-harmion-bootstrap": BOOTSTRAP},
                )
                require(status == 200, f"provision failed: {status} {body!r}")
                status, body = get_request(runtime.url + "/ready")
                require(status == 200, f"provisioned ready was not 200: {status} {body!r}")

                first = a.write("create", body="first record from daemon A")
                first_content = first["content_id"]
                report = sync(a, worker_node)
                require(int(report["uploaded"]) >= 1, report)
                # The first round captured the Worker's high before uploading.
                report = sync(a, worker_node)
                require(int(report["remote_high"]) >= 1, report)
                print("PASS: real daemon provisioned workerd and pushed a record", flush=True)

                b = Node(directory / "b", cluster)
                nodes.append(b)
                b.start(extra_options=("--signed-https-root", str(ca)))
                a.cli("approve", b.path / "peer.json")
                b.cli("join", a.path / "peer.json")
                sync(b, a.identity["node_id"])
                view = b.cli("get", first_content)
                require(view["content"]["body"] == "first record from daemon A", view)
                print("PASS: second daemon joined the cluster and pulled from A", flush=True)

                # B has not synced A since this record was created; only the
                # Worker can supply it.
                second = a.write("create", body="second record through workerd")
                second_content = second["content_id"]
                report = sync(a, worker_node)
                require(int(report["uploaded"]) >= 1, report)
                sync(a, b.identity["node_id"])
                b.cli("approve", registration_path)
                report = sync(b, worker_node)
                view = b.cli("get", second_content)
                require(view["content"]["body"] == "second record through workerd", view)
                print("PASS: second daemon pulled a record from the Worker", flush=True)

                b.stop()
                a.stop()
                a.start(extra_options=("--signed-https-root", str(ca)))
                b.start(extra_options=("--signed-https-root", str(ca)))
                third = a.write("create", body="third record after restart")
                third_content = third["content_id"]
                sync(a, worker_node)
                sync(b, worker_node)
                view = b.cli("get", third_content)
                require(view["content"]["body"] == "third record after restart", view)
                print("PASS: daemons restarted and the Worker kept serving", flush=True)

                status, body = request(
                    runtime.url + "/provision",
                    manifest_path.read_bytes(),
                    {"x-harmion-bootstrap": BOOTSTRAP},
                )
                require(status == 409, f"second provision was not refused: {status}")
                print("PASS: the Worker refused a second provision", flush=True)
            except BaseException:
                for node in nodes:
                    try:
                        node.log.flush()
                        node.log.seek(0)
                        print(f"daemon {node.path} log:\n" + node.log.read(), flush=True)
                    except Exception:
                        pass
                if forwarder is not None:
                    print(f"forwarder status: {forwarder.poll()}", flush=True)
                workerd_log.flush()
                workerd_log.seek(0)
                print("workerd log:\n" + workerd_log.read(), flush=True)
                forward_log.flush()
                forward_log.seek(0)
                print("forwarder log:\n" + forward_log.read(), flush=True)
                raise
            finally:
                for node in nodes:
                    try:
                        node.stop(crash=True)
                    except Exception:
                        pass
                if forwarder is not None:
                    forwarder.terminate()
                    try:
                        forwarder.wait(10)
                    except subprocess.TimeoutExpired:
                        forwarder.kill()
                try:
                    runtime.stop()
                except Exception:
                    pass
                CONFIG.unlink(missing_ok=True)
    print("ALL DAEMON <-> WORKER CHECKS PASSED", flush=True)


if __name__ == "__main__":
    main()
