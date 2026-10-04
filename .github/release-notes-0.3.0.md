## 🔄 Hot In-Memory Database Updates

- Added `Editor::from_reader` and ordered updates/deletions to rebuild existing MMDB databases.
- Added `ReloadableReader` powered by `ArcSwap` for concurrent lookups and atomic in-memory database updates without a global read/write lock.
- Reject stale editor commits with compare-and-swap to prevent lost updates. Release owned old database buffers after the last reader guard, snapshot, and editor is dropped.
- **Breaking:** `Editor::update_value(network, value, strategy)` now requires an explicit `MergeStrategy`. Use `MergeStrategy::Replace` for replacement or `MergeStrategy::DeepMerge` for recursive merging on exported exact prefixes.
- Accept owned `Value` inputs and borrowed custom records deriving `MmdbEncode` through `IntoMmdbValue`.
- Added typed borrowed lookup examples, concurrent publication and memory reclamation tests, and million-IP editor benchmarks.


See the runnable `concurrent_editor` and `editor_merge` examples. Old generations remain alive while queries hold their guards or snapshots; owned buffers are reclaimed after the last reference is released.

**Full changelog:** https://github.com/0x00F6/libmaxminddb-rs/compare/v0.2.1...v0.3.0
