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
scan then verifies sorted unique files/categories. All data is synthetic; no
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
