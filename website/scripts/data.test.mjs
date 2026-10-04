import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
const data = JSON.parse(
  readFileSync(new URL("../public/data/benchmarks.json", import.meta.url)),
);
test("every displayed measurement has a real source and a finite nonnegative value", () => {
  assert.equal(data.charts.length, 19);
  for (const c of data.charts) {
    assert.match(c.source, /^https:\/\/github.com\//);
    assert.match(c.sha256, /^[a-f0-9]{64}$/);
    assert.ok(c.points.length);
    for (const p of c.points) {
      assert.ok(Number.isFinite(p.value) && p.value >= 0);
      assert.ok(p.label.includes(p.library));
    }
  }
});
test("writer, memory and concurrent comparisons retain their actual populations", () => {
  for (const c of data.charts.filter((c) => c.id.startsWith("writer-")))
    assert.deepEqual([...new Set(c.points.map((p) => p.library))].sort(), [
      "libmaxminddb-rs",
      "mmdbwriter",
    ]);
  for (const c of data.charts.filter((c) => c.id.startsWith("memory-")))
    assert.equal(new Set(c.points.map((p) => p.library)).size, 4);
  for (const c of data.charts.filter((c) => c.id.startsWith("concurrent-")))
    assert.ok(c.points.every((p) => p.dimension !== "1T"));
});
test("candlesticks preserve measured random lookup quantiles without duplicate ranks", () => {
  const raw = readFileSync(
    new URL("../public/" + data.context.rawResults, import.meta.url),
    "utf8",
  )
    .trim()
    .split("\n")
    .map(JSON.parse);
  for (const family of ["ipv4", "ipv6"]) {
    const chart = data.charts.find(
      (c) => c.id === `candlestick-percentiles-${family}`,
    );
    assert.ok(chart);
    assert.equal(chart.points.length, 5);
    assert.equal(new Set(chart.points.map((p) => p.library)).size, 5);
    for (const point of chart.points) {
      const key =
        point.library === "libmaxminddb (C)" ? "libmaxminddb" : point.library;
      const row = raw.find(
        (r) =>
          r.implementation === key &&
          r.scenario === "lookup" &&
          r.pattern === "random" &&
          r.family === family,
      );
      assert.ok(row && !row.failed && !row.unsupported);
      assert.equal(point.value, point.quantiles.p99);
      for (const metric of ["min", "p50", "p95", "p99", "max"]) {
        // SVG latency labels round to two decimals in their exported unit.
        const tolerance = point.quantiles[metric] >= 1000 ? 5.01 : 0.0051;
        assert.ok(
          Math.abs(point.quantiles[metric] - row[`${metric}_ns`]) < tolerance,
          `${family} ${key} ${metric}`,
        );
      }
    }
  }
});
test("examples use complete main functions and match repository files", () => {
  const examples = JSON.parse(
    readFileSync(new URL("../public/data/examples.json", import.meta.url)),
  );
  for (const e of examples) {
    assert.equal(
      e.code,
      readFileSync(
        new URL(`../../examples/${e.id}.rs`, import.meta.url),
        "utf8",
      ),
    );
    assert.match(e.code, /fn main\(/);
    assert.ok(!e.code.includes("# fn main"));
  }
});

test("published values retain the source export and fresh run identity", () => {
  assert.equal(
    data.context.sourceCommit,
    "33d8f1fd80edbc2dcd9af19b2de41447cd70b118",
  );
  const raw = readFileSync(
    new URL("../public/" + data.context.rawResults, import.meta.url),
    "utf8",
  )
    .trim()
    .split("\n")
    .map(JSON.parse);
  for (const chart of data.charts) {
    const source = readFileSync(
      new URL(`../../benchmarks/charts/${chart.id}.svg`, import.meta.url),
    );
    assert.equal(
      createHash("sha256").update(source).digest("hex"),
      chart.sha256,
    );
    if (chart.id.startsWith("throughput-")) {
      const [, family, pattern] = chart.id.split("-");
      for (const point of chart.points) {
        const key =
          point.library === "libmaxminddb (C)" ? "libmaxminddb" : point.library;
        const row = raw.find(
          (r) =>
            r.implementation === key &&
            r.scenario === "lookup" &&
            r.family === family &&
            r.pattern === pattern,
        );
        assert.ok(row && !row.failed && !row.unsupported);
        assert.ok(
          Math.abs(row.throughput_ops_s / 1e6 - point.value) < 0.0051,
          chart.id + " " + key,
        );
      }
    }
  }
});
