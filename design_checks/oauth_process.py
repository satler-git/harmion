#!/usr/bin/env python3
"""Verify the native OAuth flow against a real daemon with independent TLS trust."""
import http.client
import json
from pathlib import Path
import re
import ssl
import subprocess
import tempfile
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlencode, urlsplit
from urllib.request import Request, urlopen

from two_process_sync import DAEMON, Node


VERIFIER = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
CHALLENGE = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"


def flow(node, directory):
    root = directory / "oauth-root.pem"
    root.write_bytes(subprocess.check_output([DAEMON, "oauth-provision", node.path]))
    assert (node.path / "oauth-identity.json").stat().st_mode & 0o077 == 0
    assert subprocess.check_output([DAEMON, "oauth-root", node.path]) == root.read_bytes()
    node.start(extra_options=("--oauth-listen", "127.0.0.1:0"))
    origin = node.ready["oauth_origin"]
    assert node.cli("capabilities")["oauth_origin"] == origin
    assert origin.startswith("https://127.0.0.1:")
    try:
        urlopen(origin + "/oauth/consent", timeout=5)
    except OSError as error:
        assert isinstance(error.reason, ssl.SSLCertVerificationError), error
    else:
        raise AssertionError("an untrusted OAuth root was accepted")
    context = ssl.create_default_context(cafile=root)

    def browser(path, data=None, browser_origin=None, status=200):
        headers = {"Origin": browser_origin} if browser_origin else {}
        request = Request(origin + path, urlencode(data).encode() if data else None, headers)
        try:
            reply = urlopen(request, context=context, timeout=5)
        except HTTPError as error:
            reply = error
        with reply:
            assert reply.status == status, (reply.status, path, reply.read())
            assert reply.headers["Cache-Control"] == "no-store"
            return reply.read().decode()

    class AppConnection(http.client.HTTPSConnection):
        def connect(self):
            http.client.HTTPConnection.connect(self)
            self.sock = self._context.wrap_socket(self.sock, server_hostname="harmion.local")

    app_cert = directory / "app-pin.pem"
    app_cert.write_text(ssl.DER_cert_to_PEM_cert((node.path / "certificate.der").read_bytes()))
    app_context = ssl.create_default_context(cafile=app_cert)

    def app_call(operation, token, body=b"", status=200):
        host, port = node.ready["app_endpoint"].split(":")
        app = AppConnection(host, port, timeout=5, context=app_context)
        try:
            app.request("POST", "/app/v1/" + operation, body, {
                "Host": "harmion.local", "Authorization": "Bearer " + token,
            })
            reply = app.getresponse()
            result = json.loads(reply.read())
            assert reply.status == status, (operation, reply.status, result)
            return result
        finally:
            app.close()

    state = "client-owned random state +%"
    registered = node.cli("oauth-register", "Example app", "http://127.0.0.1/callback?x=y")
    client = registered["client_id"]
    assert registered["authorization_endpoint"] == origin + "/oauth/authorize"
    assert registered["token_endpoint"] == origin + "/oauth/token"
    callback = "http://127.0.0.1:49152/callback?x=y"
    desired_grants = {"read": {"type": "kinds", "kinds": ["Note"]}, "write": None,
                      "manage_apps": False, "manage_nodes": False}
    params = {"response_type": "code", "client_id": client, "redirect_uri": callback,
              "scope": "read:kind:Tm90ZQ", "state": state, "code_challenge": CHALLENGE,
              "code_challenge_method": "S256"}

    def authorize(decision="approve", access_token=None):
        page = browser("/oauth/authorize?" + urlencode(params))
        request = re.search(r"oauth-consent ([a-f0-9]{64})", page)[1]
        if access_token:
            app_call("oauth-consent", access_token, bytes.fromhex(request), status=403)
        link = node.cli("oauth-consent", request)["url"]
        location = urlsplit(link)
        assert location.path == "/oauth/consent"
        assert "Harmion consent" in browser(location.path)
        ticket = {"ticket": location.fragment}
        browser("/oauth/preview", ticket, "https://other.example", status=403)
        preview = json.loads(browser("/oauth/preview", ticket, origin))
        assert preview == {"name": "Example app", "client_id": client, "redirect_uri": callback,
                           "grants": desired_grants}
        choice = {**ticket, "decision": decision}
        result = json.loads(browser("/oauth/decision", choice, origin))
        browser("/oauth/decision", choice, origin, status=403)
        redirect = urlsplit(result["redirect_uri"])
        assert (redirect.scheme, redirect.netloc, redirect.path) == ("http", "127.0.0.1:49152", "/callback")
        data = parse_qs(redirect.query)
        assert data["state"] == [state] and data["x"] == ["y"]
        return data

    def exchange(code, status=200, **overrides):
        data = {"grant_type": "authorization_code", "client_id": client, "code": code,
                "redirect_uri": callback, "code_verifier": VERIFIER}
        return json.loads(browser("/oauth/token", {**data, **overrides}, status=status))

    def refresh(token, status=200):
        data = {"grant_type": "refresh_token", "client_id": client, "refresh_token": token}
        return json.loads(browser("/oauth/token", data, status=status))

    for invalid in [{"redirect_uri": callback.replace("/callback", "/other")},
                    {"scope": "manage_apps"}, {"code_challenge_method": "plain"}]:
        browser("/oauth/authorize?" + urlencode({**params, **invalid}), status=400)
    assert authorize("deny")["error"] == ["access_denied"]
    code = authorize()["code"][0]
    assert exchange(code, status=400, code_verifier="x" * 43)["error"] == "invalid_grant"
    assert exchange(code, status=400, redirect_uri=callback.replace("49152", "49153"))["error"] == "invalid_grant"
    first = exchange(code)
    assert first["token_type"] == "Bearer" and first["expires_in"] == 600
    assert first["scope"] == params["scope"]
    access = first["access_token"]
    assert app_call("permissions", access) == desired_grants
    allowed = node.write("create", body="readable")["content_id"]
    # Existing CLI adds kinds after the request path, so create the filtered item explicitly.
    noted = node.cli("create", node.path / "body.txt", node.path / "note-request", "Note")["content_id"]
    assert app_call("get", access, bytes.fromhex(noted))["content"]["body"] == "readable"
    app_call("get", access, bytes.fromhex(allowed), status=404)
    authorize("deny", access)
    assert exchange(code, status=400)["error"] == "invalid_grant"
    app_call("permissions", access, status=401)
    assert refresh(first["refresh_token"], status=400)["error"] == "invalid_grant"

    first = exchange(authorize()["code"][0])
    node.stop()
    node.start()
    assert node.ready["oauth_origin"] == origin
    second = refresh(first["refresh_token"])
    assert second["refresh_token"] != first["refresh_token"]
    assert app_call("permissions", second["access_token"]) == desired_grants
    assert refresh(first["refresh_token"], status=400)["error"] == "invalid_grant"
    app_call("permissions", second["access_token"], status=401)
    third = exchange(authorize()["code"][0])
    grants = {**desired_grants, "read": None}
    def change_grants(grants, revoked=False, ok=True):
        request = directory / "change-grants.json"
        request.write_text(json.dumps({"app_id": client, "grants": grants, "revoked": revoked}))
        node.cli("change-grants", request, ok=ok)

    change_grants(grants)
    assert app_call("permissions", third["access_token"]) == grants
    change_grants(desired_grants, ok=False)
    change_grants(grants, revoked=True)
    app_call("permissions", third["access_token"], status=401)
    assert refresh(third["refresh_token"], status=400)["error"] == "invalid_grant"


def main():
    with tempfile.TemporaryDirectory(prefix="harmion-oauth-") as temp:
        directory = Path(temp)
        node = Node(directory / "node")
        try:
            flow(node, directory)
            print("PASS: independent TLS trust, consent and PKCE, scoped App API, replay, restart, revocation")
        finally:
            node.stop()
            node.log.close()


if __name__ == "__main__":
    main()
