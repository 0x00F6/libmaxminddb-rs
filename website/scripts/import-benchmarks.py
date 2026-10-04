"""Import the exact labels exported by make bench-compare; never infer missing points."""
import argparse
import hashlib
import json
import re
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SITE = ROOT / 'website'
parser = argparse.ArgumentParser()
parser.add_argument('--runs', type=Path, default=SITE / 'benchmark-runs.json')
args = parser.parse_args()
runs = json.loads(args.runs.read_text())
output = SITE / 'public/data'
output.mkdir(parents=True, exist_ok=True)
manifest = []


def import_run(run):
    charts_dir = SITE / run['charts']
    context = json.loads((SITE / run['context']).read_text())
    if context['architecture'] != run['id']:
        raise ValueError('Architecture does not match the selected run')
    # Never mix provenance from one machine with another machine's measurements.
    context_digest = hashlib.sha256(b''.join(p.read_bytes() for p in sorted(charts_dir.glob('*.svg')))).hexdigest()
    if context_digest != context['chartsSha256']:
        raise SystemExit(f"Chart inputs changed for {run['id']}. Review its context before updating chartsSha256: {context_digest}")
    NS = '{http://www.w3.org/2000/svg}'
    charts = []
    files = [f'throughput-{family}-{workload}.svg' for family in ['ipv4','ipv6'] for workload in ['random','hot','sequential','absent']]
    files += ['candlestick-percentiles-ipv4.svg', 'candlestick-percentiles-ipv6.svg', 'database-size-scaling.svg', 'memory-rss-after-open.svg', 'memory-rss-peak.svg', 'writer-throughput.svg', 'writer-build-time.svg', 'writer-peak-rss.svg', 'writer-p99.svg', 'concurrent-throughput.svg', 'concurrent-throughput-ipv6.svg']
    for filename in files:
        path = charts_dir / filename
        root = ET.parse(path).getroot()
        title = next(t.text for t in root.iter(NS+'text') if t.attrib.get('class') == 'chart-title')
        subtitle = next(t.text for t in root.iter(NS+'text') if t.attrib.get('class') == 'chart-subtitle')
        points = []
        for element in root.iter(NS+'title'):
            raw = element.text or ''
            if filename.startswith('candlestick-'):
                # The SVG repeats each tooltip with a rank suffix. Keep one record
                # per library and retain all exported quantiles, including Max.
                if re.match(r'^.+ \(Rank #\d+\):', raw):
                    continue
                match = re.fullmatch(r'(.+?): Min=(.+) \| p50=(.+) \| p95=(.+) \| p99=(.+) \| Max=(.+)', raw)
                if not match:
                    raise ValueError(f'Unsupported percentile label in {filename}: {raw}')
                library, *labels = match.groups()
                quantiles = {}
                for key, label in zip(['min', 'p50', 'p95', 'p99', 'max'], labels):
                    value = re.fullmatch(r'([\d.]+) (ns|µs|ms|s)', label)
                    if not value:
                        raise ValueError(f'Unsupported latency in {filename}: {label}')
                    number, unit = value.groups()
                    quantiles[key] = float(number) * {'ns': 1, 'µs': 1000, 'ms': 1000000, 's': 1000000000}[unit]
                if list(quantiles.values()) != sorted(quantiles.values()):
                    raise ValueError(f'Unordered quantiles in {filename}: {raw}')
                if any(p['library'] == library for p in points):
                    raise ValueError(f'Duplicate library in {filename}: {library}')
                points.append(dict(library=library, value=quantiles['p99'], unit='ns',
                    dimension=None, label=raw, quantiles=quantiles))
                continue
            match = re.match(r'^(.+?): ([\d.]+) (M ops/s|K ops/s|ops/s|ns|µs|ms|s|MiB)(?: \(\d+ bytes\))?(?: @ (.+)| \(.+\))?$', raw)
            if not match:
                raise ValueError(f'Unsupported measurement label in {filename}: {raw}')
            library, number, unit, dimension = match.groups()
            value = float(number)
            if unit in ('ns','µs','ms'):
                value *= {'ns': 1, 'µs': 1000, 'ms': 1000000}[unit]; unit = 'ns'
            elif unit in ('K ops/s','ops/s'):
                value /= {'K ops/s': 1000, 'ops/s': 1000000}[unit]; unit = 'M ops/s'
            # The historical 1T concurrent export includes fallback single-thread
            # scenarios; do not turn it into a same-workload speedup comparison.
            if filename.startswith('concurrent-') and dimension == '1T':
                continue
            points.append(dict(library=library, value=value, unit=unit, dimension=dimension, label=raw))
        charts.append(dict(id=path.stem, title=title, subtitle=subtitle, points=points,
            source=context['sourceExport'].replace('/tree/', '/blob/', 1) + '/' + filename,
            sha256=hashlib.sha256(path.read_bytes()).hexdigest()))
    (output/run['output']).write_text(json.dumps(dict(context=context, charts=charts), indent=2)+'\n')
    manifest.append(dict(id=run['id'], label=f"{run['id']} · {context['cpu']}", file=run['output']))
    print(f"Imported {run['id']}: {len(charts)} charts, {sum(len(c['points']) for c in charts)} measured points.")

for run in runs:
    import_run(run)
(output/'benchmark-datasets.json').write_text(json.dumps(manifest, indent=2)+'\n')

examples = []
for name, label, description in [('quickstart','Read & write','Create a database and decode typed borrowed records.'), ('editor_merge','Hot updates','Stage a typed DeepMerge update, publish with ArcSwap and release the old database.'), ('concurrent_editor','Concurrent access','Read from multiple threads while a writer publishes new generations.')]:
    examples.append(dict(id=name, label=label, description=description, code=(ROOT/f'examples/{name}.rs').read_text(), source=f'https://github.com/0x00F6/libmaxminddb-rs/blob/feature/github-pages/examples/{name}.rs'))
(output/'examples.json').write_text(json.dumps(examples, indent=2)+'\n')
print(f'Imported {len(examples)} source examples; no benchmarks executed.')
