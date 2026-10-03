# AGENTS.md

This file is the operating guide for humans and AI agents modifying `libmaxminddb-rs`.

## Mission

Maintain an independent, high-performance, memory-conscious Rust implementation of MaxMind DB v2. Performance, especially IPv6 random-lookup tail latency on large databases, is a primary engineering goal. Pursue aggressive CPU-cache optimizations, but correctness and parser safety come before benchmark wins. Never replace the core implementation with another MMDB crate or C library.

## Repository map

- `src/lib.rs`: public API and feature gates.
- `src/reader/`: file/buffer sources, metadata discovery, search-tree traversal, lookup.
- `src/writer/`: prefix trie, deep merge, data interning, explicit no-data boundaries, node serialization.
- `src/editor.rs`: copy-on-write overlay for rebuilding existing MMDB files; source records stay borrowed until rebuild.
- `src/decoder/`: MMDB control-byte, pointer, scalar, map and array decoding.
- `src/encoder.rs`: MMDB control-byte and payload encoding.
- `src/metadata.rs`: public metadata type and builder.
- `src/value.rs`: borrowed `ValueRef` and owned `Value`.
- `src/traits.rs`: derive support and field conversions.
- `derive/`: separately published workspace proc-macro crate for `MmdbDecode`, `MmdbEncode`, and `MmdbRecord`; `#[mmdb(network)]` marks the CIDR field for one-object writer insertion.
- `tests/`: integration, corruption, type and compatibility tests.
- `benches/`: native Criterion benchmarks.
- `tools/`: cross-implementation benchmark harnesses only.
- `fuzz/`: cargo-fuzz targets.
- `scripts/`: coverage, compatibility and report tooling.
- `tests/fixtures/doc.mmdb`: small checked-in MMDB used by runnable reader doctests.
- `.github/workflows/ci.yml`: formatting, lint, test, documentation and package checks.

## Packaging and releases

The main crate's `Cargo.toml` uses an explicit include list so benchmark result
archives, local reports, IDE files and tool harnesses never enter the crates.io
package. The proc-macro crate is a separate workspace member and must be
published first. Before it is available on crates.io, verify the main archive
with the local `derive` registry patch used by `make publish-check` and CI.
`make release X.Y.Z` pushes to GitHub, publishes the derive crate and then the
main crate to crates.io, and creates the tag and GitHub Release. Never invoke it
as part of a validation-only task.

## Format invariants

The MMDB v2 file layout is:

1. search tree;
2. exactly 16 zero bytes;
3. data section;
4. `\xab\xcd\xefMaxMind.com` marker;
5. metadata map.

Tree pointers have three meanings:

- `< node_count`: another tree node;
- `== node_count`: no data;
- `>= node_count + 16`: data-section address.

Values `node_count + 1 .. node_count + 15` are invalid.

The file offset for a tree data pointer is:

```text
search_tree_size + (record_value - node_count)
```

Do not change pointer formulas without adding/expanding compatibility tests.

## Performance policy

The lookup hot path should avoid:

- cloning the MMDB buffer;
- `String` creation for borrowed strings;
- `Vec<u8>` creation for borrowed byte fields;
- parsing metadata per lookup;
- unnecessary address conversions;
- virtual dispatch.

Internal traversal outcome is a compact `Option<(u64, u8)>` (24 bytes), not a
`Result<(u64, u8), Error>` (48 bytes). Search-tree traversal can only fail with
`NotFound`; the internal walks (`byte_walk_*`, `walk_v4/v6`,
`walk_scalar_128`, `PreparedTree::traverse*`, `Reader::traverse_ipv4/6`)
return the small value and the reader's private offset resolver converts it to the public
`Error` exactly once at its boundary. Keep it that way when changing the
tree: the miss path (absent/random/sequential workloads) is the dominant
serving shape and the large `Result` cost a reproducible 10-18% there
(Criterion same-run A/B: `lookup_ipv4_miss` 12.0 -> 10.2 ns, `lookup_ipv6_miss`
18.1 -> 16.5 ns; official suite: absent-IPv4 throughput 122.1 -> 164.5 M ops/s,
sequential-IPv4 114.2 -> 157.2 M ops/s, random-IPv6 69.0 -> 86.0 M ops/s).
All valid record sizes now use the prepared tree. Opening reports preparation
errors directly; traversal returns a compact `Option` and the offset resolver
performs pointer validation at the public boundary.

