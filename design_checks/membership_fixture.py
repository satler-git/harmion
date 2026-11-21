#!/usr/bin/env python3
"""Independent HMC v1/v2 byte fixtures, not signature or membership evaluation.

Signatures below are opaque repeated bytes. These fixtures pin the canonical
certificate layout, the unchanged HMC v1 bytes, and the explicit HMC v2
SignedHttps binding. Run with --write to generate, default verifies committed
files against this independent encoder.
"""
import hashlib
from pathlib import Path
import struct
import sys

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "contracts" / "fixtures"

def u32(n): return struct.pack(">I", n)
def repeated(n): return bytes([n]) * 32
def key_of(n): return bytes([n]) * 32

cluster = 1
issuer = 2
issuer_key = 2
signature = bytes([13]) * 64

def header(version, signing_key, nonce):
    return (b"HMC\0" + u32(version) + repeated(cluster) + repeated(issuer)
            + repeated(signing_key) + repeated(nonce))

def admit(version, node, member_key, tag, binding, endpoint, nonce=0,
          signing_key=issuer_key):
    out = header(version, signing_key, nonce)
    out += b"\0" + repeated(node) + repeated(member_key) + bytes([tag])
    if tag == 0:
        out += u32(len(binding)) + binding
    else:
        out += binding
    out += u32(len(endpoint)) + endpoint
    return out

def retire(target, retained):
    out = header(1, issuer_key, 0)
    out += b"\1" + repeated(target) + u32(len(retained))
    for value in sorted(retained):
        out += repeated(value)
    return out

def dismiss(retirements):
    out = header(1, issuer_key, 0)
    out += b"\2" + u32(len(retirements))
    for value in sorted(retirements):
        out += repeated(value)
    return out

certificates = {
    # Unchanged HMC v1: native TLS leaf binding + endpoint.
    "hmc-v1-https": admit(1, 5, 6, 0, b"DER-TLS-CERTIFICATE-BYTES",
                          b"127.0.0.1:8443"),
    # Unchanged HMC v1: iroh endpoint key, empty relay hint.
    "hmc-v1-iroh": admit(1, 7, 8, 1, repeated(8), b""),
    # HMC v2: explicit SignedHttps binding repeating the signing key.
    "hmc-v2-signed-https": admit(2, 9, 9, 2, repeated(9),
                                 b"https://replica.example", signing_key=9),
    # Retire/Dismiss keep v1 bytes: version is not a transport change.
    "hmc-v1-retire": retire(5, [11, 12]),
    "hmc-v1-dismiss": dismiss([11]),
}

files = {}
for name, signed in certificates.items():
    complete = signed + signature
    files[name + ".hex"] = complete.hex() + "\n"
    files[name + "-signed.hex"] = signed.hex() + "\n"
    files[name + ".sha256"] = hashlib.sha256(complete).hexdigest() + "\n"

if __name__ == "__main__":
    write = sys.argv[1:] == ["--write"]
    if sys.argv[1:] not in ([], ["--write"]):
        raise SystemExit("usage: membership_fixture.py [--write]")
    for name, text in files.items():
        path = OUT / name
        if write:
            path.write_text(text, encoding="ascii")
        elif path.read_text(encoding="ascii") != text:
            raise SystemExit(f"fixture mismatch: {path}")
    print(f"{'wrote' if write else 'verified'} {len(files)} independent HMC fixture files")
