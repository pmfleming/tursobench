#!/usr/bin/env python3
"""Refresh checked-in license supplements from immutable published-crate VCS SHAs.
Network maintenance tool, not needed to build/run TursoBench. Review its diff.
"""
import hashlib
import json
import pathlib
import urllib.request

root = pathlib.Path(__file__).resolve().parents[1]
packages = json.loads(__import__('subprocess').check_output(['cargo', 'metadata', '--locked', '--format-version', '1']))['packages']
missing = (root / 'target/package/tursobench-0.1.0-preview-linux-x86_64/docs/third-party/MISSING-LICENSE-TEXTS.txt').read_text().splitlines()
output = root / 'third-party/license-supplements'
output.mkdir(parents=True, exist_ok=True)
cache = {}
index = {}
for package in packages:
    name = f'{package["name"]}-{package["version"]}'
    if name not in missing:
        continue
    vcs_path = pathlib.Path(package['manifest_path']).parent / '.cargo_vcs_info.json'
    repository = package['repository'] or ''
    repo = '/'.join(repository.removeprefix('https://github.com/').split('/')[:2]).removesuffix('.git')
    if not vcs_path.exists() or repo.count('/') != 1:
        continue
    vcs = json.loads(vcs_path.read_text())
    sha = vcs['git']['sha1']
    key = f'{repo}@{sha}'
    try:
        if key not in cache:
            request = urllib.request.Request(f'https://api.github.com/repos/{repo}/git/trees/{sha}?recursive=1', headers={'User-Agent': 'TursoBench-license-review'})
            with urllib.request.urlopen(request, timeout=30) as response:
                tree = json.load(response)['tree']
            entries = []
            for item in tree:
                if item['type'] != 'blob' or len(pathlib.Path(item['path']).parts) > 2 or not any(word in item['path'].lower() for word in ['license', 'licence', 'copying', 'copyright', 'notice']):
                    continue
                url = f'https://raw.githubusercontent.com/{repo}/{sha}/{item["path"]}'
                with urllib.request.urlopen(url, timeout=30) as response:
                    text = response.read()
                relative = pathlib.Path(repo.replace('/', '--') + '-' + sha[:12]) / item['path']
                path = output / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(text)
                entries.append({'file': str(relative), 'url': url, 'sha256': hashlib.sha256(text).hexdigest()})
            cache[key] = entries
        if cache[key]:
            index[name] = cache[key]
    except Exception as error:
        print(f'{name}: {error}')
(output / 'sources.json').write_text(json.dumps(index, indent=2, sort_keys=True) + '\n')
print(f'Supplements for {len(index)}/{len(missing)} missing packages; review third-party/license-supplements')
