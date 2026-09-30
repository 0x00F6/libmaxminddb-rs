# Changelog

All notable changes to this project are documented here. This project follows [Semantic Versioning](https://semver.org/).

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
