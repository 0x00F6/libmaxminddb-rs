# libmaxminddb-rs-derive

Procedural derive macros for [`libmaxminddb-rs`](https://docs.rs/libmaxminddb-rs). This crate is normally enabled through the main crate's default `derive` feature; applications do not need to depend on it directly.

- `MmdbDecode` decodes typed records from MMDB maps.
- `MmdbEncode` encodes struct fields for the writer.
- `MmdbRecord` reads a network field marked `#[mmdb(network)]` for one-object insertion.

Minimum supported Rust version: 1.90. Licensed under MIT or Apache-2.0, at your option.
