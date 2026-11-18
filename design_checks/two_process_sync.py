#!/usr/bin/env python3
"""Exercise built harmion/harmiond binaries through the public CLI, using only localhost."""
import json
from contextlib import contextmanager
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
DAEMON = ROOT / "target/debug/harmiond"
CLI = ROOT / "target/debug/harmion"
RELAY_URL = None


def command(*args, ok=True):
    result = subprocess.run([str(a) for a in args], capture_output=True, text=True, timeout=45)
    if not ok:
        assert result.returncode != 0, result.stdout
        return result
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


class Node:
    def __init__(self, path, cluster=None):
        self.path, self.process, self.counter = path, None, 0
        args = [DAEMON, "init", path]
        if cluster:
            args += ["--cluster", cluster]
        self.identity = command(*args)
        self.log = open(path / "test-stderr", "w+")
        os.chmod(path / "test-stderr", 0o600)

    def start(self, interval=0):
        assert self.process is None
        options = ["--peer-transport", "https"] if RELAY_URL is None else ["--peer-transport", "iroh", "--relay-url", RELAY_URL, "--discovery", "none"]
        self.process = subprocess.Popen(
            [str(DAEMON), "run", str(self.path), *options, "--sync-seconds", str(interval)],
            stdout=subprocess.PIPE, stderr=self.log, text=True,
        )
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdout, selectors.EVENT_READ)
            assert selector.select(10), "daemon readiness timed out"
            line = self.process.stdout.readline()
        assert line, "daemon exited before readiness"
        self.ready = json.loads(line)
        assert self.ready["ready"]
        assert self.ready["peer"]["node_id"] == self.identity["node_id"]

    def stop(self, crash=False):
        if self.process is not None:
            process, self.process = self.process, None
            if crash:
                process.kill()
            else:
                process.terminate()
            try:
                code = process.wait(10)
                if not crash:
                    assert code == 0, f"unclean daemon shutdown: {code}"
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(10)
                process.stdout.close()

    def cli(self, *args, ok=True):
        return command(CLI, "--node", self.path, *args, ok=ok)

    def write(self, operation, *prefix, body=None, versions=()):
        self.counter += 1
        request = self.path / f"request-{self.counter}.bin"
        args = [operation, *prefix]
        if body is not None:
            source = self.path / "body.txt"
            source.write_text(body)
            os.chmod(source, 0o600)
            args.append(source)
        args += [request, *versions]
        return self.cli(*args)


