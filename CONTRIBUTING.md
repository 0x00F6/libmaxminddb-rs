# Contributing

Thank you for helping improve `libmaxminddb-rs`. Please open an issue before making a large API or MMDB format change so the compatibility and performance implications can be discussed.

## Local development

Install Rust 1.90 or newer. The repository's `rust-toolchain.toml` selects the tested toolchain.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --doc --all-features
```

For changes to the reader or writer hot paths, record a baseline first and rerun `make bench-performance` and `make bench-writer` after the change. Keep the fixture, build flags, and measurement protocol identical. See [AGENTS.md](AGENTS.md) for format invariants and parser safety requirements.

## Pull requests

Keep changes focused, document public API changes, add correctness coverage for behavior changes, and report the commands you ran. Benchmark results should include the environment and any memory or safety tradeoffs. The CI checks formatting, clippy, tests, docs, and package contents.

## Releases

Update `CHANGELOG.md` and both crate versions when appropriate. Run `make publish-check` to validate the packages locally. The `libmaxminddb-rs-derive` package must reach crates.io before publishing the main crate. `make publish` performs that sequence; `make release` creates the Git tag and GitHub Release after checking for a clean tree.
