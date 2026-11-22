#!/usr/bin/env python3
"""Exercise the owner Web bridge and the conflict workflow against real processes."""
import http.client
import base64
import gzip
import json
import os
from pathlib import Path
import selectors
import re
import ssl
import subprocess
import tempfile
from urllib.parse import urlsplit, urlencode
from urllib.request import urlopen
from oauth_process import CHALLENGE

from two_process_sync import DAEMON, ROOT, Node


ADMIN = ROOT / "target/debug/harmion-admin"


def check(node, directory):
    subprocess.check_output([DAEMON, "oauth-provision", node.path], timeout=10)
    node.start(extra_options=("--oauth-listen", "127.0.0.1:0"))
    ui = directory / "ui"
    root = directory / "admin-ca.pem"
    root.write_bytes(subprocess.check_output([ADMIN, "provision", "--ui-dir", ui], timeout=15))
    assert (ui / "web-tls.json").stat().st_mode & 0o077 == 0
    daemon = subprocess.Popen(
        [ADMIN, "run", node.path, "--ui-dir", ui, "--approval", "stdio"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        assert daemon.stdin is not None
        # Each R2 operation consumes one approval line. The test drives the
        # out-of-band owner channel explicitly instead of relying on a TTY.
        daemon.stdin.write("y\n" * 64)
        daemon.stdin.flush()
        with selectors.DefaultSelector() as selector:
            selector.register(daemon.stdout, selectors.EVENT_READ)
            assert selector.select(10), "admin Web readiness timed out"
            link = urlsplit(json.loads(daemon.stdout.readline())["url"])
        origin = f"https://{link.netloc}"
        ctx = ssl.create_default_context(cafile=root)

        def call(path, data=None, session=None, status=200, *, from_origin=origin, host=link.netloc):
            connection = http.client.HTTPSConnection(link.hostname, link.port, context=ctx, timeout=8)
            try:
                headers = {"Host": host}
                if from_origin:
                    headers["Origin"] = from_origin
                if session:
                    headers["Authorization"] = f"Bearer {session}"
                body = None
                if data is not None:
                    headers["Content-Type"] = "application/json"
                    body = json.dumps(data).encode()
                connection.request("POST" if data is not None else "GET", path, body, headers)
                response = connection.getresponse()
                output = response.read()
                assert response.status == status, (path, response.status, output)
                assert response.getheader("cache-control") == "no-store"
                assert response.getheader("set-cookie") is None
                assert response.getheader("access-control-allow-origin") is None
                if response.getheader("content-type") == "application/json":
                    return json.loads(output)
                return output
            finally:
                connection.close()

        html = call("/").decode()
        assert 'id="app"' in html
        script = re.search(r'src="(/assets/[^"]+\.js)"', html)[1]
        assert call(script)
        call("/contents", {}, status=401)
        call("/export", {"directory":str(directory / "unapproved-export")}, status=401)
        assert not (directory / "unapproved-export").exists()
        call("/contents", {}, session="0" * 64, status=401)
        call("/session", link.fragment, status=403, from_origin="https://other.local")
        call("/session", link.fragment, status=403, from_origin=None)
        call("/session", link.fragment, status=400, host="other.local")
        session = call("/session", link.fragment)["session"]
        call("/session", link.fragment, status=401)
        call("/contents", {}, session, status=403, from_origin="https://other.local")
        call("/issue-app", {}, session, status=404)
        call("/inspect", None, session, status=404)
        snapshot = call("/overview", {}, session)
        assert snapshot["metrics"]["contents"] == "0"
        assert snapshot["identity"]["node_id"] == node.identity["node_id"]
        assert snapshot["peer_transfer"].startswith("harmion-peer:v1:")
        decoded = base64.urlsafe_b64decode(snapshot["peer_transfer"].split(":z:")[1] + "===")
        assert json.loads(gzip.decompress(decoded)) == snapshot["identity"]

        content = node.write("create", body="head\ncenter\ntail\n")["content_id"]
        fixed = node.cli("get", content)["edit_token"]
        a = node.write("edit", content, fixed, body="LEFT\ncenter\ntail\n")["result"]["update_id"]
        b = node.write("edit", content, fixed, body="head\ncenter\nRIGHT\n")["result"]["update_id"]
        assert call("/contents", {}, session)["items"][0]["conflicted"]
        inspected = call("/inspect", {"content":content}, session)
        token = inspected["token"]
        base = node.cli("merge-base", content, token, a, b)
        assert base["text"] == "head\ncenter\ntail\n"
        proposal = call("/merge", {"content":content, "token":token, "left":a, "right":b}, session)
        assert proposal["text"] == "LEFT\ncenter\nRIGHT\n" and not proposal["has_conflicts"]
        assert len(node.cli("conflicts", content)["candidates"][0]["candidates"]) == 2
        late = node.write("edit", content, fixed, body="late\n")["result"]["update_id"]
        call("/merge", {"content":content, "token":token, "left":a, "right":late}, session, status=400)
        edit = {"type":"body","text":proposal["text"],"versions":[a,b]}
        save = {"content":content, "token":token, "request_id":"1" * 64, "edit":edit}
        result = call("/resolve", save, session)
        assert result["saved"] and call("/resolve", save, session) == result
        versions = {v for group in call("/inspect", {"content":content}, session)["groups"]
                    for candidate in group["candidates"] for v in candidate["versions"]}
        assert versions == {late, result["update_id"]}
        new_view = call("/inspect", {"content":content}, session)
        conflict = new_view["groups"][0]
        picks = [{"field":conflict["field"], "version":result["update_id"], "replaces":list(versions)}]
        call("/resolve", {"content":content, "token":new_view["token"], "request_id":"2" * 64,
                          "edit":{"type":"select","fields":picks}}, session)
        merged_view = node.cli("get", content)
        assert merged_view["content"]["body"] == proposal["text"]
        node.write("delete", content, merged_view["edit_token"])
        deleted = call("/inspect", {"content":content}, session)
        assert deleted["deleted"]
        call("/resolve", {"content":content, "token":deleted["token"], "request_id":"3" * 64,
                          "edit":{"type":"restore","text":"restored","fields":[]}}, session)
        assert node.cli("get", content)["content"]["body"] == "restored"

        def size_prefix(value):
            return len(value).to_bytes(4, "big") + value

        typed = node.cli("create", node.path / "body.txt", node.path / "typed-create", "wiki")["content_id"]
        base = bytes.fromhex(node.cli("get", typed)["edit_token"])
        decimal = b"90071992547409931234567"
        attribute = b"\x01" + size_prefix(b"wiki") + size_prefix(b"score") + b"\x01\x03" + size_prefix(decimal)
        raw = b"HRQ\x00\x00\x00\x00\x01" + os.urandom(32) + bytes.fromhex(typed) + b"\x01" + base
        edit_file = node.path / "typed-edit"
        edit_file.write_bytes(raw + b"\x00\x00\x00\x01" + attribute)
        node.cli("save", edit_file)
        node.write("delete", typed, node.cli("get", typed)["edit_token"])
        typed_deleted = call("/inspect", {"content":typed}, session)
        fields = [{"field":group["field"],"version":group["candidates"][0]["versions"][0]}
                  for group in typed_deleted["groups"] if group["field"]["type"] != "body"]
        attribute_group = next(group for group in typed_deleted["groups"] if group["field"]["type"] == "attribute")
        assert attribute_group["candidates"][0]["value"]["value"] == decimal.decode()
        call("/resolve", {"content":typed, "token":typed_deleted["token"], "request_id":"4" * 64,
                          "edit":{"type":"restore","text":"typed","fields":fields}}, session)
        typed_content = node.cli("get", typed)["content"]
        assert typed_content["attributes"][0]["value"] == int(decimal)
        assert typed_content["kinds"] == ["wiki"]

        overlap = node.write("create", body="same line\n")["content_id"]
        shared_token = node.cli("get", overlap)["edit_token"]
        left = node.write("edit", overlap, shared_token, body="left line\n")["result"]["update_id"]
        right = node.write("edit", overlap, shared_token, body="right line\n")["result"]["update_id"]
        overlap_view = call("/inspect", {"content":overlap}, session)
        proposed = call("/merge", {"content":overlap, "token":overlap_view["token"],
                                   "left":left,"right":right}, session)
        assert proposed["has_conflicts"] and "<<<<<<<" in proposed["text"]
        assert len(call("/inspect", {"content":overlap}, session)["groups"][0]["candidates"]) == 2

        peer = Node(directory / "peer", node.identity["cluster_id"])
        try:
            peer.start()
            registration = json.loads((peer.path / "peer.json").read_text())
            code = "harmion-peer:v1:z:" + base64.urlsafe_b64encode(gzip.compress(json.dumps(registration).encode())).decode().rstrip("=")
            call("/peer-preview", {"input":code,"action":"approve"}, status=401)
            wrong = {**registration, "cluster_id":"f" * 64}
            wrong_code = "harmion-peer:v1:" + base64.urlsafe_b64encode(json.dumps(wrong).encode()).decode().rstrip("=")
            call("/peer-preview", {"input":wrong_code,"action":"approve"}, session, status=400)
            preview = call("/peer-preview", {"input":code,"action":"approve"}, session)
            assert preview["registration"] == registration
            assert all(member["node_id"] != peer.identity["node_id"]
                       for member in call("/members", {}, session)["members"])
            call("/peer-confirm", {"ticket":preview["ticket"]}, session)
            call("/peer-confirm", {"ticket":preview["ticket"]}, session, status=410)
            peer.cli("join", node.path / "peer.json")
            call("/node", {"type":"sync","node":peer.identity["node_id"]}, session)
            assert peer.cli("get", content)["content"]["body"] == "restored"
            assert any(member["node_id"] == peer.identity["node_id"]
                       for member in call("/members", {}, session)["members"])
        finally:
            peer.stop()
            peer.log.close()
        app = {"name":"Example","redirect_uri":"http://127.0.0.1/callback"}
        package = "harmion-app:v1:" + base64.urlsafe_b64encode(json.dumps(app).encode()).decode().rstrip("=")
        app_preview = call("/app-preview", {"input":package}, session)
        assert app_preview == {"type":"registration",**app}
        registered = call("/oauth-register", app, session)
        oauth_root = directory / "oauth-ca.pem"
        oauth_root.write_bytes(subprocess.check_output([DAEMON,"oauth-root",node.path],timeout=10))
        oauth_ctx = ssl.create_default_context(cafile=oauth_root)
        authorize = registered["authorization_endpoint"] + "?" + urlencode({
            "response_type":"code","client_id":registered["client_id"],"redirect_uri":app["redirect_uri"],
            "state":"test","scope":"read:all","code_challenge":CHALLENGE,"code_challenge_method":"S256"})
        with urlopen(authorize,context=oauth_ctx,timeout=10) as response:
            page = response.read().decode()
        auth_code = re.search(r"harmion-auth:v1:[a-f0-9]{64}",page)[0]
        preview = call("/app-preview", {"input":auth_code}, session)
        assert preview["preview"]["name"] == "Example"
        assert preview["preview"]["grants"]["read"] == {"type":"all"}
        call("/oauth-consent", {"request":preview["request"]}, session)
        call("/change-grants", {"app_id":registered["client_id"],"revoked":True,
                               "grants":{"read":None,"write":None,"manage_apps":False,"manage_nodes":False}}, session)
        exported = directory / "export"
        call("/export", {"directory":str(exported)}, session)
        assert (exported / "bodies" / (content + ".txt")).read_text() == "restored"
        metrics = call("/overview", {}, session)["metrics"]
        assert int(metrics["contents"]) >= 3
        assert int(metrics["accepted_updates"]) > int(metrics["contents"])
        assert int(metrics["record_bytes"]) > 0
        call("/export", {"directory":str(exported)}, session, status=409)
        call("/session/end", {}, session)
        call("/inspect", {"content":content}, session, status=401)
    finally:
        daemon.terminate()
        daemon.wait(timeout=10)
        daemon.stdout.close()


def main():
    with tempfile.TemporaryDirectory(prefix="harmion-admin-") as temp:
        directory = Path(temp)
        node = Node(directory / "node")
        try:
            check(node, directory)
            print("PASS: owner session, origin and route restrictions, conflict resolution, node approval, OAuth and export")
        finally:
            node.stop()
            node.log.close()


if __name__ == "__main__":
    main()
