"""Check private Vite source-map staging without executing or publishing the build."""
import argparse
import hashlib
import json
from pathlib import Path
import re

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--surface', choices=('desktop', 'marketing'), required=True)
parser.add_argument('--build-root', type=Path, required=True)
parser.add_argument('--disabled-build-root', type=Path)
parser.add_argument('--release', required=True)
parser.add_argument('--dsn', required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
if args.disabled_build_root:
    disabled_root = args.disabled_build_root.resolve(strict=True)
    assert (disabled_root / 'index.html').is_file(), 'Missing disabled entry page'
    disabled_assets = sorted(disabled_root.rglob('*.js'))
    assert disabled_assets, 'No disabled JavaScript'
    for asset in disabled_assets:
        script = asset.read_text()
        for marker in ('__SENTRY__', 'sentry.javascript', 'sentry.io', f'{args.surface}-telemetry'):
            assert marker not in asset.name and marker not in script, ('Telemetry in disabled build', asset, marker)
root = args.build_root.resolve(strict=True)
assert (root / 'index.html').is_file(), 'Missing built entry page'
assets = sorted(root.rglob('*.js'))
assert assets, 'No built JavaScript'
reports = []
unmapped = []
reexports = []
sources = set()
release_assets = []
dsn_assets = []
for asset in assets:
    script = asset.read_text()
    assert not re.search(r'(?m)^\s*//[#@]\s*sourceMappingURL\s*=', script), asset
    assert not re.search(r'/\*[#@]\s*sourceMappingURL\s*=', script), asset
    relative = asset.relative_to(root).as_posix()
    if args.release in script:
        release_assets.append(relative)
    if args.dsn in script:
        dsn_assets.append(relative)
    report = {'asset': relative, 'bytes': asset.stat().st_size,
              'sha256': hashlib.sha256(asset.read_bytes()).hexdigest()}
    mapping = asset.with_name(asset.name + '.map')
    if mapping.exists():
        data = json.loads(mapping.read_text())
        assert data.get('version') == 3, mapping
        assert data.get('sources') and isinstance(data.get('mappings'), str), mapping
        assert len(data['sources']) == len(data.get('sourcesContent', [])), mapping
        assert all(isinstance(content, str) for content in data['sourcesContent']), mapping
        assert data.get('file') == asset.name, (mapping, data.get('file'))
        sources.update(data['sources'])
        report['map'] = {'file': mapping.relative_to(root).as_posix(),
                         'bytes': mapping.stat().st_size,
                         'sha256': hashlib.sha256(mapping.read_bytes()).hexdigest(),
                         'sources': len(data['sources'])}
    else:
        if re.fullmatch(r'rolldown-runtime-[A-Za-z0-9_-]+\.js', asset.name):
            unmapped.append(relative)
        else:
            shim = re.fullmatch(
                r'import\{[A-Za-z_$][\w$]* as (?P<local>[A-Za-z_$][\w$]*)\}from"(?P<target>\./[A-Za-z0-9_-]+\.js)";export\{(?P=local) as default\};\s*',
                script,
            )
            assert shim, ('Unmapped executable asset', asset)
            target = asset.parent / shim['target']
            assert target.is_file() and target.with_name(target.name + '.map').is_file(), target
            reexports.append({'asset': relative, 'mapped_target': target.relative_to(root).as_posix()})
    reports.append(report)
for mapping in root.rglob('*.js.map'):
    assert mapping.with_suffix('').is_file(), ('Map without asset', mapping)
assert release_assets, 'Expected release is absent from JavaScript'
assert dsn_assets, 'Expected enabled DSN is absent from JavaScript'
assert any(f'{args.surface}-instrument.ts' in source for source in sources), 'Missing instrumentation source'
assert any(f'{args.surface}-telemetry.ts' in source for source in sources), 'Missing telemetry source'
assert any('@sentry/' in source for source in sources), 'Missing Sentry SDK source maps'
result = {'surface': args.surface, 'release': args.release,
          'disabled_build_checked': args.disabled_build_root is not None,
          'release_assets': release_assets, 'enabled_dsn_assets': dsn_assets,
          'javascript_files': len(assets),
          'javascript_bytes': sum(report['bytes'] for report in reports),
          'map_files': sum('map' in report for report in reports),
          'map_bytes': sum(report.get('map', {}).get('bytes', 0) for report in reports),
          'unmapped_generated_runtime': unmapped, 'generated_reexports': reexports,
          'unique_source_files': len(sources), 'assets': reports}
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps({key: value for key, value in result.items() if key not in ('assets', 'generated_reexports')}, indent=2))
