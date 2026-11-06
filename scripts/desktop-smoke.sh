#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
python3 tests/fixtures/generate.py "$work/smoke.db" 100000
# Exercise retained sessions and Ctrl+PageUp/PageDown database cycling.
python3 tests/fixtures/generate.py "$work/second.db" 100
export TURSOBENCH_SECOND_DB="$work/second.db"
export XDG_CONFIG_HOME="$work/config"
binary=${TURSOBENCH_BINARY:-target/debug/tursobench}
python3 scripts/wayland-run.py -- timeout 30 "$binary" --smoke-test "$work/smoke.db"
