#!/usr/bin/env bash
# Native assistant-panel captures: synthetic data, isolated preferences and a
# seeded conversation. No agent CLI runs and nothing leaves the machine.
set -euo pipefail
cd "$(dirname "$0")/.."
output=$(realpath -m "${1:-docs/assets/ui/assistant}")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
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
            project_id INTEGER REFERENCES projects(id), priority INTEGER, progress INTEGER);
        CREATE INDEX items_project_status ON items(project_id,status);
    ''')
    db.executemany('INSERT INTO items VALUES(?,?,?,?,?,?)', (
        (i, f"row {i}", ['done','in progress','pending','blocked'][i%4], i%3+1, i%6, i%101)
        for i in range(1000)
    ))
PY
cargo build --locked -p tursobench-ui
export XDG_CONFIG_HOME="$work/config"
export TURSOBENCH_CAPTURE_DIR="$output"
export TURSOBENCH_ASSISTANT_CAPTURE=1
binary=${TURSOBENCH_BINARY:-target/debug/tursobench}
python3 scripts/wayland-run.py --width 1440 --height 840 -- timeout 75 "$binary" --smoke-test "$work/sample.db"
for name in assistant-consent assistant-conversation assistant-without-sidebar \
    assistant-light panels-hidden assistant-settings; do
  test -s "$output/$name.png" || { echo "missing $name.png" >&2; exit 1; }
done
echo "Assistant captures written to $output"
