# Full database scans

## API and ownership

Both `Reader::visit_records` and `Reader::visit_borrowed_records` are compiled
with `reader` alone. Neither depends on `writer` or `derive`. The latter uses
`MmdbDecode`; derive macros are an optional convenience.

Both APIs stream reachable leaf ranges in increasing stored address order and
skip no-data pointers. Embedded IPv4 ranges under `::/96` are exported as IPv4;
all other IPv6 ranges retain their stored family. Equal payloads referenced
by multiple prefixes are visited for every prefix. Exported ranges may split
writer insertions around more-specific records, so they are not a reconstruction
of original source CIDRs.

Strings and byte slices borrow the reader and can be retained after each
callback, as long as the reader remains borrowed. Neither API clones the MMDB
buffer. Generic `ValueRef` maps and arrays allocate their vectors. Derived
borrowed scalar records decode directly, avoiding those containers; `String`,
`Vec` and inventory set nodes still allocate. Manual decoders may use the
generic decoder through the trait's default implementation.

Callbacks return the library's `Result<()>`. The first callback, decoding or
traversal error is returned unchanged and stops the scan. Results already
collected are partial; do not publish them as a complete inventory until the
scan returns success. Nothing writes to or changes the source MMDB.

## Defensive traversal

Traversal uses a fixed 129-entry stack (4,128 bytes on this host) plus a
128-entry ancestor array (1,024 bytes), with no traversal heap allocation. It
tracks the active ancestors and consumes
at most `256 * (node_count + 1)` tree entries (saturating on arithmetic overflow).
Cycles and internal nodes beyond 32 IPv4 or 128 IPv6 bits return
`Error::InvalidDatabase`. Exponential expansion of shared subtrees returns
`Error::ResourceLimit`, rather than silently truncating the result. Valid DAG
aliases are retained within that budget. Data pointers use the existing
reserved-pointer, file-boundary and decoder-budget checks.

The scans do not change the lookup path, prepared tree, accelerator layout or
publication behavior. A scan on a `ReloadableReader` guard continues to borrow
one immutable generation, even when another reader is published concurrently.

## Parallel borrowed scans

`Reader::visit_borrowed_records_parallel` adds an unordered `Fn + Sync` visitor.
It uses `std::thread::scope` and available CPUs for trees with at least 24,000
nodes, with a sequential fallback for smaller trees or one CPU. The explicit
`visit_borrowed_records_parallel_with_workers(NonZeroUsize, visitor)` variant
bypasses that heuristic; one worker still uses the sequential path. Limits are
capped at 256 workers and at the frontier's task count.

Scheduling splits shallow populated subtrees into a bounded frontier of up to
eight tasks per worker. A long shared prefix (including the IPv4 subtree in an
IPv6 database) is expanded until there is enough work or no internal task is
left. No complete offset/record list is materialized. Each task carries its
ancestor path; workers use the same checked fixed-stack DFS as sequential scans.
Each record is decoded and consumed on its worker, so `T` itself requires
neither `Send` nor `Sync`; only the callback must be `Sync`.

Frontier expansion and all workers share one saturating entry budget. Workers
reserve up to 256 credits at a time and retain them across tasks. Reservations
never multiply the total budget. Unused worker credits may cause conservative
early rejection of pathological, heavily aliased DAGs near the limit. Normal
non-aliased trees require far fewer entries than that bound.

Callback, decoder, traversal and thread-creation errors trigger cooperative
cancellation. Already-running callbacks may finish before observing it; an
unspecified worker failure is returned when several occur. Every started worker
is joined before return, including on failure. Callback panics cancel the other
workers and propagate after joining. Treat all inventories as partial until
the method returns success. The ordered sequential APIs keep their existing
immediate-error behavior.

The frontier uses at most approximately `workers * 8 * size_of::<Task>()` heap
bytes (1,056 bytes per task on this x86_64 host), plus worker handles and standard
thread resources. Each worker keeps a 129-entry DFS stack and 128 ancestors.
Scalar borrowed decoding adds no per-record allocation; target `Vec`/`String`
fields and callback collections still allocate. Shared mutexes in callbacks
can limit scaling; prefer worker-local aggregation when throughput matters.

## Self-contained writer and scan example

With `reader,writer,derive` enabled, run the example without arguments to build
a synthetic in-memory IPv4/IPv6 database and verify it with assertions:

```bash
cargo run --no-default-features --features reader,writer,derive \
  --example unique_fields_scan_records
```

