#!/usr/bin/env bash
# Native Wayland captures of the record tile reading blobs: the field rows,
# previews in place and the value view at 1800x1000 and 1366x768, from the
# synthetic blob tables of tests/fixtures/generate.py.
set -euo pipefail
cd "$(dirname "$0")/.."
output=$(realpath -m "${1:-docs/assets/ui/blob-viewer}")
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
python3 tests/fixtures/generate.py "$work/blobs.db" 100
if [ -z "${TURSOBENCH_BINARY:-}" ]; then
  cargo build --locked -p tursobench-ui
fi
export XDG_CONFIG_HOME="$work/config"
export TURSOBENCH_CAPTURE_DIR="$output"
export TURSOBENCH_BLOB_CAPTURE=1
binary=${TURSOBENCH_BINARY:-target/debug/tursobench}
python3 scripts/wayland-run.py --width 1880 --height 1080 -- timeout 120 "$binary" --smoke-test "$work/blobs.db" blobs
python3 - "$output" <<'PY'
from pathlib import Path
import struct
import sys

root = Path(sys.argv[1])
# A compositor that forces fullscreen can produce valid PNGs at the wrong size.
for image in sorted(root.glob("blob-*.png")):
    expected = (1366, 768) if image.name.endswith("-laptop.png") else (1800, 1000)
    dimensions = struct.unpack(">II", image.read_bytes()[16:24])
    assert dimensions == expected, (image.name, dimensions, expected)
print("Wayland capture dimensions verified")
PY
printf 'Blob viewer captures saved to %s\n' "$output"
