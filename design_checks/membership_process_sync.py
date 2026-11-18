#!/usr/bin/env python3
"""Signed membership propagation through actual daemon APIs (no DB edits)."""
from pathlib import Path
import sys
import tempfile

from two_process_sync import Node, transport_fixture


def statuses(node):
    return {item["node_id"]: item["state"] for item in node.cli("members")["members"]}


def dismiss_target(node, target):
    for pending in node.cli("members")["pending_retirements"]:
        if pending["target"] == target.identity["node_id"]:
            node.cli("dismiss-retirement", pending["certificate_id"])


def main():
    nodes = []
    with tempfile.TemporaryDirectory(prefix="harmion-membership-") as tmp:
        try:
            for index in range(4):
                node = Node(Path(tmp) / str(index), nodes[0].identity["cluster_id"] if nodes else None)
                nodes.append(node)
                node.start()
            a, b, c, d = nodes
            # New nodes bootstrap one known peer; no B-C or A-D approval.
            edges = [(a, b), (a, c), (c, d)]
            for left, right in edges:
                left.cli("approve", right.path / "peer.json")
                right.cli("join", left.path / "peer.json")
                left.cli("sync", right.identity["node_id"])
            for _ in range(4):
                for left, right in edges:
                    left.cli("sync", right.identity["node_id"])
            for node in nodes:
                assert statuses(node) == {n.identity["node_id"]: "active" for n in nodes}
            created = d.write("create", body="admitted through C")["content_id"]
            d.cli("sync", b.identity["node_id"])
            assert b.cli("get", created)["content"]["body"] == "admitted through C"
            # A's retirement of C reaches B. C's confirmed approval of D survives.
            a.cli("retire", c.identity["node_id"])
            a.cli("sync", b.identity["node_id"])
            b.cli("sync", d.identity["node_id"])
            for node in [a, b, d]:
                assert statuses(node)[c.identity["node_id"]] == "retired"
                assert statuses(node)[d.identity["node_id"]] == "active"
            c.cli("sync", b.identity["node_id"], ok=False)
            assert statuses(c)[c.identity["node_id"]] == "retired"
            b.cli("approve", c.path / "peer.json", ok=False)
            c.cli("approve", d.path / "peer.json", ok=False)
            # Retirements issued before exchange are a management conflict.
            a.cli("retire", b.identity["node_id"])
            b.cli("retire", a.identity["node_id"])
            a.cli("sync", b.identity["node_id"], ok=False)
            b.cli("sync", a.identity["node_id"], ok=False)
            d.cli("sync", a.identity["node_id"], ok=False)
            for node in [a, b, d]:
                current = statuses(node)
                assert current[a.identity["node_id"]] == "conflict", current
                assert current[b.identity["node_id"]] == "conflict", current
                assert current[d.identity["node_id"]] == "active", current
            a.cli("retire", b.identity["node_id"], ok=False)
            # D is independently active; its owner confirms B's retirement.
            d.cli("retire", b.identity["node_id"])
            dismiss_target(d, a)
            d.cli("sync", a.identity["node_id"])
            d.cli("sync", b.identity["node_id"], ok=False)
            for node in [a, b, d]:
                current = statuses(node)
                assert current[a.identity["node_id"]] == "active", current
                assert current[b.identity["node_id"]] == "retired", current
            a.stop(crash=True)
            a.start()
            assert statuses(a)[b.identity["node_id"]] == "retired"
            a.cli("approve", b.path / "peer.json", ok=False)
            a.cli("sync", d.identity["node_id"])
            assert a.cli("get", created)["content"]["body"] == "admitted through C"
            print("PASS: four daemons, signed admission/retirement forwarding, retained child, self-retirement, mutual conflict, third-node confirmation, restart")
        finally:
            for node in nodes:
                node.stop()
                node.log.close()


def owner_recovery():
    nodes = []
    with tempfile.TemporaryDirectory(prefix="harmion-owner-recovery-") as tmp:
        try:
            for index in range(3):
                node = Node(Path(tmp) / str(index), nodes[0].identity["cluster_id"] if nodes else None)
                nodes.append(node)
                node.start()
            a, b, recovery = nodes
            a.cli("approve", b.path / "peer.json")
            b.cli("join", a.path / "peer.json")
            a.cli("sync", b.identity["node_id"])
            a.cli("trust-recovery-peer", recovery.path / "peer.json", ok=False)
            a.cli("retire", b.identity["node_id"])
            b.cli("retire", a.identity["node_id"])
            a.cli("sync", b.identity["node_id"], ok=False)
            b.cli("sync", a.identity["node_id"], ok=False)
            recovery.cli("join-recovery", a.path / "peer.json")
            a.cli("trust-recovery-peer", recovery.path / "peer.json")
            a.cli("sync", b.identity["node_id"], ok=False)
            # A local owner's recovery trust is never silently accepted by B.
            assert statuses(b).get(recovery.identity["node_id"]) != "active"
            b.cli("trust-recovery-peer", recovery.path / "peer.json")
            for node in [a, b]:
                recovery.cli("approve", node.path / "peer.json")
            recovery.cli("sync", a.identity["node_id"], ok=False)
            recovery.cli("sync", b.identity["node_id"], ok=False)
            recovery.cli("retire", b.identity["node_id"])
            dismiss_target(recovery, a)
            recovery.cli("sync", a.identity["node_id"])
            recovery.cli("sync", b.identity["node_id"], ok=False)
            a.stop(crash=True)
            a.start()
            assert statuses(a)[a.identity["node_id"]] == "active"
            assert statuses(a)[b.identity["node_id"]] == "retired"
            print("PASS: two-node mutual retirement, explicit local owner recovery trust, no remote trust escalation, restart")
        finally:
            for node in nodes:
                node.stop()
                node.log.close()


def unseen_retirement():
    nodes = []
    with tempfile.TemporaryDirectory(prefix="harmion-unseen-retirement-") as tmp:
        try:
            for index in range(3):
                node = Node(Path(tmp) / str(index), nodes[0].identity["cluster_id"] if nodes else None)
                nodes.append(node)
                node.start()
            a, b, c = nodes
            for node in [b, c]:
                a.cli("approve", node.path / "peer.json")
                node.cli("join", a.path / "peer.json")
                a.cli("sync", node.identity["node_id"])
            for _ in range(2):
                a.cli("sync", b.identity["node_id"])
                a.cli("sync", c.identity["node_id"])
            c.cli("retire", b.identity["node_id"])
            a.cli("retire", c.identity["node_id"])
            c.cli("sync", a.identity["node_id"], ok=False)
            a.cli("sync", b.identity["node_id"], ok=False)
            assert statuses(a)[b.identity["node_id"]] == "conflict"
            assert statuses(b)[b.identity["node_id"]] == "conflict"
            pending = a.cli("members")["pending_retirements"]
            assert len(pending) == 1
            certificate = pending[0]["certificate_id"]
            b.cli("dismiss-retirement", certificate, ok=False)
            a.cli("confirm-retirement", certificate)
            assert not a.cli("members")["pending_retirements"]
            a.cli("sync", b.identity["node_id"], ok=False)
            assert statuses(b)[b.identity["node_id"]] == "retired"
            a.cli("dismiss-retirement", certificate, ok=False)
            print("PASS: unseen retirement after issuer retirement holds target; explicit confirmation replaces pending proof atomically")
        finally:
            for node in nodes:
                node.stop()
                node.log.close()


if __name__ == "__main__":
    with transport_fixture("--iroh" in sys.argv):
        main()
        owner_recovery()
        unseen_retirement()
