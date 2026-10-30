#!/usr/bin/env python3
"""Collect dependency license metadata and verbatim bundled texts for a preview.
Review MISSING-LICENSE-TEXTS.txt before any public release; SPDX is not a notice.
"""
import hashlib
import json
import pathlib
import subprocess
import sys

out = pathlib.Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
metadata = json.loads(subprocess.check_output([
    "cargo", "metadata", "--locked", "--format-version", "1",
    "--filter-platform", "x86_64-unknown-linux-gnu",
]))
resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
supplement_root = pathlib.Path(__file__).resolve().parents[1] / "third-party/license-supplements"
supplements = json.loads((supplement_root / "sources.json").read_text())
missing = []
inventory = ["# Third-party dependency inventory", "", "Generated from Cargo.lock. Includes some build/test dependencies.", ""]
for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
    if package["id"] not in resolved or package["id"] in metadata["workspace_members"]:
        continue
    name = f'{package["name"]}-{package["version"]}'
    inventory.append(f'- {name}: {package["license"] or "SEE LICENSE FILE"}; {package["repository"] or ""}')
    root = pathlib.Path(package["manifest_path"]).parent
    texts = []
    for path in root.rglob("*"):
        relative = path.relative_to(root)
        if len(relative.parts) > 4 or not path.is_file():
            continue
        if any(word in path.name.lower() for word in ("license", "licence", "copying", "notice", "copyright")) or ("fonts" in relative.parts and path.suffix == ".txt"):
            texts.append(path)
    if package.get("license_file"):
        path = root / package["license_file"]
        if path.is_file() and path not in texts:
            texts.append(path)
    extra = supplements.get(name, [])
    if not texts and not extra:
        missing.append(name)
    else:
        target = out / "texts" / name
        target.mkdir(parents=True, exist_ok=True)
        for index, path in enumerate(sorted(texts)):
            (target / f"{index}-{path.name}").write_bytes(path.read_bytes())
        for index, entry in enumerate(extra):
            text = (supplement_root / entry['file']).read_bytes()
            assert hashlib.sha256(text).hexdigest() == entry['sha256'], entry['file']
            (target / f"upstream-{index}-{pathlib.Path(entry['file']).name}").write_bytes(text)
        if extra:
            (target / 'SOURCES.json').write_text(json.dumps(extra, indent=2) + '\n')
(out / "INVENTORY.md").write_text("\n".join(inventory) + "\n")
(out / "MISSING-LICENSE-TEXTS.txt").write_text("\n".join(missing) + ("\n" if missing else ""))
print(f"Collected dependency notices; {len(missing)} packages need manual license-text review", file=sys.stderr)
