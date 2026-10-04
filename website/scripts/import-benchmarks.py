"""Import the exact labels exported by make bench-compare; never infer missing points."""
import argparse
import hashlib
import json
import re
import subprocess
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SITE = ROOT / 'website'
parser = argparse.ArgumentParser()
parser.add_argument('--charts', type=Path, default=ROOT / 'benchmarks/charts')
parser.add_argument('--context', type=Path, default=SITE / 'benchmark-context.json')
args = parser.parse_args()
context = json.loads(args.context.read_text())
# Metadata must be deliberately refreshed when importing a new measurement set.
context_digest = hashlib.sha256(b''.join(p.read_bytes() for p in sorted(args.charts.glob('*.svg')))).hexdigest()
if context_digest != context['chartsSha256']:
    raise SystemExit('Chart inputs changed. Review benchmark-context.json (machine, versions, provenance) and update chartsSha256: ' + context_digest)
NS = '{http://www.w3.org/2000/svg}'
charts = []
files = [f'throughput-{family}-{workload}.svg' for family in ['ipv4','ipv6'] for workload in ['random','hot','sequential','absent']]
files += ['lookup-latency-ipv4.svg', 'lookup-latency-ipv6.svg', 'database-size-scaling.svg', 'memory-rss-after-open.svg', 'memory-rss-peak.svg', 'writer-throughput.svg', 'writer-build-time.svg', 'writer-peak-rss.svg', 'writer-p99.svg', 'concurrent-throughput.svg', 'concurrent-throughput-ipv6.svg']
for filename in files:
    path = args.charts / filename
    root = ET.parse(path).getroot()
    title = next(t.text for t in root.iter(NS+'text') if t.attrib.get('class') == 'chart-title')
    subtitle = next(t.text for t in root.iter(NS+'text') if t.attrib.get('class') == 'chart-subtitle')
    points = []
    for element in root.iter(NS+'title'):
        raw = element.text or ''
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
        source=f"https://github.com/0x00F6/libmaxminddb-rs/blob/{context['sourceCommit']}/benchmarks/charts/{filename}",
        sha256=hashlib.sha256(path.read_bytes()).hexdigest()))
output = SITE / 'public/data'
output.mkdir(parents=True, exist_ok=True)
(output/'benchmarks.json').write_text(json.dumps(dict(context=context, charts=charts), indent=2)+'\n')
examples = []
for name, label, description in [('quickstart','Read & write','Create a database and decode typed borrowed records.'), ('editor_merge','Hot updates','Stage a typed DeepMerge update, publish with ArcSwap and release the old database.'), ('concurrent_editor','Concurrent access','Read from multiple threads while a writer publishes new generations.')]:
    examples.append(dict(id=name, label=label, description=description, code=(ROOT/f'examples/{name}.rs').read_text(), source=f'https://github.com/0x00F6/libmaxminddb-rs/blob/feature/github-pages/examples/{name}.rs'))
(output/'examples.json').write_text(json.dumps(examples, indent=2)+'\n')
print(f'Imported {len(charts)} comparative charts and {sum(len(c["points"]) for c in charts)} measured points; no benchmarks executed.')
