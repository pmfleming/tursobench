#!/usr/bin/env bash
# Run real startup discovery against disposable files and an isolated HOME.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
binary=${TURSOBENCH_BINARY:-$root/target/debug/tursobench}
case "$binary" in /*) ;; *) binary="$root/$binary" ;; esac
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
unset WAYLAND_DISPLAY
export LIBGL_ALWAYS_SOFTWARE=1
xvfb-run -a -s '-screen 0 1400x900x24' python3 - "$binary" "$work" <<'PY'
import hashlib
import json
import os
import pathlib
import sqlite3
import subprocess
import sys
import time

binary, work = sys.argv[1], pathlib.Path(sys.argv[2])
home = work / "home"
project = home / "Projects/demo"
data = home / ".local/share"
config = home / ".config"
prefs_path = config / "tursobench/preferences.json"
prefs_path.parent.mkdir(parents=True)
expected = {project / "app.db", data / "demo/state"}
excluded = project / "private"
ignored = {project / "target/ignored.db", data / "chromium/History", excluded / "secret.db",
           data / "captive-portal-chrome/Default/History", data / "unusual-profile-app/Default/History",
           data / "pki/nssdb/key4.db", data / "evolution/addressbook/contacts.db",
           data / "nautilus/tags/meta.db"}
for path in expected | ignored:
    path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(path)
    conn.execute("CREATE TABLE fixture(id INTEGER PRIMARY KEY)")
    conn.execute("INSERT INTO fixture VALUES (1)")
    conn.commit()
    conn.close()
(project / ".gitignore").write_text("*.db\n")
(data / "unusual-profile-app/Local State").write_text("{}")
missing = home / "removed.db"
prefs_path.write_text(json.dumps({"discovery": {
    "enabled": True, "excluded": [str(excluded)],
    "cached": [{"path": str(path), "size": 100, "modified": None}
               for path in [missing, data / "unusual-profile-app/Default/History", data / "pki/nssdb/key4.db"]]
}}))
environment = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(config),
                   XDG_DATA_HOME=str(data), XDG_CACHE_HOME=str(home / ".cache"))

def snapshot():
    return {str(p.relative_to(home)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in home.rglob("*") if p.is_file() and not p.is_relative_to(config)
            and not p.is_relative_to(home / ".cache")}

def run_until(check, seconds):
    # The CLI path must stay a selection: discovery must never auto-open it.
    process = subprocess.Popen([binary, str(project / "app.db")], cwd=project,
                               env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    try:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise AssertionError(f"desktop exited early ({process.returncode})")
            if check():
                return
            time.sleep(0.05)
        raise AssertionError("startup discovery did not produce the expected saved results")
    finally:
        if process.poll() is None:
            process.terminate()
        try:
            output, _ = process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            output, _ = process.communicate()
        if process.returncode not in (-15, 0):
            print(output.decode(errors="replace"), file=sys.stderr)

def discovered():
    prefs = json.loads(prefs_path.read_text())
    cached = {pathlib.Path(e["path"]) for e in prefs["discovery"].get("cached", [])}
    if not expected.issubset(cached):
        return False
    assert not cached & ignored, cached
    assert missing not in cached
    assert not prefs["discovery"]["recent"], "startup must not open a database"
    return True

before = snapshot()
run_until(discovered, 15)
assert snapshot() == before, "discovery changed a fixture or created a sidecar"

# A persisted opt-out must suppress actual startup traversal and cache writes.
prefs = json.loads(prefs_path.read_text())
prefs["discovery"].update(enabled=False, cached=[])
prefs_path.write_text(json.dumps(prefs))
start = time.monotonic()
def disabled():
    assert not json.loads(prefs_path.read_text())["discovery"]["cached"]
    return time.monotonic() - start > 3
run_until(disabled, 15)
assert snapshot() == before
print("STARTUP DISCOVERY SMOKE OK: projects, extensionless app data, exclusions, stale cache, opt-out, no writes")
PY
