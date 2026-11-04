#!/usr/bin/env bash
# Run real startup discovery against disposable files and an isolated HOME.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
binary=${TURSOBENCH_BINARY:-$root/target/debug/tursobench}
case "$binary" in /*) ;; *) binary="$root/$binary" ;; esac
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$root"
cargo build --locked -p tursobench-db --example preferences
preferences_tool="$root/target/debug/examples/preferences"
python3 scripts/wayland-run.py -- python3 - "$binary" "$work" "$preferences_tool" <<'PY'
import hashlib
import json
import os
import pathlib
import sqlite3
import subprocess
import sys
import time

binary, work, preferences_tool = sys.argv[1], pathlib.Path(sys.argv[2]), sys.argv[3]
home = work / "home"
project = home / "Projects/demo"
data = home / ".local/share"
state = home / ".local/state"
config = home / ".config"
prefs_path = config / "tursobench/preferences.db"
seed_path = work / "seed.json"
def write_preferences(prefs):
    seed_path.write_text(json.dumps(prefs))
    subprocess.run([preferences_tool, str(prefs_path), str(seed_path)], check=True)
def read_preferences():
    return json.loads(subprocess.check_output([preferences_tool, str(prefs_path)], text=True))
prefs_path.parent.mkdir(parents=True)
expected = {project / "app.db", data / "demo/state",
            data / "clip-daemon/history/history.db",
            state / "app-daemon/resource-history-turso-v1.db",
            state / "bar-daemon/notifications-turso-v1.db",
            state / "bar-daemon/agenda/agenda.db"}
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
write_preferences({"discovery": {
    "enabled": True, "excluded": [str(excluded)],
    "cached": [{"path": str(path), "size": 100, "modified": None}
               for path in [missing, data / "unusual-profile-app/Default/History", data / "pki/nssdb/key4.db"]]
}})
environment = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(config),
                   XDG_DATA_HOME=str(data), XDG_STATE_HOME=str(state),
                   XDG_CACHE_HOME=str(home / ".cache"))

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
    prefs = read_preferences()
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

# Removing all listed default folders must survive restart while discovery is on.
prefs = read_preferences()
defaults = [data, state, project, home]
defaults += [home / name for name in ("Projects", "projects", "Code", "code", "Developer", "Development", "src")]
prefs["discovery"].update(removed_roots=[str(path) for path in defaults], cached=[])
write_preferences(prefs)
start = time.monotonic()
def no_cached_results():
    assert not read_preferences()["discovery"]["cached"]
    return time.monotonic() - start > 3
run_until(no_cached_results, 15)
assert snapshot() == before

# A persisted opt-out must suppress actual startup traversal and cache writes.
prefs = read_preferences()
prefs["discovery"].update(enabled=False, cached=[], removed_roots=[])
write_preferences(prefs)
start = time.monotonic()
run_until(no_cached_results, 15)
assert snapshot() == before
print("STARTUP DISCOVERY SMOKE OK: projects, extensionless app data, exclusions, stale cache, removed defaults, opt-out, no writes")
PY