Current known allocation: maps and arrays decoded into `ValueRef` allocate their container `Vec`. Typed borrowed decoding skips these generic container vectors when the target fields can borrow the database buffer.

Every performance change must include:

1. correctness tests;
2. `cargo fmt` / clippy;
3. a benchmark demonstrating the change;
4. a note if latency improves by trading memory, code size, or safety.

Never keep SIMD or `unsafe` just because it looks faster. Retain it only with reproducible measurements.

### CPU-cache and lookup-latency priorities

Optimize the whole lookup path for large working sets, not just hot addresses that
fit in L1. Track p50, p95, p99 and throughput for IPv4 and IPv6 hits, misses,
random and sequential queries; include concurrent scaling, preparation during
open, first-lookup latency and RSS. The random IPv6 p99 is a priority, but an
apparent win there must not conceal a reproducible regression elsewhere.

- Favor contiguous, 64-byte-aligned nodes and compact, immutable lookup tables.
  `src/reader/tree.rs` currently stores two native-endian `u32` children per
  8-byte node, with eight complete nodes per cache line. Preserve that layout
  unless a measured alternative wins across representative workloads.
- Reduce dependent cache misses by consuming multiple address bits per lookup
  through the existing byte-stride arena or radix root tables. Keep table
  footprints and open-time construction bounded; record any increase in RAM,
  cache pressure or initialization time. More RAM is acceptable when the gain
  is repeatable and the tradeoff is explicit.
- Keep frequently used bases, counts and address words available to the
  traversal without repeated indirections or conversions. Prefer borrowed
  outputs and avoid per-lookup allocations. Evaluate branch-light child
  selection and SIMD only where the measured workload supports them.
- Treat prefetch as an experiment, not a blanket rule. Try appropriate L1/L2/L3
  hints, distances and placements using architecture intrinsics; inline
  `core::arch::asm!` is acceptable for a measured instruction variant with
  documented safety and portable fallback. Dependent tree loads often leave
  too little lead time for a useful prefetch. Reject variants that pollute
  caches, consume excess bandwidth or regress other workloads. The current
  x86-64 native-tail prefetch in `src/reader/tree.rs` is an implementation
  detail, not a mandate to add more prefetches.
- Use profiling and generated assembly to locate expensive loads, cache
  misses, branch mispredictions and redundant work before changing the hot
  loop. Compare one targeted change at a time against a saved baseline, repeat
  runs under the same CPU affinity, compiler, flags and fixtures, then measure
  the cumulative result. Do not infer a gain from one noisy sample.

### Traversal and decode optimization (recorded baseline)

Hot-path optimizations applied and, when reproducible measurements were available,
their before/after deltas are recorded. Re-run these before changing the hot paths:

- `Reader::traverse` no longer calls `read_record` per bit. It branches once on
  `record_size` (specialized 24/28/32 loops), inlines node unpacking, and relies on
  `node < node_count` to keep `tree[offset..offset+node_size]` in-bounds (node size is
  a multiple of 4 and the tree is exactly `node_count * node_size` bytes). `read_record`
  remains for open-time `compute_ipv4_start`.
- `Reader::traverse` reads each node as one unaligned 8-byte big-endian word (`load!`).
  The 28-bit layout is **not** contiguous: the high nibble of byte 3 belongs to the
  left record, so the specialized formulas in `traverse` must be kept (`word >> 40`,
  `(word >> 16) & 0x00ff_ffff`; `(high >> 8) | ((high & 0xf0) << 20)` and
  `(low >> 8) | ((high & 0x0f) << 24)`; `word >> 32` and `word & 0xffff_ffff`). The
  8-byte window is in bounds on every iteration because the file is at least
  `node_count * node_size + 16` bytes (validated in `from_source`), so the last node
  still has its slack plus the 16-byte zero separator. `bits > 32` falls back to
  `read_packed_record`.
