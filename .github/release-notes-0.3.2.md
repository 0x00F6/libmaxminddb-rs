## 🦾 ARM64 NEON build fix

- Fix compilation of the NEON metadata marker scanner with Rust 2024 and the default SIMD feature on AArch64.
- Document the safety of the vector-to-byte-array conversion and compile the x86-specific candidate helper only on x86_64.
- Run the existing CI checks on both native ARM64 and x86_64: formatting, Clippy, tests, feature isolation, examples, documentation and crate packaging.

The public API and MMDB format are unchanged. The fix was also validated with native ARM64 `make check`, parser fuzzing and the complete comparative benchmark suite.

Both `libmaxminddb-rs` and `libmaxminddb-rs-derive` use version `0.3.2`; the derive macros are unchanged.

**Full changelog:** https://github.com/0x00F6/libmaxminddb-rs/compare/v0.3.1...v0.3.2
