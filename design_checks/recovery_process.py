#!/usr/bin/env python3
"""Recovery sync through real daemons; SIGSTOP is a runtime pause, not kernel suspend."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

from two_process_sync import CLI, Node, transport_fixture


def status(node, peer):
    return next(item for item in node.cli("sync-status")["peers"]
                if item["node"] == peer.identity["node_id"])


def await_condition(check, description, seconds=20):
    deadline = time.monotonic() + seconds
    while True:
        if check():
            return
        assert time.monotonic() < deadline, description
        time.sleep(0.1)


def content_body(node, content):
    result = subprocess.run(
        [str(CLI), "--node", str(node.path), "get", content],
        capture_output=True, text=True, timeout=10,
    )
    return json.loads(result.stdout)["content"]["body"] if result.returncode == 0 else None


def pause(node):
    os.kill(node.process.pid, signal.SIGSTOP)
    try:
        time.sleep(6)
    finally:
        os.kill(node.process.pid, signal.SIGCONT)


def main():
    nodes = []
    with tempfile.TemporaryDirectory(prefix="harmion-recovery-process-") as tmp:
        try:
            a = Node(Path(tmp) / "a")
            nodes.append(a)
            b = Node(Path(tmp) / "b", a.identity["cluster_id"])
            nodes.append(b)
            a.start()
            b.start()
            a.cli("approve", b.path / "peer.json")
            b.cli("join", a.path / "peer.json")
            b.cli("sync", a.identity["node_id"])
            b.stop()
            a.stop()
            # Startup fails, leaving B's scheduler in a 240s-or-longer retry delay.
            # A never initiates automatic sync, so only B's recovery can pull data.
            b.start(120)
            await_condition(lambda: status(b, a)["last_failure"] is not None,
                            "startup failed sync was not observed", seconds=45)
            a.start()
            content = a.write("create", body="after runtime recovery")["content_id"]
            pause(b)
            await_condition(lambda: content_body(b, content) == "after runtime recovery",
                            "resume hint did not bypass long failed retry delay", seconds=45)
            await_condition(lambda: status(b, a)["last_failure"] is None,
                            "recovery did not finish a successful round")
            # A manual-only daemon must not start background synchronization merely
            # because its process was paused, even though B has an unseen update.
            b.stop()
            b.start()
            manual = b.write("create", body="manual mode stays manual")["content_id"]
            pause(a)
            time.sleep(3)
            assert content_body(a, manual) is None, "sync-seconds 0 started background sync"
            a.cli("sync", b.identity["node_id"])
            assert content_body(a, manual) == "manual mode stays manual"
            print("PASS: runtime pause recovery bypasses failed backoff; manual mode stays manual")
        finally:
            for node in nodes:
                if node.process is not None:
                    # Ensure cleanup also works if an assertion interrupted a paused process.
                    os.kill(node.process.pid, signal.SIGCONT)
                node.stop()
                node.log.close()


if __name__ == "__main__":
    if not hasattr(signal, "SIGSTOP"):
        raise SystemExit("This runtime-pause check requires Unix SIGSTOP/SIGCONT")
    with transport_fixture("--iroh" in sys.argv):
        main()