- `Decoder` truncates its backing slice to `[..limit]` at construction, so `slice()`
  performs a single bounds check instead of a separate `limit` comparison.
- Decoder bounds hardening: every `get_unchecked` load is guarded by a cheap branch.
  `decode_inner` checks `offset < data.len()` once per recursion (this is also the
  net for arbitrary pointer targets), the leaf fast paths require
  `cursor < data.len()`, and each payload read is preceded by the inlined
  `in_range(offset, len)`. `slice()` stays an unchecked `&[u8]` so the hot reads
  never funnel through `Result<&[u8], Error>`: returning that fat 32-byte `Result`
  cost a reproducible ~27% on `city_lookup_ipv4`; the branch-guarded form costs
  ~5% over the pre-hardening baseline. `u8` extended-type bytes 249..=255 are
  rejected as `InvalidDataType` (reserved type-descriptor continuation) instead of
  overflowing the `ext + 7` addition.
- Hot-path errors are constructed lazily: `ok_or_else` is used throughout
  `decoder/mod.rs` and `reader/mod.rs` so the large, `String`-bearing `Error` enum is
  never materialized on the success path. The `#[allow(clippy::unnecessary_lazy_evaluations)]`
  attributes on those methods are intentional; reverting them to `ok_or` costs a
  reproducible 15-20% on the reader microbenchmarks.
- String decoding inlines the ASCII probe before the checked validator and moves the
  non-ASCII fallback into `#[cold] #[inline(never)] utf8_validated`, keeping
  `std::str::from_utf8` out of the hot instruction stream. Only proven-ASCII payloads
  skip validation; any non-ASCII byte still goes through the full checked validator,
  so `from_utf8_unchecked` is reachable only with an ASCII proof (see `SAFETY` comment
  in `decoder/mod.rs`).
- `decoder/ascii.rs::is_ascii` uses the retained measured x86-64 crossover: SWAR below
  64 bytes, SSE2 for 64–255 bytes, and AVX2 from 256 bytes when runtime/compile-time
  support is available. AVX-512 remains a benchmark kernel but is not auto-dispatched
  because the retained Zen 4 measurements were slower than AVX2. AArch64 uses NEON from
  64 bytes. All vector loads are unaligned and wholly within the slice; no padding is required.

Measured on the deterministic `GeoIP2-City-Bench.mmdb` dataset (see benchmark policy):

- Criterion (cumulative, pre-session -> current): `reader/lookup_ipv4_hot` 330 -> 193 ns;
  `lookup_ipv6_hot` 285 -> 163 ns; `lookup_ipv4_random` 278 -> 206 ns;
  `decode_borrowed_struct` 357 -> 212 ns; `open_from_bytes` 916 -> 696 ns;
  `cold_parse_plus_lookup` 1.44 -> 0.90 µs; `city_lookup_ipv4` 1.46 -> 0.99 µs;
  `tree_only_v6_miss` 27.9 -> 25.2 ns.
- Cross-implementation `lookup_decode_city` (`make bench-compare`): mean 1499 -> 1313 ns
  (p95 2064 -> 1362 ns); throughput 650 -> 743 k ops/s.
- Historical ASCII scan of a 64 KiB all-ASCII buffer: scalar 4053 ns,
  SSE2 813 ns, AVX2 546 ns, AVX-512 558 ns (~7.4x vs scalar). The AVX-512 path is
  retained for capable hardware but is not faster than AVX2 on this Zen 4 host
  (double-pumped 512-bit execution); it loses only ~2% at 64 KiB.
- valgrind callgrind, `lookup/map_sum`, 20000 iterations: the lazy-error + wide-load
  changes took 825.5M -> 665.6M instructions (-19%), and the inline-UTF-8 + SWAR-tail
  change took 665.6M -> 609.1M (-8.5%); `utf8` no longer appears in the profile.

