#!/usr/bin/env python3
"""Independent struct/hashlib HHT v2 fixtures, not signature/auth validation.

Run with --write to generate files; default checks committed bytes unchanged.
Uses only Python's standard library, not Rust output or the Rust codec.
"""
import hashlib
from pathlib import Path
import struct
import sys

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "contracts" / "fixtures"

def u32(n): return struct.pack(">I", n)
def u16(n): return struct.pack(">H", n)
def repeated(n): return bytes([n]) * 32
def blob(b): return u32(len(b)) + b
def route(b): return b"\0" + blob(b)
def digest(b): return hashlib.sha256(b).digest()

origin = b"https://replica.example"
challenge_path = b"/peer/v2/challenge"
exchange_path = b"/peer/v2/exchange"
request = b"HPQ\0" + u32(1) + repeated(1) + repeated(2) + b"\0"
response = (b"HPS\0" + u32(1) + repeated(1) + repeated(3) + b"\x03"
            + repeated(9) + struct.pack(">Q", 4))
commitment = b"\0" + u32(len(request)) + digest(request)
signature = bytes([11]) * 64  # Deliberately not a cryptographic signature.
frames = {}

def frame(name, kind, sender, audience, path, body, domain=None):
    unsigned = (b"HHT\0" + u32(2) + bytes([kind]) + repeated(1)
                + repeated(sender) + repeated(audience) + blob(origin)
                + route(path) + body)
    complete = unsigned + signature
    signing = b"harmion/https-peer/v2/" + (domain or name).encode() + b"\0" + unsigned
    stem = "https-peer-v2-" + name
    frames[stem + ".hex"] = complete.hex() + "\n"
    frames[stem + "-signing.hex"] = signing.hex() + "\n"
    frames[stem + ".sha256"] = digest(complete).hex() + "\n"
    return complete

q = frame("challenge-request", 0, 2, 3, challenge_path,
          repeated(4) + route(exchange_path) + commitment)
c = frame("challenge", 1, 3, 2, challenge_path,
          digest(q) + repeated(4) + repeated(5) + repeated(6)
          + u32(60000) + route(exchange_path) + commitment)
x = frame("exchange", 2, 2, 3, exchange_path,
          digest(q) + digest(c) + repeated(4) + repeated(5) + repeated(6)
          + b"\0" + blob(request))
frame("response", 3, 3, 2, exchange_path,
      digest(x) + digest(c) + repeated(4) + repeated(5) + repeated(6)
      + u16(200) + b"\0" + blob(response))
frame("challenge-error", 4, 3, 2, challenge_path,
      digest(q) + repeated(4) + u16(429) + u16(6) + u32(250))
# Extra response-error fixture exercises the length-prefixed six-byte error.
frame("response-error", 3, 3, 2, exchange_path,
      digest(x) + digest(c) + repeated(4) + repeated(5) + repeated(6)
      + u16(503) + b"\x01" + blob(u16(7) + u32(500)), domain="response")

if __name__ == "__main__":
    write = sys.argv[1:] == ["--write"]
    if sys.argv[1:] not in ([], ["--write"]):
        raise SystemExit("usage: https_peer_fixture.py [--write]")
    for name, text in frames.items():
        path = OUT / name
        if write:
            path.write_text(text, encoding="ascii")
        elif path.read_text(encoding="ascii") != text:
            raise SystemExit(f"fixture mismatch: {path}")
    print(f"{'wrote' if write else 'verified'} {len(frames)} independent HTTPS Peer fixture files")
