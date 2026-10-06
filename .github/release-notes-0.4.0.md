## Checked MMDB record scans

- Expose `Reader::visit_records` with the `reader` feature alone to visit stored network ranges and borrowed values.
- Add `Reader::visit_borrowed_records` for typed decoding directly from the reader buffer. Borrowed scalar scans avoid generic map/array containers; arrays and inventory sets still allocate.
- Share bounded traversal with cycle, depth, pointer and expansion checks, preserving callback errors and IPv4/IPv6 prefix semantics.
- Add reader-only regression tests, borrowed-string and allocation checks, bounded scan fuzzing, and deterministic generic/typed Criterion benchmarks.
- Fix `make bench-performance` to prepare its shared datasets, including verified IPv6 misses.

## Self-contained example

`cargo run --example unique_fields_scan_records` builds a synthetic IPv4/IPv6 MMDB with `DeepMerge`, scans only its writer's in-memory output, and verifies merged values, network ranges and unique files/categories with assertions.

See [scan documentation](https://github.com/0x00F6/libmaxminddb-rs/blob/v0.4.0/docs/record-scan.md) for ownership boundaries, limits and locally measured benchmarks.

The existing lookup APIs and MMDB format are unchanged. Both `libmaxminddb-rs` and `libmaxminddb-rs-derive` use version `0.4.0`; the derive macros are unchanged.

**Full changelog:** https://github.com/0x00F6/libmaxminddb-rs/compare/v0.3.2...v0.4.0
