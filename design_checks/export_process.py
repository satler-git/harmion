#!/usr/bin/env python3
"""Verify the export directory produced by the real daemon and owner CLI."""
import json
from pathlib import Path
import tempfile

from two_process_sync import Node


def main():
    with tempfile.TemporaryDirectory(prefix="harmion-export-") as temp:
        root = Path(temp)
        node = Node(root / "node")
        try:
            node.start()
            content = node.write("create", body="superseded secret")["content_id"]
            base = node.cli("get", content)["edit_token"]
            node.write("edit", content, base, body="left\n")
            node.write("edit", content, base, body="right\r\n")
            normal = node.cli("get", content)["content"]["body"]
            deleted = node.write("create", body="deleted plaintext")["content_id"]
            node.write("delete", deleted, node.cli("get", deleted)["edit_token"])

            # Three escaped bodies exceed the regular 8 MiB response limit together.
            large = "\x01" * 500_000
            big_ids = [node.write("create", body=large)["content_id"] for _ in range(3)]
            destination = root / "export"
            result = node.cli("export", destination)["export"]
            manifest = json.loads((destination / "manifest.json").read_text())
            assert result == manifest
            assert manifest["format"] == "harmion-export" and manifest["version"] == 1
            records = [json.loads(line) for line in (destination / manifest["metadata"]).read_text().splitlines()]
            assert len(records) == manifest["records"]
            assert manifest["body_files"] == 4
            bodies = destination / "bodies"
            assert (bodies / f"{content}.txt").read_bytes() == normal.encode()
            for content_id in big_ids:
                assert (bodies / f"{content_id}.txt").read_bytes() == large.encode()
            assert not (bodies / f"{deleted}.txt").exists()
            assert any(record["type"] == "deletion" and record["content_id"] == deleted for record in records)
            candidates = [
                record for record in records
                if record["type"] == "candidate" and record["content_id"] == content
            ]
            assert {record["value"]["value"] for record in candidates} == {"left\n", "right\r\n"}
            assert len([record for record in candidates if record["selected"]]) == 1
            assert "superseded secret" not in json.dumps(records)
            assert "deleted plaintext" not in json.dumps(records)
            for path in [destination, *destination.rglob("*")]:
                assert path.stat().st_mode & 0o077 == 0, path
            node.cli("export", destination, ok=False)
            assert json.loads((destination / "manifest.json").read_text()) == manifest
            print("PASS: owner export, streamed bodies, conflicts, deletion, permissions, no overwrite")
        finally:
            node.stop()
            node.log.close()


if __name__ == "__main__":
    main()
