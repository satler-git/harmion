"""Independent record-v1 fixture generator (Python standard library only).

Run from repository root. Without --write, verify checked-in hex/digest fixtures.
This deliberately does not call the Rust codec.
"""
import hashlib
from pathlib import Path
import struct
import sys


def string(text):
    data = text.encode("utf-8")
    return struct.pack(">I", len(data)) + data


def fixture():
    # create: cluster=01, content=02, root/update=03, author=04, time=i64::MIN
    header = b"HUP\0" + struct.pack(">I", 1)
    header += b"".join(bytes([n]) * 32 for n in (1, 2, 3, 3, 4))
    header += struct.pack(">qBII", -(2**63), 0, 0, 4)
    body = b"\0\0" + string("本文\n") + struct.pack(">I", 0)
    # attribute(app, data) = {"array": [null,false,true,number,string], "object": {}}
    data = b"\x06" + struct.pack(">I", 2)
    data += string("array") + b"\x05" + struct.pack(">I", 5)
    data += b"\0\x01\x02\x03" + string("9007199254740993") + b"\x04" + string("")
    data += string("object") + b"\x06" + struct.pack(">I", 0)
    attribute = b"\x01" + string("app") + string("data") + b"\x01" + data + struct.pack(">I", 0)
    absent = b"\x01" + string("app") + string("unset") + b"\x02" + struct.pack(">I", 0)
    kind = b"\x02" + string("wiki") + b"\x04" + struct.pack(">I", 0)
    return header + body + attribute + absent + kind


data = fixture()
root = Path(__file__).resolve().parents[1] / "contracts" / "fixtures"
outputs = {"create-v1.hex": data.hex() + "\n", "create-v1.sha256": hashlib.sha256(data).hexdigest() + "\n"}
# Independent Peer status fixtures: cluster=01, node=02, epoch=09, high=4.
outputs["peer-status-request-v1.hex"] = (b"HPQ\0" + struct.pack(">I", 1) + bytes([1])*32 + bytes([2])*32 + b"\0").hex() + "\n"
outputs["peer-status-response-v1.hex"] = (b"HPS\0" + struct.pack(">I", 1) + bytes([1])*32 + bytes([2])*32 + b"\0"
    + bytes([9])*32 + struct.pack(">QIIII", 4, 1, 1024*1024, 4*1024*1024, 128)).hex() + "\n"
# Membership codec fixtures deliberately use zero signatures. They test the
# encoding independently; signature/authority verification is a separate layer.
membership_header = b"HMC\0" + struct.pack(">I", 1) + b"".join(bytes([n])*32 for n in [1, 2, 3, 4])
outputs["membership-admit-v1.hex"] = (membership_header + b"\0" + bytes([5])*32 + bytes([6])*32
    + b"\1" + bytes([7])*32 + struct.pack(">I", 0) + bytes(64)).hex() + "\n"
outputs["membership-retire-v1.hex"] = (membership_header + b"\1" + bytes([5])*32
    + struct.pack(">I", 2) + bytes([8])*32 + bytes([9])*32 + bytes(64)).hex() + "\n"
for name, expected in outputs.items():
    path = root / name
    if "--write" in sys.argv:
        path.write_text(expected)
    else:
        assert path.read_text() == expected, name
print(f"record v1 fixture: {len(data)} bytes, SHA-256 {hashlib.sha256(data).hexdigest()}")