Those figures are retained historical measurements for the supplied pre-2026-09-19
source state, not a baseline for the current tree. Do not claim current deltas
without rerunning `make bench-performance` on the same Rust host and workload.

## Unsafe policy

`unsafe` must be minimal, documented and testable. Each block needs a `SAFETY:` comment describing the invariant.

MMAP is particularly sensitive. A mapped file must not be modified/truncated while borrowed by the process. Keep mmap construction behind an explicit unsafe API unless a fully safe ownership/file-stability mechanism is introduced.

Do not use unchecked UTF-8 for untrusted MMDB data without a separately gated, documented API.

## Parser hardening

Treat every MMDB as untrusted input. Preserve or strengthen:

- final-128-KiB metadata marker limit;
- tree-size overflow checks;
- separator validation;
- pointer bounds;
- data-section/metadata boundaries;
- nesting limits;
- total expanded-value budget;
- total expanded string/byte budget;
- container-size sanity checks;
- UTF-8 validation;
- major format/IP version validation.

Fuzz parser changes.

## Writer rules

The writer must be deterministic for identical insert order and data. Preserve:

- longest-prefix behavior;
- inherited parent values for missing child branches;
- IPv4 subtree encoding in IPv6 databases;
- data deduplication by encoded value;
- metadata emitted after the marker;
- 24/28/32-bit pointer selection based on generated size.

Deep merge semantics are API behavior. Changing them is a breaking change unless a new strategy is introduced.

## Feature isolation

Keep these commands working:

```bash
cargo build
cargo build --no-default-features --features reader
cargo build --no-default-features --features writer
cargo test --no-default-features --features reader
cargo test --no-default-features --features writer
```

Reader-only code must not accidentally require writer modules and vice versa.

## Required checks

Before submitting normal changes:

```bash
make check
```

Before performance-sensitive changes:

```bash
make tests
make bench-performance
make bench-compare
```

Before parser/decoder changes:

```bash
make coverage
make fuzz
```

If an optional tool is unavailable, state that fact. Never claim a benchmark, fuzz run, coverage percentage, compatibility test, or build passed unless it actually ran.

## Benchmark policy

`make bench-micro` adds native `native-million-v1` fixtures with exactly
1,000,000 distinct IPv4 /24 or IPv6 /64 routes. Every hit and miss is verified
against the serialized reader before timing; all large-fixture lookups use
`lookup_borrowed`. Keep query cursors across Criterion samples. The first run
creates Criterion baselines in `benchmarks/baseline`; later runs compare against
them and write `summary.json`, `summary.md` and `summary.html` under
`target/micro-benchmarks/runs/`. The report labels changes within ±3% as
noise; that threshold is a screening rule, not proof of statistical equivalence.
The runner checks compiler, CPU, flags and fixture identity before comparing.
RSS scenarios must exec fresh workers after preparation, load auxiliary queries
before the pre-open RSS sample, and report their capacity separately from reader
RSS deltas.
Never use a fixture generator's high-water RSS as reader memory.

Comparison libraries may appear only in benchmark/compatibility tooling, never core dependencies.

The default cross-implementation dataset is generated deterministically by `tools/gen-bench-data` (our own writer emits a GeoIP2-City-shaped MMDB with exact integer widths), not downloaded. Some pinned competitor versions (notably geoip2-rs 0.1.8) cannot parse current MaxMind test databases, and libmaxminddb rejects `build_epoch == 0`. When changing the dataset generator, re-verify that all four implementations can open and decode the resulting file.

Keep dataset, queried IPs, warmup and measured iteration counts identical when comparing implementations. Capture:

- implementation version/commit;
- Rust/C compiler;
- optimization flags;
- CPU and architecture;
- OS;
- dataset identity;
- operation definition (tree-only vs lookup+decode).

