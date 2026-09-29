# Examples for `libmaxminddb-rs`

This directory provides practical, self-contained examples demonstrating how to use `libmaxminddb-rs` to build, inspect, and query MaxMind DB (MMDB) files in pure Rust.

## Overview of Examples

| Example | Description | Key APIs Used |
| :--- | :--- | :--- |
| **[`quickstart.rs`](quickstart.rs)** | Complete 2-minute introductory example: build in memory, insert records, and query. | `Writer`, `Reader`, `MmdbEncode`, `MmdbDecode` |
| **[`zero_copy_lookup.rs`](zero_copy_lookup.rs)** | Borrowed strings, optional lookup, and generic values. Generic maps still allocate container vectors. | `lookup_borrowed`, `lookup_borrowed_opt`, `lookup_value`, `lookup_value_with_prefix` |
| **[`fast_ip_lookup.rs`](fast_ip_lookup.rs)** | Fast IPv4 and IPv6 hit/miss loops that decode only hits and return a small value. | `Reader::lookup_borrowed_map` |
| **[`lookup_with_copy.rs`](lookup_with_copy.rs)** | Owned serde and derived records, plus explicit copies of borrowed generic results. | `lookup`, owned `lookup_borrowed` variants, `lookup_value`, `lookup_value_with_prefix`, `lookup_many` |
| **[`lookup_exists.rs`](lookup_exists.rs)** | Check whether an IP is covered without decoding a record. | `Reader::lookup_exists` |
| **[`lookup_many.rs`](lookup_many.rs)** | High-throughput batch IP lookups with automatic sequential/multi-threaded parallel scaling. | `Reader::lookup_many`, `ValueRef` |
| **[`concurrent_lookups.rs`](concurrent_lookups.rs)** | Lock-free multi-threaded lookup across threads with zero mutex/rwlock contention. | `Arc<Reader>`, `std::thread::scope`, lock-free lookups |
| **[`custom_database.rs`](custom_database.rs)** | Advanced database building with IPv4/IPv6 subnets, metadata configuration, and disk I/O. | `MetadataBuilder`, `Writer::write_to_file`, `Reader::open_mmap` |
| **[`custom_record.rs`](custom_record.rs)** | Ergonomic insertion using `#[derive(MmdbRecord)]` and `#[mmdb(network)]`. | `MmdbRecord`, `Writer::insert_entry` |
| **[`metadata.rs`](metadata.rs)** | Inspecting database headers, node counts, record sizes, languages, and build timestamps. | `Reader::metadata`, `Metadata` |
| **[`read_write.rs`](read_write.rs)** | Using decoupled write and read structures (`StoredRecord` vs `GeoRecord`). | `MmdbEncode`, `MmdbDecode`, `Reader::from_bytes` |
| **[`geolite2_country.rs`](geolite2_country.rs)** | Downloads GeoLite2 Country when needed and performs a borrowed typed lookup. | `Reader::open`, `lookup_borrowed`, `MmdbDecode` |
| **[`geolite2_city.rs`](geolite2_city.rs)** | Downloads GeoLite2 City when needed and prints a borrowed city record. | `Reader::open`, `lookup_borrowed`, `MmdbDecode` |
| **[`geolite2_asn.rs`](geolite2_asn.rs)** | Downloads GeoLite2 ASN under a distinct filename and performs a borrowed ASN lookup. | `Reader::open`, `lookup_borrowed`, `MmdbDecode` |
| **[`deep_merge.rs`](deep_merge.rs)** | Multi-source dataset enrichment with recursive deep merge on overlapping subnets. | `MergeStrategy::DeepMerge`, `Writer::insert` |
| **[`generate_compat_db.rs`](generate_compat_db.rs)** | Generating standard MMDB fixtures verified across other MMDB language implementations. | `Writer::insert`, `serde_json::json!` |

---

## Running the Examples

You can run any example directly using Cargo:

### 1. Quickstart
```bash
cargo run --example quickstart
```

### 2. Zero-Copy Borrowed Lookups
```bash
cargo run --example zero_copy_lookup
```

### 3. High-Throughput Batch Lookups (`lookup_many`)
```bash
cargo run --example lookup_many
```

For latency-sensitive IPv4 and IPv6 loops with a pre-opened reader and pre-parsed addresses:

```bash
cargo run --release --example fast_ip_lookup
```

For owned lookup results, explicit copies, or tree-only queries, run the focused examples:

```bash
cargo run --example lookup_with_copy
cargo run --example lookup_exists
```

The GeoLite2 Country example stores the downloaded database under `target/database` and reuses it on later runs:

```bash
cargo run --example geolite2_country
cargo run --example geolite2_city
cargo run --example geolite2_asn
```

### 4. Concurrent Multi-Threaded Lookups
```bash
cargo run --example concurrent_lookups
```

### 5. Custom Database Builder (IPv4 & IPv6)
```bash
cargo run --example custom_database
```

### 6. Custom Record Derive (`MmdbRecord`)
```bash
cargo run --example custom_record
```

### 7. Metadata Inspection
```bash
cargo run --example metadata
```

### 8. Read & Write with Decoupled Models
```bash
cargo run --example read_write
```

### 9. Multi-Source Dataset Enrichment (Deep Merge)
```bash
cargo run --example deep_merge
```

### 10. Generate Compatibility Fixture
```bash
cargo run --example generate_compat_db -- target/compat.mmdb
```
