#!/usr/bin/env python3
"""Exercise the integrated OAuth connection through real daemon/admin binaries."""
import base64
import http.client
import json
import os
from pathlib import Path
import selectors
import ssl
import socket
import struct
import subprocess
import tempfile
from urllib.parse import urlsplit

from oauth_process import CHALLENGE, VERIFIER
from two_process_sync import DAEMON, ROOT, Node

ADMIN = ROOT / "target/debug/harmion-admin"


class AdminWeb:
    def __init__(self, node, directory):
        ui = directory / "ui"
        root = directory / "admin-ca.pem"
        root.write_bytes(subprocess.check_output([ADMIN, "provision", "--ui-dir", ui], timeout=15))
        self.process = subprocess.Popen(
            [str(ADMIN), "run", node.path, "--ui-dir", ui, "--approval", "stdio"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )
        assert self.process.stdin is not None
        self.process.stdin.write("y\n" * 16)
        self.process.stdin.flush()
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdout, selectors.EVENT_READ)
            assert selector.select(10), "admin Web readiness timed out"
            link = urlsplit(json.loads(self.process.stdout.readline())["url"])
        self.link = link
        self.origin = f"https://{link.netloc}"
        self.context = ssl.create_default_context(cafile=root)

    def call(self, path, data=None, session=None, status=200):
        connection = http.client.HTTPSConnection(
            self.link.hostname, self.link.port, context=self.context, timeout=8
        )
        try:
            headers = {"Host": self.link.netloc, "Origin": self.origin}
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
            if response.getheader("content-type") == "application/json":
                return json.loads(output)
            return output
        finally:
            connection.close()

    def session(self):
        return self.call("/session", self.link.fragment)["session"]

    def close(self):
        self.process.terminate()
        self.process.wait(timeout=10)
        self.process.stdout.close()


class OAuthWeb:
    def __init__(self, origin, root):
        parsed = urlsplit(origin)
        self.host = parsed.hostname
        self.port = parsed.port
        self.netloc = parsed.netloc
        self.origin = origin
        self.context = ssl.create_default_context(cafile=root)

    def call(self, method, path, body=None, headers=None, status=200):
        connection = http.client.HTTPSConnection(
            self.host, self.port, context=self.context, timeout=8
        )
        try:
            merged = {"Host": self.netloc}
            if headers:
                merged.update(headers)
            payload = None
            if body is not None:
                payload = json.dumps(body).encode()
                merged.setdefault("Content-Type", "application/json")
            connection.request(method, path, payload, merged)
            response = connection.getresponse()
            output = response.read()
            assert response.status == status, (path, response.status, output)
            assert response.getheader("cache-control") == "no-store"
            if response.getheader("content-type") == "application/json":
                return json.loads(output)
            return output
        finally:
            connection.close()


class AppPinned:
    class Connection(http.client.HTTPSConnection):
        def connect(self):
            http.client.HTTPConnection.connect(self)
            self.sock = self._context.wrap_socket(self.sock, server_hostname="harmion.local")

    def __init__(self, endpoint, certificate):
        self.host, self.port = endpoint.split(":")
        self.port = int(self.port)
        context = ssl.create_default_context(cafile=certificate)
        self.context = context

    def call(self, operation, token, body=b"", status=200):
        connection = self.Connection(
            self.host, self.port, timeout=8, context=self.context
        )
        try:
            connection.request(
                "POST",
                "/app/v1/" + operation,
                body,
                {"Host": "harmion.local", "Authorization": "Bearer " + token},
            )
            response = connection.getresponse()
            result = json.loads(response.read())
            assert response.status == status, (operation, response.status, result)
            return result
        finally:
            connection.close()


def read_exact(sock, size):
    data = b""
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        if not chunk:
            raise AssertionError("WebSocket closed unexpectedly")
        data += chunk
    return data


def read_frame(sock, buffer):
    while len(buffer) < 2:
        buffer += sock.recv(8192)
    first, second = buffer[0], buffer[1]
    length = second & 0x7F
    offset = 2
    if length == 126:
        while len(buffer) < 4:
            buffer += sock.recv(8192)
        length = struct.unpack("!H", buffer[2:4])[0]
        offset = 4
    elif length == 127:
        while len(buffer) < 10:
            buffer += sock.recv(8192)
        length = struct.unpack("!Q", buffer[2:10])[0]
        offset = 10
    while len(buffer) < offset + length:
        buffer += sock.recv(8192)
    payload = buffer[offset : offset + length]
    return first & 0x0F, payload, buffer[offset + length :]


def send_text(sock, text):
    payload = text.encode()
    mask = os.urandom(4)
    if len(payload) < 126:
        header = bytes([0x81, 0x80 | len(payload)])
    elif len(payload) < 65536:
        header = bytes([0x81, 0x80 | 126]) + struct.pack("!H", len(payload))
    else:
        raise AssertionError("test payload too large")
    masked = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
    sock.sendall(header + mask + masked)


