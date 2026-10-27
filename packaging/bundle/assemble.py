"""Assemble the single-file bundle: the static binary, then the runtime
payload zip, then a trailer (payload length as u64 LE, then a magic string)
that the binary reads to find its payload. Run inside the bundle build."""
import os
import struct

n = os.path.getsize("/bundle/payload.zip")
with open("/bundle/bifrost", "wb") as out:
    for part in ("/bundle/bifrost.bin", "/bundle/payload.zip"):
        with open(part, "rb") as f:
            while chunk := f.read(1 << 20):
                out.write(chunk)
    out.write(struct.pack("<Q", n) + b"BIFROST-BUNDLE01")
os.chmod("/bundle/bifrost", 0o755)
