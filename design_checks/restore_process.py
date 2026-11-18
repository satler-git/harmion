#!/usr/bin/env python3
"""Restore an offline backup to a fresh node and resynchronize its full history."""
from pathlib import Path
import shutil
import tempfile

from two_process_sync import DAEMON, Node, command


def main():
    nodes = []
    with tempfile.TemporaryDirectory(prefix="harmion-restore-") as temp:
        root = Path(temp)
        try:
            original = Node(root / "original")
            nodes.append(original)
            peer = Node(root / "peer", original.identity["cluster_id"])
            nodes.append(peer)
            original.start()
            peer.start()
            original.cli("approve", peer.path / "peer.json")
            peer.cli("join", original.path / "peer.json")
            original.cli("sync", peer.identity["node_id"])
            command(DAEMON, "restore", original.path, root / "live-copy", ok=False)
            first = original.write("create", body="backup only")
            original.stop()
            backup = root / "backup"
            shutil.copytree(original.path, backup)
            before = (backup / "node.db").read_bytes()
            command(DAEMON, "restore", backup, peer.path, ok=False)
            second = peer.write("create", body="peer only")
            peer.cli("get", first["content_id"], ok=False)

            restored = Node(root / "restored", backup=backup)
            nodes.append(restored)
            assert restored.identity["node_id"] != original.identity["node_id"]
            assert restored.identity["cluster_id"] == original.identity["cluster_id"]
            assert (backup / "node.db").read_bytes() == before
            for name in ("iroh.secret", "admin.secret", "certificate.der"):
                assert (restored.path / name).read_bytes() != (backup / name).read_bytes()
            restored.start()
            restored.cli("get", second["content_id"], ok=False)
            assert restored.cli("get", first["content_id"])["content"]["body"] == "backup only"
            restored.cli("save", backup / "request-1.bin", ok=False)
            peer.cli("approve", restored.path / "peer.json")
            restored.cli("join", peer.path / "peer.json")
            restored.cli("sync", peer.identity["node_id"])
            assert peer.cli("get", first["content_id"])["content"]["body"] == "backup only"
            assert restored.cli("get", second["content_id"])["content"]["body"] == "peer only"
            third = restored.write("create", body="after restore")
            restored.cli("sync", peer.identity["node_id"])
            assert peer.cli("get", third["content_id"])["content"]["body"] == "after restore"
            print("PASS: offline backup, new node and credentials, bidirectional catch-up and new save")
        finally:
            for node in reversed(nodes):
                node.stop()
                node.log.close()


if __name__ == "__main__":
    main()
