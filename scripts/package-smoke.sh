#!/usr/bin/env bash
# Validate a locally built, trusted preview archive in a disposable prefix.
set -euo pipefail
cd "$(dirname "$0")/.."
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
(cd target && sha256sum -c tursobench-0.1.0-preview-linux-x86_64.tar.gz.sha256)
tar -xzf target/tursobench-0.1.0-preview-linux-x86_64.tar.gz -C "$work"
package="$work/tursobench-0.1.0-preview-linux-x86_64"
bash "$package/install.sh" "$work/installed"
cmp README.md "$work/installed/share/doc/tursobench/README.md"
cmp docs/review-remediation.md "$work/installed/share/doc/tursobench/docs/review-remediation.md"
test -f "$work/installed/share/applications/tursobench.desktop"
test -f "$work/installed/share/icons/hicolor/512x512/apps/tursobench.png"
test -f "$work/installed/share/doc/tursobench/third-party/texts/turso-0.8.3/0-LICENSE.md"
cmp vendor/turso/PATCH.md "$work/installed/share/doc/tursobench/third-party/turso/PATCH.md"
TURSOBENCH_BINARY="$work/installed/bin/tursobench" bash scripts/desktop-smoke.sh
printf 'Installed release archive smoke passed.\n'
