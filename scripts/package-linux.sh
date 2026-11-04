#!/usr/bin/env bash
# Preview packaging. Run on the intended Linux distribution; Nix linkage is not portable.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ $(uname -s) != Linux || $(uname -m) != x86_64 ]]; then
  echo 'This preview packager supports Linux x86_64 only.' >&2
  exit 1
fi
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}
export CARGO_INCREMENTAL=0
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$(pwd)=tursobench"
cargo build --locked --release -p tursobench-ui
name=tursobench-0.1.0-preview-linux-x86_64
stage=target/package/$name
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/docs" "$stage/share/applications" "$stage/share/icons/hicolor/scalable/apps" "$stage/share/icons/hicolor/512x512/apps"
install -m755 target/release/tursobench "$stage/bin/tursobench"
install -m755 packaging/install.sh "$stage/install.sh"
cp packaging/tursobench.desktop "$stage/share/applications/"
cp packaging/tursobench.svg "$stage/share/icons/hicolor/scalable/apps/"
cp packaging/tursobench.png "$stage/share/icons/hicolor/512x512/apps/"
cp README.md LICENSE CONTRIBUTING.md CODE_OF_CONDUCT.md SECURITY.md Cargo.lock rust-toolchain.toml "$stage/docs/"
cp -R docs contracts "$stage/docs/"
python3 scripts/license-notices.py "$stage/docs/third-party"
cp third-party/README.md "$stage/docs/third-party/"
cp vendor/README.md "$stage/docs/third-party/VENDOR-PATCHES.md"
mkdir -p "$stage/docs/third-party/turso"
cp vendor/turso/PATCH.md "$stage/docs/third-party/turso/PATCH.md"
# Fixed ordering, owner and timestamps make the archive deterministic for the same binary.
tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner -C target/package -cf - "$name" | gzip -n > "target/$name.tar.gz"
(cd target && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
printf 'Preview package: target/%s.tar.gz\n' "$name"