Lookup harness closures must mirror the competitors' miss-path shape: `black_box` the decoded value **only on a hit** (the form maxminddb-rust and geoip2-rs use). Black-boxing a large decoded `Result`/`Option` unconditionally on every miss forces a dead multi-hundred-byte copy per op (our `Option<CityRecord>` is 256 bytes) that competitor miss paths never pay, skewing the absent/random categories against us. Measured 2026-09 (pinned Zen 4, 200k): unconditional black_box cost ~5-6 ns/op on ipv4/absent; aligning ours to hit-only black_box took it from 77 to ~135 M/s with zero reduction in functional work.

Do not copy benchmark numbers from upstream READMEs into our generated result tables. The report generator must display only locally measured data.

Current comparison targets at repository generation time:

- maxminddb-rust 0.32.0;
- geoip2-rs 0.1.8;
- libmaxminddb 1.14.1.

Re-verify these before future benchmark publications.

## Coverage

`make coverage` uses `cargo-llvm-cov` and writes HTML to:

```text
target/coverage/html/index.html
```

Aim for >90% where reasonable, but prioritize meaningful branch/error-path tests over gaming the percentage.

## Documentation

Public behavior changes require updates to:

- rustdoc;
- README examples;
- AGENTS.md when architecture/invariants change;
- benchmark methodology when measurements change.

Doctests must compile.

## Style

- Rust 2024 edition.
- Prefer explicit error propagation over `unwrap()`/`expect()` in library code.
- `unwrap()` is acceptable in tests and benchmark fixture setup where failure means the fixture itself is invalid.
- Keep functions focused; comment invariants rather than obvious syntax.
- Avoid premature abstraction on bit-level hot paths.
- Use deterministic collections in serialized output where ordering matters.

## Sensitive areas

Changes in these areas deserve extra review:

- `decode_pointer`;
- extended-size decoding;
- `record_to_file_offset`;
- 28-bit node packing/unpacking;
- IPv4-in-IPv6 subtree traversal;
- inherited trie values during writer flattening;
- metadata pointer base;
- resource limits;
- mmap safety.

## Decoder and SIMD invariants

- Generic decoding enforces the depth, pointer, UTF-8 and expanded-value/byte budgets.
  Keep recursive frames small in debug/sanitizer builds: scalar decoding is a separate
  function, inlined into optimized builds. Test depth-limit and cyclic inputs.
- The default `simd` feature selects SSE2 at 64–255 bytes and AVX2 from 256 bytes
  when available on x86-64, and NEON from 64 bytes on aarch64, for ASCII scans only.
  Short strings and other targets use SWAR. All vector loads are unaligned and wholly
  within the slice; no padding is required. This is an ASCII proof only, never
  permission to accept invalid UTF-8.
- Writer data interning stores encoded payloads once. Hash collisions must compare
  actual bytes; hashes never determine serialization order. Inherited offsets are
  cached lazily so first-use data ordering stays identical.
- `benches/reader.rs`, `benches/million_reader.rs`, `benches/search_strategies.rs`
  and `benches/writer.rs` feed `make bench-micro`; `make bench-performance`
  runs the reader and borrowed-lookup Criterion suites.


## Concurrent reader and benchmark invariants

- The native fast tree and its radix/byte-stride tables are prepared during
  `Reader::from_source`, before any lookup or worker thread can use the reader.
  The lookup path only borrows the mandatory immutable `PreparedTree`; do not move
  construction back to first use. Preparation errors fail `Reader::open`.
  Report preparation in open-time and RSS measurements. The 24/28/32-bit
  records use aligned `u32` children and acceleration tables; 36..=64-bit
  records use two decoded `u64` children per node (16 bytes/node) prepared at
  open. The lookup path has only IPv4 and IPv6 traversal entry points.
- For at least 128 nodes, IPv6 may use an immutable byte-stride arena. Each
  8-byte entry stores original record (32 bits), exact consumed depth (8 bits),
  and next-table index (24 bits). Every table has exactly 256 entries. The arena's
  `byte_entries` are owned directly by `PreparedTree` (`Box<[RootEntry]>` plus the
  IPv4-subtree table index), not nested behind `Option<ByteTree>`, so the hot
  per-lookup traversal fetches the entries base with a single load from `self`.
