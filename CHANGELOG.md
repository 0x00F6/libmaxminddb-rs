# Changelog

All notable changes to this project are documented here. This project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

- Prepared crate metadata, package contents, documentation, and CI for public distribution.
- Removed the unused, hidden `map_get` re-export; use `ValueRef::get` directly.
- Fixed generic byte-slice tree traversal for non-IPv4/IPv6 lengths so it reads each bit in order.

## [0.1.0] - Unreleased

- Initial MMDB v2 reader and writer with borrowed decoding, IPv4/IPv6 lookup, and derived record support.
