## Parallel borrowed record scans

- Add `Reader::visit_borrowed_records_parallel`, using available CPUs for trees with at least 24,000 nodes and sequential traversal for smaller trees or one CPU.
- Add `visit_borrowed_records_parallel_with_workers(NonZeroUsize, visitor)` to set a worker limit and bypass the size heuristic.
- Keep records streaming: strings and bytes borrow the reader, and decoded `T` needs neither `Send` nor `Sync`. Scalar decoding avoids per-record allocations; thread setup and bounded scheduling allocate.
- Callbacks require `Fn + Sync` and run in unspecified order. Cancellation is cooperative, with all started workers joined before returning or propagating a panic.
- Extend the self-contained scan example, corruption/cancellation tests, fuzz target and reproducible IPv4/IPv6 benchmarks.

## Measured performance

Repeated synthetic scalar scans on an eight-CPU Xeon VM measured **3.26–3.41x speedups at 100k ranges** and **4.14–4.26x at 1M ranges**, including thread setup. A finer crossover study lowered the initial 100k-node threshold to **24k nodes**, with gains confirmed at that size on 2, 4 and 8 CPUs.

The crossover depends on the machine, tree, schema and callback. Full samples, confidence intervals, fixture hashes, scheduling variability and the small sequential generic-IPv6 tradeoff are documented in the [scan results](https://github.com/0x00F6/libmaxminddb-rs/blob/v0.5.0/docs/record-scan.md).

Both `libmaxminddb-rs` and `libmaxminddb-rs-derive` use version `0.5.0`; the derive macros are unchanged. Existing lookup APIs and the MMDB format are unchanged.

**Full changelog:** https://github.com/0x00F6/libmaxminddb-rs/compare/v0.4.0...v0.5.0