def main():
    nodes = []
    with tempfile.TemporaryDirectory(prefix="harmion-process-test-") as tmp:
        try:
            a = Node(Path(tmp) / "a")
            nodes.append(a)
            b = Node(Path(tmp) / "b", a.identity["cluster_id"])
            nodes.append(b)
            secret = a.path / "iroh.secret"
            backup = a.path / "iroh.secret.backup"
            secret.rename(backup)
            try:
                command(DAEMON, "run", a.path, ok=False)
                assert not secret.exists(), "missing key was silently replaced"
            finally:
                backup.rename(secret)
            a.start()
            b.start()
            # Reinitialization and second daemon must not replace live identity or credentials.
            command(DAEMON, "init", a.path, ok=False)
            command(DAEMON, "run", a.path, ok=False)
            a.cli("approve", b.path / "peer.json")
            b.cli("join", a.path / "peer.json")
            created = a.write("create", body="base")
            content = created["content_id"]

            def sync():
                a.cli("sync", b.identity["node_id"])
                b.cli("sync", a.identity["node_id"])

            sync()
            old_a = a.cli("get", content)["edit_token"]
            old_b = b.cli("get", content)["edit_token"]
            b.stop()
            a.write("edit", content, old_a, body="offline A")
            a.cli("sync", b.identity["node_id"], ok=False)
            b.start()
            b.write("edit", content, old_b, body="offline B")
            sync()
            conflicts = a.cli("conflicts", content)
            assert len(conflicts["candidates"][0]["candidates"]) == 2
            assert a.cli("get", content)["content"] == b.cli("get", content)["content"]
            versions = [v for c in conflicts["candidates"][0]["candidates"] for v in c["versions"]]
            unseen = b.cli("get", content)["edit_token"]
            a.write("resolve", content, conflicts["view"]["edit_token"], body="resolved", versions=versions)
            b.write("edit", content, unseen, body="late B")
            sync()
            conflicts = a.cli("conflicts", content)
            assert len(conflicts["candidates"][0]["candidates"]) == 2, "late update was lost"
            versions = [v for c in conflicts["candidates"][0]["candidates"] for v in c["versions"]]
            a.write("resolve", content, conflicts["view"]["edit_token"], body="final", versions=versions)
            sync()
            assert not b.cli("get", content)["conflicts"]
            edit_b = b.cli("get", content)["edit_token"]
            a.write("delete", content, a.cli("get", content)["edit_token"])
            b.write("edit", content, edit_b, body="edit during delete")
            sync()
            assert a.cli("inspect", content)["view"]["deletion_conflict"]
            assert b.cli("inspect", content)["view"]["deletion_conflict"]
            a.write("keep", content, a.cli("inspect", content)["view"]["edit_token"])
            sync()
            assert not b.cli("get", content)["deletion_conflict"]
            # Crash after committed operations; replay original create returns original receipt.
            certificate = a.ready["peer"]["binding"]
            a.stop(crash=True)
            a.start()
            assert certificate == a.ready["peer"]["binding"]
            assert a.cli("save", created["request_file"]) == created["result"]
            sync()
            quiet = a.cli("sync", b.identity["node_id"])
            assert quiet["uploaded"] == quiet["downloaded"] == "0", "echo did not terminate"
            # Verify automatic synchronization without explicit sync calls.
            a.stop()
            b.stop()
            a.start(1)
            b.start(1)
            automatic = a.write("create", body="automatic")["content_id"]
            deadline = time.monotonic() + 15
            while True:
                result = subprocess.run([str(CLI), "--node", str(b.path), "get", automatic], capture_output=True, text=True, timeout=10)
                if result.returncode == 0:
                    assert json.loads(result.stdout)["content"]["body"] == "automatic"
                    break
                assert time.monotonic() < deadline, result.stderr
                time.sleep(0.1)
            a.cli("retire", b.identity["node_id"])
            a.cli("approve", b.path / "peer.json", ok=False)
            b.cli("sync", a.identity["node_id"], ok=False)
            for node in nodes:
                for name in ["node.db", "admin.secret", "private-key.der", "connection.json", "iroh.secret"]:
                    assert (node.path / name).stat().st_mode & 0o077 == 0, name
            print("PASS: two daemon processes, CLI, partition, late resolution, deletion conflict, crash/replay, finite echo, automatic sync, retirement")
        finally:
            for node in nodes:
                node.stop()
                node.log.close()


@contextmanager
def transport_fixture(iroh=False):
    """Own the local relay process; usable by the multi-process checks too."""
    global RELAY_URL
    relay = None
    try:
        if iroh:
            relay = subprocess.Popen([str(ROOT / "target/debug/examples/local_relay")], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
            with selectors.DefaultSelector() as selector:
                selector.register(relay.stdout, selectors.EVENT_READ)
                assert selector.select(10), "relay readiness timed out"
                RELAY_URL = relay.stdout.readline().strip()
            assert RELAY_URL.startswith("http://127.0.0.1:")
        yield
    finally:
        if relay is not None:
            relay.stdin.close()
            try:
                relay.wait(10)
            finally:
                if relay.poll() is None:
                    relay.kill()
                    relay.wait(10)
                relay.stdout.close()
        RELAY_URL = None


if __name__ == "__main__":
    with transport_fixture("--iroh" in sys.argv):
        main()
