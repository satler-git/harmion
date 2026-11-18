#!/usr/bin/env python3
"""Five real daemons: data replication only; membership propagation is separate."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from two_process_sync import CLI, Node, transport_fixture


def await_content(node, content, body):
    deadline = time.monotonic() + 20
    while True:
        result = subprocess.run(
            [str(CLI), "--node", str(node.path), "get", content],
            capture_output=True, text=True, timeout=10,
        )
        if result.returncode == 0:
            assert json.loads(result.stdout)["content"]["body"] == body
            return
        assert time.monotonic() < deadline, result.stderr
        time.sleep(0.1)


def main():
    nodes = []
    with tempfile.TemporaryDirectory(prefix="harmion-multi-process-") as tmp:
        try:
            for index in range(5):
                node = Node(Path(tmp) / str(index), nodes[0].identity["cluster_id"] if nodes else None)
                nodes.append(node)
                node.start()
            a, b, c, d, e = nodes
            # Explicit adjacent approvals isolate data forwarding from membership.
            chain = list(zip(nodes, nodes[1:]))
            for left, right in chain:
                left.cli("approve", right.path / "peer.json")
                right.cli("join", left.path / "peer.json")
                left.cli("sync", right.identity["node_id"])

            def rounds(edges, count=2):
                for _ in range(count):
                    for left, right in edges:
                        left.cli("sync", right.identity["node_id"])
                    for left, right in reversed(edges):
                        right.cli("sync", left.identity["node_id"])

            content = a.write("create", body="base")["content_id"]
            rounds(chain)
            tokens = [node.cli("get", content)["edit_token"] for node in nodes]
            # Keep the middle offline while opposite sides save independent edits.
            c.stop()
            a.write("edit", content, tokens[0], body="partition A")
            e.write("edit", content, tokens[4], body="partition E")
            rounds([chain[0], chain[-1]])
            c.start()
            rounds(chain)
            views = [node.cli("get", content)["content"] for node in nodes]
            assert all(view == views[0] for view in views)
            conflicts = b.cli("conflicts", content)
            assert len(conflicts["candidates"][0]["candidates"]) == 2
            versions = [v for candidate in conflicts["candidates"][0]["candidates"] for v in candidate["versions"]]
            late_token = e.cli("get", content)["edit_token"]
            b.write("resolve", content, conflicts["view"]["edit_token"], body="resolved", versions=versions)
            e.write("edit", content, late_token, body="late edit")
            # Add an independent path to exercise duplicate forwarding around a cycle.
            e.cli("approve", a.path / "peer.json")
            a.cli("approve", e.path / "peer.json")
            ring = chain + [(e, a)]
            rounds(ring, 3)
            for node in nodes:
                assert len(node.cli("conflicts", content)["candidates"][0]["candidates"]) == 2
            snapshots = [node.cli("get", content)["content"] for node in nodes]
            assert all(snapshot == snapshots[0] for snapshot in snapshots)
            c.stop(crash=True)
            c.start()
            assert c.cli("get", content)["content"] == snapshots[0]
            rounds(ring)
            for left, right in ring:
                quiet = left.cli("sync", right.identity["node_id"])
                assert quiet["uploaded"] == quiet["downloaded"] == "0", quiet
            # Offline C must not stop B from reaching A. Periodic rounds only.
            for node in nodes:
                node.stop()
            for node in [a, b, d, e]:
                node.start(1)
            automatic = b.write("create", body="with offline neighbor")["content_id"]
            for node in [a, e, d]:
                await_content(node, automatic, "with offline neighbor")
            c.start(1)
            await_content(c, automatic, "with offline neighbor")
            print("PASS: five daemons, chain/ring forwarding, partition edits, late conflict, crash/restart, finite echo, periodic sync with offline peer")
        finally:
            for node in nodes:
                node.stop()
                node.log.close()


if __name__ == "__main__":
    with transport_fixture("--iroh" in sys.argv):
        main()
