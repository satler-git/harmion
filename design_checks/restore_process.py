#!/usr/bin/env python3
"""Restore an offline backup to a fresh node and resynchronize its full history."""
from pathlib import Path
import json
import os
import pty
import shutil
import subprocess
import tempfile

from two_process_sync import CLI, DAEMON, Node, command


def cli_with_owner_approval(node, *args, env=None):
    """Run an R2 CLI command with a PTY on stdin and answer the owner prompt."""
    master, slave = pty.openpty()
    process = subprocess.Popen(
        [str(CLI), "--node", str(node.path), *[str(arg) for arg in args]],
        stdin=slave,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )
    os.close(slave)
    try:
        os.write(master, b"y\n")
        stdout, stderr = process.communicate(timeout=60)
    finally:
        os.close(master)
    assert process.returncode == 0, stderr
    return json.loads(stdout)



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
            # Owner credentials do not cross the restore boundary: R2 must be
            # fail-closed until the owner re-enrolls a key on the stopped node.
            assert restored.cli("owner-status")["enforce"] is True
            restored.cli("join", peer.path / "peer.json", ok=False)
            restored.stop()
            owner_key = root / "restore-owner-key"
            subprocess.run(
                ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "restore-owner",
                 "-f", str(owner_key)],
                check=True,
            )
            command(DAEMON, "owner-enroll", restored.path,
                    "--ssh-key", owner_key.with_name("restore-owner-key.pub"))
            restored.start()
            status = restored.cli("owner-status")
            assert status["enforce"] is True and status["audit_valid"] is True
            owner_env = {
                **os.environ,
                "HARMION_OWNER_SSH_KEY": str(owner_key.with_name("restore-owner-key.pub")),
            }
            peer.cli("approve", restored.path / "peer.json")
            cli_with_owner_approval(
                restored, "join", peer.path / "peer.json", env=owner_env
            )
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
