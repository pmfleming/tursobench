#!/usr/bin/env python3
"""Closed-file compatibility fixture, generated with Python's stdlib SQLite.
No SQLite runtime/adapter is linked into TursoBench. Own fixture, no copied data.
Refuses to overwrite an existing file. Use only in disposable test directories.
"""
import pathlib
import sqlite3
import sys

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
print(f"Created closed fixture {path}: {rows} items; SQLite generator {sqlite3.sqlite_version}")
