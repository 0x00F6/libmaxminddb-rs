# libmaxminddb-rs project website

A static single-page application for GitHub Pages. Vite bundles ECharts and
Highlight.js locally: charts, fonts and syntax highlighting require no CDN.
The terminal-inspired interface defaults to dark, remembers the light-theme
choice and uses hash navigation so direct links work on project Pages.

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

The importer validates the combined SHA-256 of the generated SVG files against
`chartsSha256` in the context. A changed export deliberately fails until its
metadata is reviewed. Copy the digest reported by the importer only after that
review. Values come from the suite's SVG tooltip labels; they preserve the
source's exported precision. No missing values are filled in.

Throughput and latency are separate measurements. Concurrency excludes the
1-worker series because the report combines fallback single-thread scenarios.
Reader RSS includes four Rust/C implementations; the suite does not export Go
reader RSS. Writer comparisons include both libmaxminddb-rs and Go mmdbwriter.
Code examples are copied verbatim from `examples/` during the build.

## Publish

`.github/workflows/pages.yml` builds and checks the website on `main` and
`feature/github-pages`. Enable **Settings → Pages → Source → GitHub Actions**
once, and allow the publishing branch in the `github-pages` environment.
Then rerun the workflow to deploy to:

https://0x00f6.github.io/libmaxminddb-rs/

When Pages has not been enabled, CI keeps the tested build artifact and explains
the missing repository setting without claiming a successful deployment.
The full comparison workflow is separate, so site edits do not rerun benchmarks.
