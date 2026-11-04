#!/usr/bin/env bash
# Current README: native Wayland screenshots of a closed local database copy.
set -euo pipefail
cd "$(dirname "$0")/.."
source_db=${1:?Usage: capture-readme.sh CLOSED_SESSION_DB [OUTPUT_DIR]}
output=$(realpath -m "${2:-docs/assets/ui/current}")
work=$(mktemp -d /tmp/tursobench-readme.XXXXXX)
trap 'rm -rf "$work"' EXIT
desktop_config=${XDG_CONFIG_HOME:-${HOME}/.config}
for version in gtk-3.0 gtk-4.0; do
  mkdir -p "$work/config/$version"
  for theme_file in gtk.css settings.ini; do
    if [ -f "$desktop_config/$version/$theme_file" ]; then
      cp "$desktop_config/$version/$theme_file" "$work/config/$version/$theme_file"
    fi
  done
done
python3 - "$source_db" "$work/session.db" <<'PYDB'
import hashlib
from pathlib import Path
import shutil
import sys

source, destination = map(Path, sys.argv[1:])
wal = Path(str(source) + '-wal')
if wal.exists() and wal.stat().st_size:
    raise SystemExit('Use a closed, checkpointed offline snapshot; the supplied copy has WAL data.')
before = hashlib.sha256(source.read_bytes()).hexdigest()
shutil.copyfile(source, destination)
assert hashlib.sha256(destination.read_bytes()).hexdigest() == before
assert hashlib.sha256(source.read_bytes()).hexdigest() == before
print(f'Offline capture source SHA-256: {before}')
PYDB
cargo build --locked -p tursobench-ui
export XDG_CONFIG_HOME="$work/config"
export TURSOBENCH_CAPTURE_DIR="$output"
export TURSOBENCH_README_CAPTURE=1
python3 scripts/wayland-run.py --width 1920 --height 1120 -- timeout 75 \
  target/debug/tursobench --smoke-test "$work/session.db" document_history
printf 'Current README captures saved to %s\n' "$output"
