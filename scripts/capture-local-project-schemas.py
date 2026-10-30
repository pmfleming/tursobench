#!/usr/bin/env python3
"""Capture SQL constants and MIT notices, never databases or application state.
Run inside the Nix dev shell. Review the fixture/provenance diff after refreshing.
"""
import argparse
import hashlib
import json
import pathlib
import re
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("projects", type=pathlib.Path, help="directory containing scratchpad, clip-daemon and bar-daemon")
args = parser.parse_args()
output = pathlib.Path(__file__).resolve().parents[1] / "tests/fixtures/local-projects"
output.mkdir(parents=True, exist_ok=True)
session = "src/app/services/session_store/database"
fixtures = {
    "scratchpad": ("scratchpad", [f"{session}/{name}.rs" for name in ["mod", "quarantine", "closed", "versions"]]),
    "clipboard": ("clip-daemon", ["src/history_store/schema.rs"]),
    "agenda": ("bar-daemon", ["src/activity/agenda_store/database.rs"]),
    "notifications": ("bar-daemon", ["src/activity/notifications/database.rs"]),
}
manifest = {}
for name, (project, files) in fixtures.items():
    root = args.projects / project
    revision = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    sources, fragments = [], []
    for relative in files:
        source = (root / relative).read_bytes()
        text = source.decode("utf-8")
        match = re.search(r'const SCHEMA:\s*&str\s*=\s*(?:r#"(.*?)"#|"([^"\\]*)")\s*;', text, re.DOTALL)
        if not match:
            raise SystemExit(f"Unsupported SCHEMA literal in {root / relative}; review manually")
        fragments.append(next(group for group in match.groups() if group is not None).strip())
        dirty = bool(subprocess.check_output(["git", "-C", str(root), "status", "--porcelain", "--", relative], text=True).strip())
        sources.append({"file": relative, "sha256": hashlib.sha256(source).hexdigest(), "working_tree_modified": dirty})
    sql = "\n\n".join(fragments) + "\n"
    (output / f"{name}.sql").write_text(sql)
    license_name = f"LICENSE-{project}"
    (output / license_name).write_bytes((root / "LICENSE").read_bytes())
    manifest[name] = {"project": project, "revision": revision, "sources": sources,
                      "schema_sha256": hashlib.sha256(sql.encode()).hexdigest(), "license": license_name}
(output / "provenance.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
print(f"Captured {len(fixtures)} source-only schema fixtures in {output}")
