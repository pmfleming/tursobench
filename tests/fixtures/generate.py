#!/usr/bin/env python3
"""Closed-file compatibility fixture, generated with Python's stdlib SQLite.
No SQLite runtime/adapter is linked into TursoBench. Own fixture, no copied data.
Refuses to overwrite an existing file. Use only in disposable test directories.
"""
import base64
import gzip
import hashlib
import pathlib
import plistlib
import random
import sqlite3
import struct
import sys
import zlib

# Small images and a zstd frame made once with the Rust `image` and `zstd`
# crates the viewer uses; the stdlib can write PNG and BMP but not these.
JPEG = base64.b64decode(
    "/9j/4AAQSkZJRgABAgAAAQABAAD/wAARCAAgADADAREAAhEBAxEB/9sAQwAIBgYHBgUIBwcHCQkICgwUDQwLCwwZEhMPFB0aHx4dGhwcICQuJyAiLCMcHCg3KSwwMTQ0NB8nOT04MjwuMzQy/9sAQwEJCQkMCwwYDQ0YMiEcITIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIy/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDGrpPmwoAKALVjpt3qLutpCZCgy3IAH4msK2IpUEnUdrnRQw1Wu2qSvYnudB1S0hMs1o4jHUqQ2O+TgnA96zp47D1JcsZa/d+ZrVy/E0o804afJ/kZ1dZxBQAUAFAHWeG/EFlY2H2S6/clWLB1QkNnHXGTnr26AV4eY4CtVqe0p6+V/wCtP1PfyzMaNGl7Kpp5239fP9DXv/EmlQQzR+aly4XHlKMq+e2cYxzz/kVw0MuxM5J25V37fLc9DEZnhYRkr8z7d/nt6nnlfVHyBVn1C1t8h5QWGflXk5Hb2/GldGkacpbIzpteOcQQjGern+g/xqeY1jQ7srHV55IGilVXBH3uh65+lLmL9lFO6JIdVkQ/6wkZ6Pz+tHMxSpJmhFqqNjzEI6cryKrmMnSfQuRzxTf6tw3t3/KnczcWtzj6zO8KACgAoAVWZfukigCVblh94A/pQLlP/9k=")
WEBP = base64.b64decode(
    "UklGRhYBAABXRUJQVlA4TAoBAAAvL8AHEM1VICICHgjKFeQAAIAHAAAAAAAAAPgAAAAAAAAAgAIAAAAAAAAABgAAAQAABAAAmbpsd+dOHcADIcQBAAAAOP8DQACAABAAAAAAAAAAAAAAHjAAAAAAAAMAAAAAAAAAAADggZATAAAAAJz/eQAAAAAAAAAAAAAwAAAAAAAAAAAAQAAAAAAAAAAAAIAAkT//4as77xakzjtY591A5LyD8HkHl+cdfKBu8FbdIKHbwNYNQroZRrdfCOo2EKkaiFcZMloDu8rwVMswWoZdZbivZVhanQZ2leGOluGPToZdZbjWMtzqZNhVhlctw4NOhl1lsBY87mTYVYZIJ0O8kyHeyRDvZPh3AA==")
GIF = base64.b64decode(
    "R0lGODlhGAAQAIAAAAAAAAAAACH5BAgyAAAALAAAAAAYABAAgRZAOh5UTC6AePLAYAIxlI+py+0P25gjKkrtwVkbXnmbKASkARjBakbACwMsy8R2PK/Hzdt50AviVsKi8fgqAAAh+QQIMgAAACwAAAAAGAAQAIENwGDRgHjhVEzpQDoCMYyPqcvtDxuYICpK7cFZG155mxgIpDEYwmpGwwsPLMvEdjyvx83budAL4lbCovH4KgAAOw==")
ZSTD = base64.b64decode(
    "KLUv/WAoBF0CACQEeyJqb2IiOiJleHBvcnQtODEyIiwicm93cyI6NDgyMTMsInRhcmdldCI6InMzOi8vYXJjaGl2ZS8yMDI2L3EzIn0KAQDCOMUfog==")
META = {"job": "export-812", "rows": 48213, "target": "s3://archive/2026/q3",
        "retries": 2, "warnings": ["slow disk", "retry on part 7"], "done": True, "ratio": 0.5}


