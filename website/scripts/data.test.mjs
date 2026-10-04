import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const data=JSON.parse(readFileSync(new URL('../public/data/benchmarks.json',import.meta.url)));
test('every displayed measurement has a real source and a finite nonnegative value',()=>{
  assert.equal(data.charts.length,19);
  for(const c of data.charts){assert.match(c.source,/^https:\/\/github.com\//);assert.match(c.sha256,/^[a-f0-9]{64}$/);assert.ok(c.points.length);for(const p of c.points){assert.ok(Number.isFinite(p.value)&&p.value>=0);assert.ok(p.label.includes(p.library));}}
});
test('writer, memory and concurrent comparisons retain their actual populations',()=>{
  for(const c of data.charts.filter(c=>c.id.startsWith('writer-'))) assert.deepEqual([...new Set(c.points.map(p=>p.library))].sort(),['libmaxminddb-rs','mmdbwriter']);
  for(const c of data.charts.filter(c=>c.id.startsWith('memory-'))) assert.equal(new Set(c.points.map(p=>p.library)).size,4);
  for(const c of data.charts.filter(c=>c.id.startsWith('concurrent-'))) assert.ok(c.points.every(p=>p.dimension!=='1T'));
});
test('examples use complete main functions and match repository files',()=>{
  const examples=JSON.parse(readFileSync(new URL('../public/data/examples.json',import.meta.url)));
  for(const e of examples){assert.equal(e.code,readFileSync(new URL(`../../examples/${e.id}.rs`,import.meta.url),'utf8'));assert.match(e.code,/fn main\(/);assert.ok(!e.code.includes('# fn main'));}
});
