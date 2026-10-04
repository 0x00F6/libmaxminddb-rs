# libmaxminddb-rs project website

A static single-page application for GitHub Pages. Vite bundles ECharts and
Highlight.js locally: charts, fonts and syntax highlighting require no CDN.
The terminal-inspired interface defaults to dark, remembers the light-theme
choice and uses hash navigation so direct links work on project Pages.
The existing `docs/images/ferris-maxmind.png` logo is bundled locally for the
homepage, navigation and favicon.

## Run and verify

Requires Node.js 22+, npm and Python 3. A Chrome/Chromium executable is required
for browser checks (`CHROMIUM_PATH` overrides auto-detection).

```sh
make website-install
make website-serve
# Open the /libmaxminddb-rs/ path printed by Vite.
make website-check
```

The build is in `website/dist`. Browser tests exercise all comparison controls,
SPA history, source highlighting, persistent theme selection and mobile layout.
Screenshots are written to `website/test-results` and retained by CI.

## Refresh the measurements

```sh
make bench-compare
# Record the benchmarked commit, date, CPU count, machine and toolchain versions
# in website/benchmark-context.json. Preserve the raw JSON/CSV and run logs.
make website-data
make website-check
```

`benchmark-runs.json` maps each architecture to its context, SVG directory and
output JSON. Preserve existing runs when adding another machine. The importer
validates the combined SHA-256 of each run's SVG files against `chartsSha256`
in its context. A changed export deliberately fails until its
metadata is reviewed. Copy the digest reported by the importer only after that
review. Values come from the suite's SVG tooltip labels; they preserve the
source's exported precision. No missing values are filled in.

The architecture selector switches the complete measured snapshot: all chart
categories, tables, source links, downloads and methodology. Its selection is
remembered locally and encoded in shareable links such as
`?arch=arm64#benchmarks`. Comparisons between machines also reflect their CPU,
compiler and runtime differences; they do not isolate an architecture effect.

Throughput and latency are separate measurements. Concurrency excludes the
1-worker series because the report combines fallback single-thread scenarios.
Reader RSS includes four Rust/C implementations; the suite does not export Go
reader RSS. Writer comparisons include both libmaxminddb-rs and Go mmdbwriter.
Code examples are copied verbatim from `examples/` during the build.

Latency opens on **Candlestick Percentile Rank** for IPv4 or IPv6 random
lookups. Like the suite's `candlestick-percentiles-*.svg` exports, each row
shows a minimum wick, a p50–p95 body and a p99 tail marker, ranked by p99.
The chart labels p50 and p99 together; tooltips and the accessible table retain
all five exported statistics, including the maximum (not plotted). These are
measured quantiles, not confidence intervals. Database-size scaling remains a
separate p99-only scenario.

## Publish

`.github/workflows/pages.yml` builds and checks the website on `main` and
`feature/github-pages`. Enable **Settings → Pages → Source → GitHub Actions**
once, and allow the publishing branch in the `github-pages` environment.
Then rerun the workflow to deploy to:

https://0x00f6.github.io/libmaxminddb-rs/

When Pages has not been enabled, CI keeps the tested build artifact and explains
the missing repository setting without claiming a successful deployment.
The full comparison workflow is separate, so site edits do not rerun benchmarks.

## Published measurement sets

The 4 October 2026 snapshot comes from Actions run `37203873066` at benchmarked
commit `33d8f1f`. The complete suite finished successfully in 743 seconds on an
AMD EPYC 9V74 runner with 4 vCPUs. It produced 238 successful aggregate rows
and one explicitly unsupported C `open_buffer` row. Source exports are pinned
to a separate commit and hashed. The raw JSON, JSONL, CSV, full generated report
and environment evidence are retained in `public/data/run-2026-10-04/`.

Go provenance has a limitation: environment capture records launcher 1.23.1,
but the module requires 1.25.0 and automatic switching was enabled. The exact
Go compiler was not archived. The site and README make this explicit; the raw
generated report is preserved unmodified. 8/16-worker comparisons oversubscribe
the runner, and this run should not be used to claim a regression or improvement
against older results collected on a different CPU.

The native ARM64 snapshot comes from Actions run `37222685709` at benchmarked
commit `07022c0`, on a Neoverse-N2 runner with 4 vCPUs. The full suite completed
in 780.3 seconds with 238 successful aggregate rows and the same unsupported
C `open_buffer` case. The raw results, 33 original SVG exports, generated report,
dataset hashes and environment evidence are preserved in
`public/data/run-2026-10-04-arm64/`. Both architectures expose 19 comparison
charts in the website.

The ARM run used Rust 1.98.1, GCC 13.3.0 and Go 1.25.0. Go binary build metadata
confirms the effective compiler for both reader and writer. The benchmarked
commit includes the Rust 2024 NEON compilation fix; it is identified separately
from the package version. Native ARM `make check` and `make fuzz` passed. The
archived dataset and workload hashes match the x86 run for the protocols that
record them (`memory-rss-v2` and `ipv6-absent-v1`). This does not make the two
machines a controlled comparison of instruction-set architectures.

Each Pages archive has a unique run/attempt name. Deployment consumes the build
job's recorded archive name, so rerunning a build or only the deployment cannot
select an older archive with a duplicate name.