Each address is inserted twice with `MergeStrategy::DeepMerge`. The scan checks
preserved and newly added nested map keys, newer scalar values, array order and
duplicates, the exact network set and agreement with point lookups. A borrowed
scan then verifies sorted unique files/categories. An adaptive parallel inventory
is compared against it, and an explicit two-worker scan verifies the range count
even on this tiny fixture. All data is synthetic; no
database download, network traffic or filesystem output is needed.

The reader borrows only the bytes produced by this example's writer. Its
`files` and `categories` inventories use sorted `BTreeSet<&str>` collections;
duplicate field values are removed without category normalization or display
limits. Output is printed only after all scans and assertions succeed.
The example accepts no arguments and requires `reader,writer,derive`.

## Benchmark protocol

`make bench-scan` runs Criterion's `record-scan-v1` workload. It generates exactly
100,000 distinct IPv4 /24 routes and 100,000 distinct IPv6 /64 routes, each
containing `file`, `category` and `score` fields. Setup and reader construction
are excluded from timing. Both scans produce and verify the same route count
and checksum of all three fields plus the prefix length before measurement.

Generic scanning materializes the value map; typed scanning borrows its scalar
fields directly. Separate thread-local allocator samples count allocation and
reallocation requests plus requested bytes, not peak RSS or retained memory.
The allocator is a System-forwarding measurement wrapper, with no cross-thread
shared atomic counters. Its sampling is disabled for Criterion timing.

Criterion uses 20 samples, a one-second warmup and three seconds of measurement
per operation by default. The fixtures fix `build_epoch` to 1,700,000,000 and
are saved in `target/record-scan` for external SHA-256 verification. Native scans
are CPU/control-plane measurements and must not be presented as capture throughput, network latency or GPU performance.

## Parallel benchmark protocol

The same `make bench-scan` target also runs `parallel_record_scan_v1` for 1,000,
100,000 and 1,000,000 IPv4 /24 and IPv6 /64 routes. Each family/size compares the
original typed sequential visitor, explicit 1/2/4/8/16 workers, and the adaptive
API on exactly the same bytes. Before timing, all methods must match the route
count and checksum verified by both generic and typed scans. Fixtures have the
same deterministic epoch/content as record-scan-v1 and are cached under
`target/record-scan/ipv{version}-{routes}.mmdb`.

Thread creation, frontier construction, callbacks, worker joining and checksum
reduction are included in timing. Fixture construction, file reads/writes and
reader opening are excluded. Sequential timing accumulates directly in local
variables; parallel timing accumulates in thread-local state and publishes one
count/checksum per worker on thread exit, without per-record shared atomics.
The one-worker and adaptive fallback cases flush the caller's thread-local
state on return. Parallel reduction/thread state allocate once per worker, not
per record. No parallel allocator/RSS claim is derived from the original
single-thread allocator sampler.

Run only the new benchmark with:

```bash
cargo +1.98.1 bench --bench record_scan -- parallel_record_scan_v1 --noplot
```

Retain the same compiler, flags, fixture identities and CPU affinity for repeats.
Avoid other CPU-heavy work during measurement. A forced multithreaded small scan
can be slower because thread setup dominates; the adaptive API is intended to
avoid that crossover. Synthetic scalar results do not prove speedups for the
historical FireHOL array/set inventory or other arbitrary callbacks.

## Local validation (2026-10-06)

Validation used Rust 1.98.1 (48a229cea), x86_64 Linux 7.0.0-34-generic and an
AMD Ryzen 7 PRO 7840U. Criterion used Cargo's optimized bench profile with debug
information, default crate features and no additional RUSTFLAGS. Scan timing
was pinned to CPU 6. Preparation, file writes, opening and allocator samples
were outside measurement. The System-forwarding allocator wrapper remained
installed during timing with sampling disabled.