def provider_attach(websocket_url, oauth_root, provider_id):
    parsed = urlsplit(websocket_url)
    context = ssl.create_default_context(cafile=oauth_root)
    tcp = socket.create_connection((parsed.hostname, parsed.port), timeout=8)
    sock = context.wrap_socket(tcp, server_hostname=parsed.hostname)
    try:
        key = base64.b64encode(os.urandom(16)).decode()
        request = (
            f"GET {parsed.path}?{parsed.query} HTTP/1.1\r\n"
            f"Host: {parsed.netloc}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            "Sec-WebSocket-Version: 13\r\n"
            f"Sec-WebSocket-Key: {key}\r\n\r\n"
        )
        sock.sendall(request.encode())
        response = b""
        while b"\r\n\r\n" not in response:
            response += sock.recv(8192)
        headers, buffer = response.split(b"\r\n\r\n", 1)
        assert b" 101 " in headers.split(b"\r\n", 1)[0], headers
        send_text(
            sock,
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": "attach",
                    "method": "interpreter.attach",
                    "params": {"provider_id": provider_id},
                }
            ),
        )
        while True:
            opcode, payload, buffer = read_frame(sock, buffer)
            if opcode == 0x9:
                mask = os.urandom(4)
                sock.sendall(
                    bytes([0x8A, 0x80 | len(payload)])
                    + mask
                    + bytes(v ^ mask[i % 4] for i, v in enumerate(payload))
                )
                continue
            if opcode == 0x8:
                raise AssertionError("provider closed before attach response")
            if opcode == 0x1:
                result = json.loads(payload)
                assert result["result"]["ok"] is True, result
                assert result["result"]["provider_id"] == provider_id
                return
    finally:
        sock.close()


def check(node, directory):
    subprocess.check_output([DAEMON, "oauth-provision", node.path], timeout=10)
    node.start(extra_options=("--oauth-listen", "127.0.0.1:0"))
    oauth_origin = node.ready["oauth_origin"]
    assert oauth_origin.startswith("https://127.0.0.1:")
    connection = json.loads((node.path / "connection.json").read_text())
    assert connection["oauth_origin"] == oauth_origin
    oauth_root = directory / "oauth-ca.pem"
    oauth_root.write_bytes(subprocess.check_output([DAEMON, "oauth-root", node.path]))
    admin = AdminWeb(node, directory)
    session = admin.session()
    oauth = OAuthWeb(oauth_origin, oauth_root)
    try:
        html = admin.call("/").decode()
        assert 'id="app"' in html

        web_origin = "http://127.0.0.1:43123"
        offer = {
            "version": 1,
            "name": "Real process app",
            "redirect_uri": web_origin + "/callback",
            "web_origin": web_origin,
            "challenge": CHALLENGE,
            "state": "real-state",
            "grants": {
                "read": {"type": "all"},
                "call": True,
                "provide": True,
                "provide_names": ["ledger/balance"],
            },
            "provider": {
                "name": "ledger/balance",
                "contract_version": 1,
                "input_schema": {"type": "object"},
                "output_schema": {"type": "object"},
                "default_query": {"kinds": ["ledger"]},
            },
            "provider_replace": False,
        }
        headers = {"Origin": web_origin}
        started = oauth.call("POST", "/connect/start", offer, headers)
        assert started["verification_url"].startswith(oauth_origin + "/connect#")
        menu = admin.call("/connect-list", {}, session)
        request = next(
            item for item in menu["connections"] if item["connection_id"] == started["connection_id"]
        )
        assert request["status"] == "pending"
        assert request["provider"]["name"] == "ledger/balance"
        approved = admin.call(
            "/connect-approve",
            {"connection_id": started["connection_id"], "allow": True, "replace": False},
            session,
        )
        assert approved["status"] == "approved"

        status = oauth.call(
            "POST",
            "/connect/status",
            {
                "connection_id": started["connection_id"],
                "request_secret": started["request_secret"],
            },
            headers,
        )
        assert status["status"] == "approved"
        completed = oauth.call(
            "POST",
            "/connect/complete",
            {
                "connection_id": started["connection_id"],
                "request_secret": started["request_secret"],
                "code_verifier": VERIFIER,
            },
            headers,
        )
        assert completed["token_type"] == "Bearer"
        assert completed["provider"]["name"] == "ledger/balance"
        assert completed["provider"]["ticket"]
        access = completed["access_token"]
        ticket = oauth.call(
            "POST",
            "/connect/ticket",
            {"provider_id": completed["provider"]["provider_id"]},
            {"Origin": web_origin, "Authorization": "Bearer " + access},
        )
        assert ticket["ticket"]

        certificate = directory / "app-pin.pem"
        certificate.write_text(
            ssl.DER_cert_to_PEM_cert((node.path / "certificate.der").read_bytes())
        )
        app = AppPinned(node.ready["app_endpoint"], certificate)
        permissions = app.call("permissions", access)
        assert permissions["call"] is True
        assert permissions["provide_names"] == ["ledger/balance"]
        provider_attach(ticket["websocket_url"], oauth_root, completed["provider"]["provider_id"])

        preflight = oauth.call(
            "OPTIONS",
            "/connect/start",
            headers={
                "Origin": web_origin,
                "Access-Control-Request-Method": "POST",
            },
            status=204,
        )
        assert preflight == b""
        page = oauth.call("GET", "/connect").decode()
        assert "Harmion connection" in page
        oauth.call(
            "POST",
            "/connect/status",
            {"connection_id": started["connection_id"], "request_secret": started["request_secret"]},
            {"Origin": web_origin, "Authorization": "Bearer " + access},
            status=200,
        )
        print("PASS: real daemon/admin integrated connection, CORS, ticket, provider attach")
    finally:
        admin.close()


def main():
    with tempfile.TemporaryDirectory(prefix="harmion-connect-") as temp:
        directory = Path(temp)
        node = Node(directory / "node")
        try:
            check(node, directory)
        except Exception:
            node.log.flush()
            print((node.path / "test-stderr").read_text())
            raise
        finally:
            node.stop()
            node.log.close()


if __name__ == "__main__":
    main()