def picture(width, height):
    """A sun over two hills, as RGB rows."""
    rows = []
    for y in range(height):
        row = bytearray()
        for x in range(width):
            fx, fy = x / width, y / height
            if (fx - 0.68) ** 2 + (fy - 0.3) ** 2 < 0.14 ** 2:
                row += bytes((242, 192, 96))
            elif fy > 0.55 + 0.5 * abs(fx - 0.35):
                row += bytes((22, 64, 58))
            else:
                row += bytes((46, 128, 120))
        rows.append(bytes(row))
    return rows


def png(width, height, rows=None):
    rows = rows or picture(width, height)
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    raw = b"".join(b"\0" + row for row in rows)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def bmp(width, height):
    stride = (width * 3 + 3) & ~3
    pixels = b"".join(bytes(row[i + 2 - j] for i in range(0, len(row), 3) for j in range(3)).ljust(stride, b"\0")
                      for row in reversed(picture(width, height)))
    header = struct.pack("<IiiHHIIiiII", 40, width, height, 1, 24, 0, len(pixels), 2835, 2835, 0, 0)
    return b"BM" + struct.pack("<IHHI", 14 + len(header) + len(pixels), 0, 0, 54) + header + pixels


def ico(side):
    image = png(side, side)
    return struct.pack("<HHH", 0, 1, 1) + struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(image), 22) + image


def jsonb(value):
    """SQLite's JSONB encoding (https://sqlite.org/jsonb.html)."""
    def element(kind, payload):
        n = len(payload)
        if n <= 11:
            header = bytes((n << 4 | kind,))
        elif n <= 0xFF:
            header = bytes((0xC0 | kind, n))
        elif n <= 0xFFFF:
            header = bytes((0xD0 | kind,)) + n.to_bytes(2, "big")
        else:
            header = bytes((0xE0 | kind,)) + n.to_bytes(4, "big")
        return header + payload
    if value is None:
        return element(0, b"")
    if value is True or value is False:
        return element(1 if value else 2, b"")
    if isinstance(value, int):
        return element(3, str(value).encode())
    if isinstance(value, float):
        return element(5, repr(value).encode())
    if isinstance(value, str):
        plain = not any(c in value for c in '"\\') and all(ord(c) >= 0x20 for c in value)
        return element(7 if plain else 10, value.encode())
    if isinstance(value, list):
        return element(11, b"".join(jsonb(v) for v in value))
    return element(12, b"".join(jsonb(k) + jsonb(v) for k, v in value.items()))


def bson(document):
    def element(key, value):
        name = key.encode() + b"\0"
        if isinstance(value, bool):
            return b"\x08" + name + bytes((value,))
        if isinstance(value, int):
            return b"\x10" + name + struct.pack("<i", value)
        if isinstance(value, float):
            return b"\x01" + name + struct.pack("<d", value)
        if isinstance(value, str):
            data = value.encode() + b"\0"
            return b"\x02" + name + struct.pack("<i", len(data)) + data
        if isinstance(value, list):
            return b"\x04" + name + bson({str(i): v for i, v in enumerate(value)})
        return b"\x03" + name + bson(value)
    body = b"".join(element(k, v) for k, v in document.items()) + b"\0"
    return struct.pack("<i", len(body) + 4) + body


def cbor(value):
    def head(major, n):
        if n < 24:
            return bytes((major << 5 | n,))
        for size, code in ((1, 24), (2, 25), (4, 26), (8, 27)):
            if n < 1 << (8 * size):
                return bytes((major << 5 | code,)) + n.to_bytes(size, "big")
    if value is True or value is False:
        return bytes((0xF5 if value else 0xF4,))
    if isinstance(value, int):
        return head(0, value) if value >= 0 else head(1, -1 - value)
    if isinstance(value, float):
        return b"\xFB" + struct.pack(">d", value)
    if isinstance(value, str):
        return head(3, len(value.encode())) + value.encode()
    if isinstance(value, list):
        return head(4, len(value)) + b"".join(cbor(v) for v in value)
    return head(5, len(value)) + b"".join(cbor(k) + cbor(v) for k, v in value.items())


