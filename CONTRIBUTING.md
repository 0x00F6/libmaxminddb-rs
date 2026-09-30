# Contributing

Thank you for helping improve `libmaxminddb-rs`. Please open an issue before making a large API or MMDB format change so the compatibility and performance implications can be discussed.

## Local development

Install Rust 1.98.1 or newer. The repository's `rust-toolchain.toml` selects the stable toolchain for local development; CI pins Rust 1.98.1.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --doc --all-features
```

To upgrade dependencies across all Cargo projects, install `cargo-edit` and run
`make upgrade-deps`. The command also updates lockfiles and runs the full
test suite. On failure, it restores the manifests and lockfiles and keeps the
output in `target/upgrade-deps.log`.

For changes to the reader or writer hot paths, record a baseline first and rerun `make bench-performance` and `make bench-writer` after the change. Keep the fixture, build flags, and measurement protocol identical. See [AGENTS.md](AGENTS.md) for format invariants and parser safety requirements.

## Pull requests

Keep changes focused, document public API changes, add correctness coverage for behavior changes, and report the commands you ran. Benchmark results should include the environment and any memory or safety tradeoffs. The CI checks formatting, clippy, tests, docs, and package contents.

## Releases

Update `CHANGELOG.md` when appropriate. Run `make release X.Y.Z` to synchronize both crate versions, tracked `Cargo.lock` files, and the installation examples in the Markdown files. If it changes files, commit them and rerun the same command. The release validates both packages, pushes `main`, publishes `libmaxminddb-rs-derive` and then `libmaxminddb-rs` to crates.io, and creates the Git tag and GitHub Release. Use `make publish-check` to validate the packages without publishing.
