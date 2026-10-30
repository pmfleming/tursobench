#!/usr/bin/env bash
# Source-derived schemas and synthetic data only. Never opens live project stores.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test --locked -p tursobench-db --test local_projects -- --nocapture
cargo build --locked -p tursobench-ui
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cargo run --locked -p tursobench-db --example local_project_fixtures -- "$work/fixtures"
export XDG_CONFIG_HOME="$work/config"
unset WAYLAND_DISPLAY
export LIBGL_ALWAYS_SOFTWARE=1
binary=${TURSOBENCH_BINARY:-target/debug/tursobench}
for pair in scratchpad:buffer_ids agenda:items notifications:notifications clipboard:search_content; do
  name=${pair%%:*}
  table=${pair#*:}
  echo "Desktop smoke: $name / $table"
  xvfb-run -a -s '-screen 0 1400x900x24' timeout 30 "$binary" --smoke-test "$work/fixtures/$name.db" "$table"
done
