#!/usr/bin/env bash
# Native v8 captures using synthetic data and the invoking desktop's GTK colours.
set -euo pipefail
cd "$(dirname "$0")/.."
output=$(realpath -m "${1:-docs/assets/ui/v8}")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# Copy only theme inputs; never use or modify the user's application preferences.
desktop_config=${XDG_CONFIG_HOME:-${HOME}/.config}
for version in gtk-3.0 gtk-4.0; do
  mkdir -p "$work/config/$version"
  for theme_file in gtk.css settings.ini; do
    if [ -f "$desktop_config/$version/$theme_file" ]; then
      cp "$desktop_config/$version/$theme_file" "$work/config/$version/$theme_file"
    fi
  done
done
python3 - "$work/sample.db" <<'PY'
import sqlite3
import sys
with sqlite3.connect(sys.argv[1]) as db:
    db.executescript('''
        CREATE TABLE accounts(id INTEGER PRIMARY KEY, name TEXT NOT NULL);
        INSERT INTO accounts VALUES(1,'North team'),(2,'South team');
        CREATE TABLE projects(id INTEGER PRIMARY KEY, title TEXT, account_id INTEGER REFERENCES accounts(id));
        INSERT INTO projects VALUES(1,'Launch',1),(2,'Workbench',1),(3,'Search',2);
        CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT, status TEXT,
            project_id INTEGER REFERENCES projects(id), priority INTEGER, progress INTEGER,
            data BLOB, number REAL);
        CREATE INDEX items_project_status ON items(project_id,status);
        CREATE VIEW item_names AS SELECT id,name FROM items;
    ''')
    db.executemany('INSERT INTO items VALUES(?,?,?,?,?,?,?,?)', (
        (i, None if i % 5 == 0 else f"row {i} λ; ' quote",
         ['done','in progress','pending','blocked'][i%4], i%3+1, i%6, i%101,
         bytes([0,255,i%256]), i/10) for i in range(1000)
    ))
PY
cargo build --locked -p tursobench-ui
export XDG_CONFIG_HOME="$work/config"
export TURSOBENCH_CAPTURE_DIR="$output"
# Leave room for Wayland decorations around the largest 3440x1440 client scene.
python3 scripts/wayland-run.py --width 3520 --height 1520 -- timeout 75 target/debug/tursobench --smoke-test "$work/sample.db"
python3 - "$output" <<'PY'
from pathlib import Path
import struct
import sys

root = Path(sys.argv[1])
# A compositor that forces fullscreen can produce valid PNGs at the wrong size.
for name, expected in {
    "workspace-dark.png": (1800, 1000),
    "laptop-light.png": (1366, 768),
    "laptop-dark.png": (1366, 768),
    "widescreen.png": (3440, 1440),
}.items():
    dimensions = struct.unpack(">II", (root / name).read_bytes()[16:24])
    assert dimensions == expected, (name, dimensions, expected)
print("Wayland capture dimensions verified")
PY
printf 'Native captures saved to %s\n' "$output"
