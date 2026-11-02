#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")" && pwd)
prefix=${1:-"$HOME/.local"}
install -Dm755 "$root/bin/tursobench" "$prefix/bin/tursobench"
install -Dm644 "$root/share/applications/tursobench.desktop" "$prefix/share/applications/tursobench.desktop"
install -Dm644 "$root/share/icons/hicolor/scalable/apps/tursobench.svg" "$prefix/share/icons/hicolor/scalable/apps/tursobench.svg"
install -Dm644 "$root/share/icons/hicolor/512x512/apps/tursobench.png" "$prefix/share/icons/hicolor/512x512/apps/tursobench.png"
mkdir -p "$prefix/share/doc/tursobench"
cp -R "$root/docs/." "$prefix/share/doc/tursobench/"
printf 'Installed to %s; ensure %s/bin is on PATH.\n' "$prefix" "$prefix"
