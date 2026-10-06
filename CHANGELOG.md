# Changelog

All notable changes to this project are documented here. This project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.4.0] - 2026-10-06

- Exposed `Reader::visit_records` with the reader feature alone for checked,
  streaming scans of reachable network/value pairs.
- Added `Reader::visit_borrowed_records` for schema-directed borrowed scans,
  retaining source strings without generic map/array materialization.
- Reject cycles, over-deep trees and excessive DAG expansion during scans;
  preserve callback errors, prefix semantics and decoder resource limits.
- Added the self-contained `unique_fields_scan_records` DeepMerge writer/scan
  example and matching README samples, with IPv4/IPv6 assertions, reader-only
  API regression coverage, scan fuzzing and deterministic Criterion benchmarks.
  The example reads only its writer's in-memory output and requires
  `reader,writer,derive`; scan APIs remain available with `reader` alone.
- Prepare the shared reader benchmark datasets before `make bench-performance`,
  including verified million-route IPv6 misses, without requiring competitor tools.

## [0.3.1] - 2026-10-04

- Fixed escaped Markdown code fences and inline code in the crates.io README.
- Condensed Editor documentation into one complete example with visible `main`, English comments, typed DeepMerge updates and old database reclamation.
- Moved More examples to the end of both READMEs and synchronized the runnable example.

## [0.3.0] - 2026-10-04

- Added `Editor::from_reader` and ordered updates/deletions to rebuild existing MMDB databases.
- Added `ReloadableReader` powered by `ArcSwap` for concurrent lookups and atomic in-memory database updates without a global read/write lock.
- Reject stale editor commits with compare-and-swap to prevent lost updates. Release owned old database buffers after the last reader guard, snapshot, and editor is dropped.
- **Breaking:** `Editor::update_value(network, value, strategy)` now requires an explicit `MergeStrategy`. Use `MergeStrategy::Replace` for replacement or `MergeStrategy::DeepMerge` for recursive merging on exported exact prefixes.
- Accept owned `Value` inputs and borrowed custom records deriving `MmdbEncode` through `IntoMmdbValue`.
- Added typed borrowed lookup examples, concurrent publication and memory reclamation tests, and million-IP editor benchmarks.

## [0.2.1] - Unreleased

- Upgraded Cargo dependencies and aligned the main crate, CI, and Docker benchmarks with Rust 1.98.1.
- Reworked the Makefile with grouped, colored help; consolidated test and coverage checks; and added dependency upgrades with rollback on failure.
- Combined release and crate publication in `make release X.Y.Z`, including version synchronization and package validation.
- Updated GeoLite2 examples to download the latest Country, City, and ASN databases directly, and refreshed the README examples.
- Prepared crate metadata, package contents, documentation, and CI for public distribution.
- Removed the unused, hidden `map_get` re-export; use `ValueRef::get` directly.
- Fixed generic byte-slice tree traversal for non-IPv4/IPv6 lengths so it reads each bit in order.

## [0.1.0] - Unreleased

- Initial MMDB v2 reader and writer with borrowed decoding, IPv4/IPv6 lookup, and derived record support.