Final `taskset -c 6 make bench-scan` results on the fixed-epoch fixtures
(20 samples; interval columns are Criterion's 95% confidence intervals):

| Family | API | Full 100,000-range scan | 95% interval | Allocation requests | Requested bytes |
| --- | --- | --- | --- | --- | --- |
| IPv4 | Generic | 8.2823 ms | 8.2075–8.3430 ms | 100,000 | 14,400,000 |
| IPv4 | Typed borrowed scalars | 3.9562 ms | 3.9447–3.9677 ms | 0 | 0 |
| IPv6 | Generic | 9.0624 ms | 8.9305–9.2273 ms | 100,000 | 14,400,000 |
| IPv6 | Typed borrowed scalars | 4.5249 ms | 4.5005–4.5507 ms | 0 | 0 |

On these fixtures, typed scanning took about half the time of generic scanning.
This is not a speed claim for arbitrary schemas or for the FireHOL inventory:
array fields and sorted-set insertion perform additional work. A preliminary
30-sample confirmation measured roughly 12% lower typed scan time after replacing
the heap traversal stack and inlining schema-directed decoding. Preliminary
fixtures used the same route/value content but a time-based metadata epoch;
the final benchmark fixes that epoch. An earlier noisy repeat was not used to
claim a stable gain.

Fixture identities:

- IPv4: 5,484,492 bytes; SHA-256
  `43fba4d5fe6dc1b88b11183457c5ef8503d6e5e9d39f3d2cf5befa46ef1fc4b8`.
- IPv6: 5,484,732 bytes; SHA-256
  `689874afaf6c6bf72644a0e2ee0a6c743ef119ae909060323df50eefc879540b`.

Historical validation before file input was removed: the earlier example ran
with `reader,derive` against the public FireHOL release MMDB downloaded from
the repository's
`firehol-blocklist-ipsets` release. It completed a scan of **4,345,021 ranges**,
reporting **148 unique files** and **7 unique raw categories**. That input was
136,514,099 bytes with SHA-256
`164d9e105c3acbd0ab19c39987fe688df4dc88dc376faa55bc4a9bb52300cb7f`.
These counts describe this downloaded version, not all possible FireHOL lists.

Executed checks:

- `make help` and `make check`: passed, including the workspace suite, reader-only
  and writer-only tests, tooling tests, doctests, coverage and documentation.
  The workspace suite passed 177 tests; the example passed 3 additional tests.
- Identical Rust writer/scan examples in both README files: compiled and ran
  with `reader,writer,derive` only. Each builds a synthetic IPv4/IPv6 database,
  applies DeepMerge and asserts exact networks, array values and inventories.
- `unique_fields_scan_records`: the self-contained demo passed in debug and
  release with `reader,writer,derive`. The reader consumes only Writer output;
  command-line arguments are rejected before construction or scanning.
- `make build-release` and Cargo packaging/verification of both crates: passed.
- Strict workspace clippy (`-D warnings`) and isolated `cargo build` invocations
  for `reader`, `writer` and default features: passed.
- `cargo test --no-default-features --features reader,derive --test scan_allocations`:
  passed and verified no heap allocation for a complete borrowed scalar scan.
- `make fuzz`: passed 17,385 runs over 31 seconds with the scan included; this is
  bounded host parser fuzzing, not privileged eBPF validation.
- `make bench-performance`: initially failed because its expected datasets were
  absent. Its recipe now invokes the shared workload generator, including the
  million-route IPv6 absent fixture, before timing. The rerun passed both native
  reader benchmark suites.
- `make bench-scan`: passed; the results and measurement scope are recorded above.
- `make bench-compare`: blocked by an external tool limitation. Rust and C
  harnesses built, but the runner stopped at `go build failed: No such file or
  directory (os error 2)` because Go is unavailable. No cross-implementation
  measurements were produced or claimed.

All source changes and generated files were created as the project owner.
All builds, scans and tests ran as the project owner. No networking hooks or
captured metadata were used for this library validation. Temporary download
files were removed after validation; no root-owned files were found.

## Parallel implementation validation (2026-10-06, Xeon VM)

This branch was checked with Rust 1.98.1 (48a229cea), x86_64 Linux 6.18.44,
Intel Xeon Platinum 8573C, nine visible CPUs and a cgroup quota of eight CPUs.
These are separate measurements from the historical Ryzen results above.

- `make check`: passed, including strict all-target clippy, the workspace suite,
  reader-only/writer-only/reader+writer+derive isolation, tooling tests, 51
  doctests, coverage and documentation. The coverage updater counted 184 tests;
  total line coverage was 97.09%, with 94.10% in `src/reader/scan.rs`.
- The existing writer/scan example and its three tests passed with only
  `reader,writer,derive`. The identical README Rust snippets compiled and ran.
  Both adaptive and explicit two-worker scans are exercised by the example.
- Forced worker tests verified matching networks/values for borrowed, owned,
  file and mmap inputs, retained borrowed strings/bytes, IPv4-in-IPv6 ranges,
  shared payloads, /0 coverage, empty trees, non-Send records, real concurrent
  callbacks, joined workers after errors, panic propagation, reserved pointers,
  all pointer widths, cycles across frontier boundaries and global budget limits.
- `make bench-performance`: passed both native lookup/reader suites. This is a
  validation run, not a new cross-host lookup speedup claim.
- `ASAN_OPTIONS=detect_leaks=0 make fuzz`: passed 6,177 inputs in 31 seconds,
  with forced parallel traversal included. AddressSanitizer remained enabled.
  LeakSanitizer was disabled because this container cannot inspect task threads
  through ptrace; the initial run completed input fuzzing but failed in that
  environment-specific leak checker at shutdown.

## Repeated parallel scan results

The first pass measured all 42 size/family/method combinations; an independent
second pass repeated all 28 combinations at 100k and 1M routes. Both used CPU
affinity `0-7`, the same compiler/profile/default features, no additional
RUSTFLAGS, and no other validation builds or benchmark processes during timing.
The adaptive API used eight workers on this affinity. Each case had 20 samples,
a one-second warmup and a three-second target measurement time; Criterion
extended collection where necessary to retain the requested sample count.

All scan methods verified the same count/checksum before timing. These mean
full-scan times include thread setup, subtree splitting, callbacks and reduction.

| Routes | Family | Sequential, pass 1 | Adaptive, pass 1 | Sequential, pass 2 | Adaptive, pass 2 | Speedup across passes |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 100,000 | IPV4 | 5.057 ms | 1.552 ms | 5.113 ms | 1.560 ms | 3.26–3.28x |
| 100,000 | IPV6 | 6.033 ms | 1.787 ms | 5.988 ms | 1.756 ms | 3.38–3.41x |
| 1,000,000 | IPV4 | 53.523 ms | 12.924 ms | 53.903 ms | 12.648 ms | 4.14–4.26x |
| 1,000,000 | IPV6 | 59.726 ms | 14.372 ms | 59.686 ms | 14.124 ms | 4.16–4.23x |

Pass 2 mean estimates and bootstrap 95% confidence intervals:

| Routes | Family | Sequential mean (95% CI), ms | Adaptive mean (95% CI), ms |
| --- | --- | ---: | ---: |
| 100,000 | IPV4 | 5.113 (5.089–5.140) | 1.560 (1.536–1.583) |
| 100,000 | IPV6 | 5.988 (5.881–6.169) | 1.756 (1.712–1.804) |
| 1,000,000 | IPV4 | 53.903 (53.467–54.392) | 12.648 (12.484–12.843) |
| 1,000,000 | IPV6 | 59.686 (59.392–59.989) | 14.124 (13.975–14.310) |

Worker scaling, pass 2 (mean full-scan times, ms):

| Routes | Family | Sequential | 1 worker | 2 workers | 4 workers | 8 workers | 16 workers |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 100,000 | IPV4 | 5.113 | 5.860 | 3.510 | 1.998 | 1.537 | 2.140 |
| 100,000 | IPV6 | 5.988 | 6.690 | 4.070 | 2.326 | 1.825 | 2.327 |
| 1,000,000 | IPV4 | 53.903 | 61.600 | 36.369 | 22.984 | 12.634 | 12.873 |
| 1,000,000 | IPV6 | 59.686 | 72.115 | 43.392 | 21.918 | 14.285 | 14.390 |

Eight workers won over sixteen here; sixteen oversubscribes the eight-CPU
budget. For 1k routes (first pass), explicit eight-worker scans took
0.435/0.401 ms (IPv4/IPv6), versus 0.055/0.060 ms sequentially. The adaptive path
used no worker threads there. Its benchmark still pays thread-local callback
bookkeeping, unlike the directly accumulated sequential reference; compare
operation definitions before attributing that overhead to traversal itself.
These passes establish gains at 100k nodes, but do not locate the crossover.
The finer measurements below replace the initial 100k-node default with 24k;
neither set establishes a universal optimum or a measured FireHOL speedup.

A separate CPU-0 A/B compared the existing 100k sequential APIs against base
commit `ba0bc348a3ff0b8ee1826a38cde09a60fd8db268`. Typed IPv4 showed no detected
change; typed IPv6 was slightly faster in that pass. Generic IPv6 initially
showed +4.34% mean time. A 40-sample confirmation measured +2.26%
(95% change interval +1.61% to +2.87%), inside the repository's ±3% screening
band, though Criterion classified it as statistically significant at its 1%
threshold. Keep this small sequential tradeoff visible; the screening band is
not proof of equivalence. Generic IPv4 showed no detected change. The parallel
API's 3.26–4.26x gains are much larger than this sequential variation.

Raw per-sample iteration counts/times, mean/median/slope estimates with confidence
intervals, source hashes, CPU/compiler/affinity information, and all six fixture
SHA-256 identities are retained in
[`benchmarks/parallel-record-scan-2026-10-06.json`](benchmarks/parallel-record-scan-2026-10-06.json).
The pinned sequential before/current samples are also retained there; the generic
IPv6 current case is the 40-sample confirmation. No parallel peak-RSS estimate is
inferred from thread counts or the single-thread allocator sampler.

Reproduce the two parallel passes with:

```bash
taskset -c 0-7 cargo +1.98.1 bench --bench record_scan -- \
  parallel_record_scan_v1 --save-baseline parallel-run-1 --noplot
taskset -c 0-7 cargo +1.98.1 bench --bench record_scan -- \
  'parallel_record_scan_v1/(100000|1000000)/' --save-baseline parallel-run-2 --noplot
```

## Measured crossover on smaller trees

The initial 100k-node cutoff was conservative and had not been measured between
1k and 100k. `parallel_record_scan_crossover_v1` fills that gap with 1k, 2k, 4k,
8k, 12k, 16k, 24k, 32k and 64k stored routes, for IPv4 and IPv6. It compares
direct sequential scanning, explicit 1/2/4/8 workers and the adaptive API.
The same deterministic scalar schema, compiler 1.98.1, default release features
and Xeon VM were used. Each case has 30 samples, a 500 ms warmup and a two-second
target measurement time. Thread creation, subtree splitting, decoding, callbacks
and reduction are included; fixture generation, opening and count/checksum
verification are excluded. No validation builds ran during measurement.

The first pass and an independent repeat used affinity `0-7`. First-pass mean
full-scan times, in milliseconds:

| Routes | Family | Direct sequential | 2 workers | 4 workers | 8 workers |
| --- | --- | ---: | ---: | ---: | ---: |
| 1,000 | IPv4 | 0.056 | 0.134 | 0.206 | 0.491 |
| 1,000 | IPv6 | 0.057 | 0.117 | 0.223 | 0.501 |
| 4,000 | IPv4 | 0.224 | 0.254 | 0.321 | 0.466 |
| 4,000 | IPv6 | 0.232 | 0.242 | 0.307 | 0.475 |
| 8,000 | IPv4 | 0.391 | 0.379 | 0.395 | 0.522 |
| 8,000 | IPv6 | 0.458 | 0.428 | 0.382 | 0.523 |
| 12,000 | IPv4 | 0.612 | 0.536 | 0.422 | 0.684 |
| 12,000 | IPv6 | 0.689 | 0.565 | 0.447 | 0.669 |
| 16,000 | IPv4 | 0.786 | 0.756 | 0.628 | 0.902 |
| 16,000 | IPv6 | 0.892 | 0.733 | 0.532 | 0.648 |
| 24,000 | IPv4 | 1.174 | 0.943 | 0.680 | 0.820 |
| 24,000 | IPv6 | 1.379 | 1.348 | 0.893 | 0.908 |
| 32,000 | IPv4 | 1.610 | 1.250 | 1.004 | 3.552 |
| 32,000 | IPv6 | 1.787 | 1.462 | 0.959 | 0.953 |
| 64,000 | IPv4 | 3.175 | 2.664 | 1.540 | 1.460 |
| 64,000 | IPv6 | 3.659 | 3.539 | 1.797 | 1.551 |

The repeat measured every sequential/2/4/8-worker case at 16k, 24k, 32k and 64k
routes. At 24k, eight workers beat direct sequential scanning in both passes
and both families; all four mean confidence intervals are separated:

| Family | Pass | Sequential mean (95% CI), ms | 8-worker mean (95% CI), ms | Speedup |
| --- | --- | ---: | ---: | ---: |
| IPv4 | First | 1.174 (1.165–1.186) | 0.820 (0.790–0.859) | 1.43x |
| IPv4 | Repeat | 1.187 (1.181–1.193) | 0.857 (0.807–0.911) | 1.39x |
| IPv6 | First | 1.379 (1.342–1.438) | 0.908 (0.862–0.959) | 1.52x |
| IPv6 | Repeat | 1.398 (1.356–1.456) | 0.892 (0.861–0.928) | 1.57x |

**The adaptive cutoff is now 24,000 tree nodes.** The 24k-route fixtures contain
24,011 IPv4 / 24,051 IPv6 nodes; the 16k fixtures contain 16,011 / 16,051 nodes.
This is the lowest tested point with repeatable eight-worker gains in both
families, rather than a precise mathematical crossover. Four workers can already
help around 12k nodes, so explicit worker limits remain useful below the cutoff.
Sixteen thousand nodes were not consistently beneficial with eight workers:
IPv4 was slower in the first pass and faster in the repeat.

The 32k IPv4 eight-worker slowdown is retained above: 3.552 ms in the first pass
versus 1.000 ms in the repeat (sequential: 1.610 / 1.598 ms). Its cause was not
isolated. A fixed size threshold cannot prevent every scheduling-related
regression; these results do not guarantee a gain on other machines, sparse or
aliased trees, container schemas or contended callbacks. Node count is only a
proxy for useful scan work. The one-worker cases use the same TLS callback as
the multi-worker cases, while the direct sequential reference uses cheaper local
accumulation; their raw samples are retained to make this distinction explicit.

All fixture identities, actual node counts, count/checksum verification results,
source hashes, per-sample times and confidence intervals are retained in
[`benchmarks/parallel-record-scan-crossover-2026-10-06.json`](benchmarks/parallel-record-scan-crossover-2026-10-06.json).

Reproduce the crossover grid and repeat with:

```bash
taskset -c 0-7 make bench-scan-crossover CARGO='cargo +1.98.1'
taskset -c 0-7 cargo +1.98.1 bench --bench record_scan -- \
  'parallel_record_scan_crossover_v1/(16000|24000|32000|64000)/(sequential|workers-(2|4|8))/' \
  --save-baseline crossover-repeat --noplot
```

The archived first-pass adaptive cases used the previous 100k cutoff. The
explicit-worker methods bypass the cutoff, so they measure the crossover
independently of the heuristic being evaluated.

After lowering the cutoff, a separate adaptive-API pass measured 1k, 16k, 24k
and 32k routes with eight available CPUs. Additional 1/2/4-CPU affinity passes
measured 16k, 24k and 32k. At 24k routes, the adaptive API was faster with each
tested multi-CPU budget, with separated mean confidence intervals:

| Available CPUs | Family | Sequential mean (95% CI), ms | Adaptive mean (95% CI), ms | Speedup |
| --- | --- | ---: | ---: | ---: |
| 2 | IPv4 | 1.184 (1.176–1.192) | 0.978 (0.948–1.009) | 1.21x |
| 2 | IPv6 | 1.577 (1.549–1.608) | 1.007 (0.968–1.048) | 1.57x |
| 4 | IPv4 | 1.333 (1.275–1.400) | 0.741 (0.679–0.813) | 1.80x |
| 4 | IPv6 | 1.370 (1.352–1.395) | 0.688 (0.672–0.709) | 1.99x |
| 8 | IPv4 | 1.194 (1.179–1.214) | 0.932 (0.850–1.030) | 1.28x |
| 8 | IPv6 | 1.558 (1.458–1.681) | 0.856 (0.824–0.888) | 1.82x |

With one CPU, the adaptive API remains sequential even above the cutoff. At 24k
its TLS-based benchmark callback took 1.530 / 1.600 ms (IPv4 / IPv6), versus
1.202 / 1.423 ms for direct local accumulation. This callback-cost difference
also exists below the cutoff; it is not a thread-creation regression. The
integration test checks callback thread identity and total record count both
below and above the cutoff, including when run with one CPU.

Reproduce the post-change adaptive checks with:

```bash
taskset -c 0-7 cargo +1.98.1 bench --bench record_scan -- \
  'parallel_record_scan_crossover_v1/(1000|16000|24000|32000)/(sequential|adaptive)/' \
  --save-baseline crossover-confirm --noplot
# Repeat with affinities 0-1, 0-3 and 0, using a distinct baseline for each run.
```

Lowering the cutoff opts more medium-sized trees into bounded thread/frontier
allocations in exchange for lower scan latency; records still stream and borrow
the reader. No lookup-tree layout or decoder changes were made for this tuning.

Validation after tuning: `make check` passed formatting, Clippy, 185 workspace
tests, 51 doctests, isolated-feature/tooling tests and rustdoc. Line coverage was
97.05% overall and 94.46% for the scan module. The adaptive integration test also
passed under affinity `0`, verifying the one-CPU fallback above the cutoff.
`make bench-performance` completed all 24 reader/borrowed-lookup cases.
`make bench-compare` remains unavailable: after bypassing this container's tar
ownership restriction with `TAR_OPTIONS=--no-same-owner`, the C dependency's
configure step failed because `pkg-config` is missing. No new cross-library
comparison result is claimed.