- Initialize the 64-byte-aligned native node buffer directly; do not restore a
  temporary `Vec` followed by a full-tree copy. Check allocation multiplication
  and layout bounds, initialize every child slot before exposing a slice, and
  retain ownership during initialization so errors/unwinding deallocate safely.
- Only reachable byte-boundary nodes are expanded; deduplicate by original node
  to bound preparation for DAGs/cycles. The final arena budget is native-tree
  bytes + 1 MiB, capped at 64 MiB; fall back to the original traversal on overflow.
  Queries consume at most 16 bytes, including for cyclic malicious trees. Reserved
  data pointers still pass through Reader's existing offset validation.
- If the byte arena does not fit and there are at least 2^20 nodes, the native
  fallback uses a 20-bit root table (8 MiB) instead of 16 bits (512 KiB). It
  retains original records and exact depths, including leaves before bit 20;
  IPv4/IPv6 tails consume 12/108 bits at most. Never duplicate the root table
  when the IPv4 subtree is the root. Empty trees must retain count zero during
  preparation so no native-node load is attempted.
- Preserve bit-exact prefixes, non-byte-aligned leaves and bounded cyclic
  traversal when accumulating byte-entry depths.
- Byte traversal returns `None` immediately for `node == node_count`, before
  processing a data pointer or accumulating depth. Keep reserved pointers
  (`node_count + 1..=node_count + 15`) on Reader's offset-validation path.
- Comparative and Criterion City lookups share the `city::lookup` adapter in
  `tools/rust-competitor-bench/src/city.rs`, which calls `lookup_borrowed` for both
  address families. The requested subdivisions `Vec` allocates once per City
  hit; strings remain borrowed. Misses allocate nothing.
- The comparison runner honors `BENCH_PIN` and `BENCH_CPU` for single-threaded
  children; concurrent children retain the parent's CPU set. Record affinity
  and measurement protocol, and do not combine pinned and unpinned results.
- The cross-library concurrent harness uses thread-local allocation counters in
  Rust and C. Never restore per-allocation shared atomics in measured loops.
  Ready/go/done barriers exclude worker setup/teardown; start clocks before go.
- Keep concurrent benchmark operations distinct by the public reader API they
  exercise. Generic value decoding still allocates map/array vectors.
- Reports must group by thread count, operation and measurement protocol, and
  compare history only for compatible datasets, workloads and environments.

- Reader builds always exercise fast-tree preparation, including the fuzz
  package. Seed fuzzing with a valid generated MMDB to reach tree/decode paths.

## Comparative reader RSS

- `memory-rss-v2` compares the four Rust/C readers in mmap mode at all eight
  scaling sizes, with three isolated processes per point. Prepare and verify
  exact-size IPv4 /32 databases outside the measured children; preserve the
  common binary 50/50 hit/miss workload and SHA-256 identities.
- Measure Linux VmRSS before/after open and reset VmHWM immediately after open
  for the lookup peak. Keep the 4,000,000-byte query buffer in the baseline and
  report its size separately. Aggregate after-open RSS by median, lookup peaks
  by maximum. Missing/failed repeats and unavailable peaks must stay explicit,
  never zero; never mix legacy protocols or incompatible inputs/opening modes.
- Keep this RSS protocol distinct from the four owned/mmap scenarios reported
  by `make bench-micro`.

## Verified IPv6 absent benchmark

- The `ipv6-absent-v1` scenario uses a dedicated deterministic MMDB with 1,000,000
  distinct random IPv6 /128 routes and 1,000,000 distinct random misses.
- Generate and verify the serialized fixture with `gen-scaling-bench-data ipv6-absent`;
  never infer absence from an address range. Every reader verifies all misses before timing.
- Rebuild harnesses through their build tools; an existing executable is not proof of freshness.
- Preserve scenario dimensions and diagnostics on failures; failed/unsupported runs are not measurements.
- `run_benchmarks_compare --ipv6-absent-only` replaces only this scenario after all five
  readers succeed. Dataset/workload SHA-256 and protocol identify compatible result groups.