def msgpack(value):
    if value is True or value is False:
        return bytes((0xC3 if value else 0xC2,))
    if isinstance(value, int):
        return bytes((value,)) if 0 <= value < 128 else b"\xD2" + struct.pack(">i", value)
    if isinstance(value, float):
        return b"\xCB" + struct.pack(">d", value)
    if isinstance(value, str):
        data = value.encode()
        return (bytes((0xA0 | len(data),)) if len(data) < 32 else b"\xD9" + bytes((len(data),))) + data
    if isinstance(value, list):
        return bytes((0x90 | len(value),)) + b"".join(msgpack(v) for v in value)
    return bytes((0x80 | len(value),)) + b"".join(msgpack(k) + msgpack(v) for k, v in value.items())


def blob_samples():
    """One synthetic value per content type the record viewer recognises."""
    noise = random.Random(812)
    embedding = [noise.uniform(-0.3, 0.3) for _ in range(384)]
    numbers = ",".join(str(noise.randrange(1 << 30)) for _ in range(400))
    large = [[noise.randrange(256) for _ in range(3 * 256)] for _ in range(256)]
    return [
        ("short bytes", bytes((0, 255, 122))),
        ("empty", b""),
        ("png", png(64, 48)),
        ("jpeg", JPEG),
        ("gif", GIF),
        ("webp", WEBP),
        ("bmp", bmp(20, 12)),
        ("ico", ico(32)),
        ("utf-8 text", "Synthetic note λ — stored as a blob.\nSecond line.".encode()),
        ("utf-16 text", "﻿UTF-16 text, Straße λ".encode("utf-16-le")),
        ("uuid", bytes.fromhex("3f2a9c1e7b404d1a9e5c0a1b2c3d4e5f")),
        ("sha-256", hashlib.sha256(b"synthetic").digest()),
        ("jsonb", jsonb(META)),
        ("bson", bson(META)),
        ("binary plist", plistlib.dumps(META, fmt=plistlib.FMT_BINARY)),
        ("cbor", b"\xD9\xD9\xF7" + cbor(META)),
        ("messagepack", msgpack(META)),
        ("protobuf", bytes.fromhex("089601120774657374696e67")),
        ("float32 vector", struct.pack("<384f", *embedding)),
        ("float64 vector", struct.pack("<8d", *embedding[:8]) + b"\x02"),
        ("gzip", gzip.compress(numbers.encode(), mtime=0)),
        ("zlib", zlib.compress(str(META).encode() * 8)),
        ("zstd", ZSTD),
        ("geopackage point", b"GP\0\x01" + struct.pack("<i", 4326) + b"\x01" + struct.pack("<Idd", 1, -6.26, 53.35)),
        ("pdf", b"%PDF-1.7\n%synthetic\n1 0 obj\n<<>>\nendobj\ntrailer\n<<>>\n%%EOF\n"),
        ("random bytes", bytes(noise.randrange(256) for _ in range(300))),
        ("large png", png(256, 256, [bytes(row) for row in large])),
        ("large gzip", gzip.compress(bytes(noise.randrange(256) for _ in range(200_000)), mtime=0)),
        ("null", None),
    ]


path = pathlib.Path(sys.argv[1])
rows = int(sys.argv[2]) if len(sys.argv) > 2 else 10000
if path.exists():
    raise SystemExit(f"Refusing to overwrite {path}")
with sqlite3.connect(path) as c:
    c.execute("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT DEFAULT 'default', data BLOB, number REAL)")
    c.executemany("INSERT INTO items VALUES (?, ?, ?, ?)",
                  ((i, None if i % 5 == 0 else f"row {i} λ; ' quote", bytes([0, 255, i % 256]), i / 10) for i in range(rows)))
    c.execute('CREATE TABLE "composite keys"(a INTEGER NOT NULL, b TEXT NOT NULL, value TEXT, PRIMARY KEY(a,b))')
    c.execute('INSERT INTO "composite keys" VALUES(1,\'key\',\'value\')')
    c.execute("CREATE TABLE keyless(value TEXT)")
    c.execute("INSERT INTO keyless VALUES('duplicate'),('duplicate')")
    c.execute("CREATE VIEW item_names AS SELECT id, name FROM items")
    c.execute("CREATE TABLE blobs(id INTEGER PRIMARY KEY, kind TEXT NOT NULL, data BLOB)")
    c.executemany("INSERT INTO blobs(kind, data) VALUES (?, ?)", blob_samples())
print(f"Created closed fixture {path}: {rows} items; SQLite generator {sqlite3.sqlite_version}")
