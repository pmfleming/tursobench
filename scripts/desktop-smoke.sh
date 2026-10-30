#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
python3 tests/fixtures/generate.py "$work/smoke.db" 100000
export XDG_CONFIG_HOME="$work/config"
# Select X11 so this is independent of the invoking user's Wayland session.
unset WAYLAND_DISPLAY
export LIBGL_ALWAYS_SOFTWARE=1
binary=${TURSOBENCH_BINARY:-target/debug/tursobench}
xvfb-run -a -s '-screen 0 1400x900x24' timeout 30 "$binary" --smoke-test "$work/smoke.db"
