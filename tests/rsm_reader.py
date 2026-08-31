"""Minimal reader for the RSM binary format (spec: cad_io/src/rsm.rs header).

Walks the fixed tables (linetype / layer / pen) so the DObjects count and the
first entity's geom tag can be read. Version gate: the layer record gained a
trailing `order` u32 at v20; the current writer emits v34, so the reader
always consumes it.

Only used for count + first-geom-tag verification (the plan's v1 scope);
full geom payload parsing is deliberately out of scope.
"""

from __future__ import annotations

import struct

MAGIC = b"RSM\x01"


class RsmError(Exception):
    pass


class _R:
    def __init__(self, data: bytes):
        self.data = data
        self.pos = 0

    def take(self, n: int) -> bytes:
        if self.pos + n > len(self.data):
            raise RsmError("truncated file")
        b = self.data[self.pos: self.pos + n]
        self.pos += n
        return b

    def u8(self) -> int:
        return self.take(1)[0]

    def u16(self) -> int:
        return struct.unpack("<H", self.take(2))[0]

    def u32(self) -> int:
        return struct.unpack("<I", self.take(4))[0]

    def u64(self) -> int:
        return struct.unpack("<Q", self.take(8))[0]

    def f32(self) -> float:
        return struct.unpack("<f", self.take(4))[0]

    def f64(self) -> float:
        return struct.unpack("<d", self.take(8))[0]

    def string(self) -> str:
        n = self.u32()
        return self.take(n).decode("utf-8", errors="replace")

    def color(self):
        tag = self.u8()
        if tag == 0 or tag == 1:
            return tag
        if tag == 2:
            self.take(1)
            return tag
        if tag == 3:
            self.take(4)
            return tag
        raise RsmError(f"bad color tag {tag}")

    def lineweight(self):
        tag = self.u8()
        if tag in (0, 1, 2):
            return tag
        if tag == 3:
            self.take(4)
            return tag
        raise RsmError(f"bad lineweight tag {tag}")


def read_rsm(data: bytes) -> dict:
    """Returns {version, dobject_count, first_geom_tag (int|None)}."""
    if len(data) < 8:
        raise RsmError("file too short for header")
    if data[:4] != MAGIC:
        raise RsmError(f"bad magic {data[:4]!r}")
    r = _R(data)
    r.take(4)  # magic
    version = r.u16()
    r.u16()  # pad
    if version < 20:
        raise RsmError(f"unsupported version {version} (need >= 20 for the "
                       f"layer draw-order field)")

    # --- LinetypeTable ---
    lt_count = r.u32()
    for _ in range(lt_count):
        r.string()      # name
        r.string()      # description
        pat_len = r.u32()
        r.take(pat_len * 4)  # pattern f32s

    # --- LayerTable ---
    r.u32()  # active
    layer_count = r.u32()
    for _ in range(layer_count):
        r.string()          # name
        r.color()
        r.u32()             # linetype id
        r.lineweight()
        r.u8()              # flags
        r.u32()             # v20 draw order

    # --- PenTable ---
    pen_count = r.u32()
    for _ in range(pen_count):
        r.string()      # name
        r.color()
        r.u32()         # linetype
        r.lineweight()

    # --- DObjects ---
    dobject_count = r.u32()

    first_geom_tag = None
    if dobject_count > 0:
        r.u64()             # handle
        r.u32()             # style.layer
        r.color()           # style.color
        r.u32()             # style.linetype
        r.f32()             # style.linetype_scale
        r.lineweight()      # style.lineweight
        r.u8()              # style.visible
        r.u8()              # v34 style.hatch_aux
        first_geom_tag = r.u8()

    return {
        "version": version,
        "dobject_count": dobject_count,
        "first_geom_tag": first_geom_tag,
    }
