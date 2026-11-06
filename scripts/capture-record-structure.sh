#!/usr/bin/env bash
# Two real Wayland capture passes: default columns and compact foreign-key map.
set -euo pipefail
cd "$(dirname "$0")/.."
output=$(realpath -m "${1:-target/record-structure-captures}")
bash scripts/capture-v8.sh "$output"
TURSOBENCH_CAPTURE_STRUCTURE_SECTION=foreign-keys bash scripts/capture-v8.sh "$output/foreign-keys"
