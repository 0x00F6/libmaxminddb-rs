<p align="center">
  <img src="docs/images/ferris-maxmind.png" alt="Ferris MaxMind" width="480">
</p>

# 🦀 libmaxminddb-rs

<p align="center">
  <a href="https://crates.io/crates/libmaxminddb-rs"><img src="https://img.shields.io/badge/crates.io-libmaxminddb--rs-orange" alt="crates.io"></a>
  <a href="https://docs.rs/libmaxminddb-rs"><img src="https://img.shields.io/badge/API-documentation-blue" alt="API documentation"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="MIT OR Apache-2.0"></a>
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/Rust-1.98.1%2B-informational" alt="Rust 1.98.1+"></a>
</p>

<p align="center">
  <a href="#code-coverage--tests"><img src="https://img.shields.io/badge/tests-141%20passing-brightgreen" alt="141 workspace tests passing"></a>
  <a href="#code-coverage--tests"><img src="https://img.shields.io/badge/code%20coverage-97.55%25-brightgreen" alt="97.55% line coverage"></a>
</p>

An independent Rust implementation of the MaxMind DB (MMDB) v2 format. It reads IPv4/IPv6 databases with borrowed decoding and writes deterministic MMDB files.

---

## 📋 Table of Contents

- [🚀 Features](#-features)
- [⚡ Architecture & Optimizations](#-architecture--optimizations)
- [📦 Installation & Cargo Features](#-installation--cargo-features)
- [🚀 Quickstart Guide](#-quickstart-guide)
  - [🔍 1. Reading & Zero-Copy Lookups](#-1-reading--zero-copy-lookups)
  - [Lookup API Reference](#lookup-api-reference)
  - [✍️ 2. Database Creation & Serialization](#️-2-database-creation--serialization)
  - [🔀 3. Merging Databases (Deep Merge)](#-3-merging-databases-deep-merge)
- [📊 Benchmarks](#-benchmarks)
  - [⚙️ Evaluated Libraries & Reproducibility Specification](#️-evaluated-libraries--reproducibility-specification)
  - [🏁 Reader Performance Summary — p99 Tail Latency & Peak Throughput](#-reader-performance-summary--p99-tail-latency--peak-throughput)
  - [📈 Visual Benchmark Charts](#-visual-benchmark-charts)
- [🔥 FlameGraph](#-flamegraph)
- [Documentation and compatibility](#documentation-and-compatibility)
- [Development and tests](#development-and-tests)
- [Code Coverage & Tests](#code-coverage--tests)
- [Contributing](#contributing)
- [License](#license)

---

## 🚀 Features

- **Full MMDB v2 Specification** — Independent, pure-Rust implementation of the MaxMind DB v2 format for both IPv4 and IPv6 network lookups, without external C dependencies or wrapper overhead.
- **Zero-Copy Deserialization** — Derive-based deserialization (`#[derive(MmdbDecode)]`) that borrows string (`&'a str`) and binary (`&'a [u8]`) slices directly from the database buffer without heap allocations.
- **Flexible Lookup APIs**:
  - `lookup_borrowed`: Strongly typed, single-pass zero-copy decoding into user structs.
  - `lookup_borrowed_opt`: Borrowed decoding with an `Option` result for miss-heavy workloads.
  - `lookup_borrowed_map`: Borrowed decoding with a small callback result and `None` on a miss.
  - `lookup_value`: Generic dynamic inspection via borrowed `ValueRef` trees.
  - `lookup_value_with_prefix`: Longest-prefix matching returning both data and matched CIDR prefix length (subnet mask).
  - `lookup`: Owned deserialization through serde.
  - `lookup_many`: Order-preserving batch lookup with adaptive multithreading for bulk IP resolution.
  - `lookup_exists`: Check whether an address matches a record without decoding it.
- **Database Creation & Writer** — High-performance in-memory trie builder producing standard-compliant MMDB v2 binary files with automatic payload deduplication and optimal pointer widths (24, 28, or 32 bits).
- **Multi-Source Dataset Merging** — Built-in conflict resolution strategies for overlapping CIDR blocks (`Replace`, `Append`, `AppendUnique`, and recursive `DeepMerge`).
- **Ergonomic Derive Macros** — `#[derive(MmdbDecode, MmdbEncode, MmdbRecord)]` with `#[mmdb(network)]` attribute support for one-object database insertion and zero-boilerplate struct mapping.
- **Flexible Storage Backends** — Load databases from owned memory buffers (`Reader::open`), borrowed byte slices (`Reader::from_bytes`), or memory-mapped files (`Reader::open_mmap`).

---

## ⚡ Architecture & Optimizations

### 1. Zero-Copy & Allocation Avoidance
- **Lifetime-Bound Borrowing**: `lookup_borrowed` projects wire bytes straight into derived structs without intermediate `ValueRef` trees; string and byte fields borrow the reader buffer directly, avoiding `String` and `Vec<u8>` allocations.
- **Compact Miss Path**: Tree traversal represents misses without constructing a decoded record.
- **Arena-Based Trie Builder**: The writer stores trie nodes contiguously in a flat `Vec<TrieNode>` arena using `u32` index references, eliminating individual heap-allocated pointer indirections (`Box`).
- **Lower Writer Peak Memory**: Replacing an existing prefix keeps the new shared `Arc<Value>` without cloning its payload, and finalization releases trie nodes before allocating the output buffer.

### 2. Search Tree & Traversal Optimizations
- **Specialized Unaligned Node Traversal**: Unaligned 64-bit word loads (`load!`) read each node in a single instruction, with dedicated branch-free decoding loops tailored for 24-bit, 28-bit, and 32-bit pointer layouts.
- **Cache-Line Aligned Fast Tree**: Opening a reader prepares a native-endian search tree aligned to 64-byte boundaries (`AlignedNodes`), packing 8 nodes per CPU cache line. Radix and byte-stride tables are built at the same time, so the first lookup does not construct an index. This trades open time and reader memory for lower steady-state lookup latency.
- **Radix Accelerator Tables**: Root-level lookup tables index the first 8, 16, or 20 bits of IPv4 and IPv6 prefixes, reducing dependent pointer-chasing iterations.
- **CPU Cache Prefetching**: Targeted `_mm_prefetch` instructions warm CPU cache lines ahead of sequential and multi-step tree traversals on supported architectures.

### 3. Vectorization & SIMD
- **Hardware-Accelerated ASCII Probing**: Vectorized SIMD scanners (AVX2 / SSE2 on x86_64, NEON on AArch64) validate ASCII strings in bulk before UTF-8 decoding.
- **Out-of-Line Non-ASCII Fallback**: Payloads with non-ASCII bytes divert to an out-of-line `#[cold]` validator, keeping standard library UTF-8 validation routines out of the hot instruction stream.

### 4. Concurrency & Throughput
- **Shared Reader**: Prepared tree indexes are initialized once and shared across lookup threads.
- **Adaptive Batch Parallelism**: `lookup_many` dynamically selects sequential execution for smaller workloads and chunked parallel processing via `std::thread::scope` for large batches ($\ge 4,096$ IPs).

### 5. Memory-Mapped File Access & Safety
- **Direct Kernel Paging (`open_mmap`)**: Memory-maps database files to leverage OS page caches, reducing startup overhead and memory footprint across shared processes.
- **Branch-Guarded Bounds Hardening**: Unchecked slice accesses on hot paths are guarded by cheap, inlined range checks (`in_range`, cursor limits) and audited `unsafe` blocks with explicit safety invariants.

---

## 📦 Installation & Cargo Features

Add `libmaxminddb-rs` to your `Cargo.toml`:

```toml
[dependencies]
libmaxminddb-rs = "0.2.1"
```

### ⚙️ Cargo Features

| Feature | Default | Status | Description |
|:---|:---:|:---:|:---|
| 🔍 `reader` | **Yes** | ✅ | Search tree traversal, MMDB v2 decoding, `mmap` file support |
| ✍️ `writer` | **Yes** | ✅ | In-memory trie builder, binary serialization, deep merge |
| 🧬 `derive` | **Yes** | ✅ | Procedural derive macros: `#[derive(MmdbDecode, MmdbEncode, MmdbRecord)]` |
| ⚡ `simd` | **Yes** | ✅ | Vectorized SSE2/AVX2 (x86_64) and NEON (AArch64) ASCII validation with runtime detection |

The fast tree is built into the reader and is always prepared during `Reader::open`, `Reader::open_mmap`, `Reader::from_vec`, or `Reader::from_bytes` for every valid MMDB record size. It is no longer a Cargo feature. Common 24/28/32-bit records use aligned `u32` nodes and acceleration tables; 36–64-bit records use decoded `u64` children (16 bytes per node). Preparation errors are returned when opening the database.

---

## 🚀 Quickstart Guide

### 🔍 1. Reading & Zero-Copy Lookups

This complete example builds a small database, borrows a typed record from it, and handles an absent address. `&str` fields refer to the reader's MMDB bytes; the record cannot outlive the reader.

```rust
use libmaxminddb_rs::{Error, MetadataBuilder, MmdbDecode, MmdbEncode, Reader, Writer};
use std::net::IpAddr;

#[derive(MmdbDecode, MmdbEncode)]
struct NetworkRecord<'a> {
    asn: u32,
    org: &'a str,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let metadata = MetadataBuilder::new().ip_version(4).build()?;
    let mut writer = Writer::with_metadata(metadata);
    writer.insert_encoded(
        "198.51.100.0/24".parse()?,
        &NetworkRecord { asn: 64512, org: "Example Network" },
    )?;
    let bytes = writer.finish()?;
    let reader = Reader::from_bytes(&bytes)?;

    let ip: IpAddr = "198.51.100.7".parse()?;
    let record: NetworkRecord<'_> = reader.lookup_borrowed(ip)?;
    assert_eq!(record.asn, 64512);
    assert_eq!(record.org, "Example Network");

    // Project a large decoded record to a small return value on a hit.
    let asn = reader.lookup_borrowed_map(ip, |r: NetworkRecord<'_>| r.asn)?;
    assert_eq!(asn, Some(64512));

    let missing: IpAddr = "203.0.113.7".parse()?;
    assert!(matches!(reader.lookup_borrowed::<NetworkRecord<'_>>(missing), Err(Error::NotFound)));
    Ok(())
}
```

For a zero-copy walkthrough that can be run with Cargo, see [`examples/zero_copy_lookup.rs`](examples/zero_copy_lookup.rs).

### Lookup API Reference

The GeoLite2 examples use shared record types and a streaming download helper from [`examples/common/mod.rs`](examples/common/mod.rs). Each example downloads its database once into `target/database` and reuses it on later runs.
<details>
<summary>GeoLite2 Country — full example</summary>

Source: [`examples/geolite2_country.rs`](examples/geolite2_country.rs)<br>
Run with: `cargo run --example geolite2_country`

```rust
use anyhow::Result;
mod common;
use common::download_file;
use libmaxminddb_rs::{MmdbDecode, Reader};
use std::net::IpAddr;
use std::path::PathBuf;
use tokio::fs;

#[derive(Debug, MmdbDecode)]
pub struct GeoLite2Country<'a> {
    pub continent: Option<Continent<'a>>,
    pub country: Option<Country<'a>>,
    pub location: Option<Location<'a>>,
    pub registered_country: Option<Country<'a>>,
}

#[derive(Debug, MmdbDecode)]
pub struct Continent<'a> {
    pub code: Option<&'a str>,
    pub geoname_id: Option<u64>,
    pub names: Option<Names<'a>>,
}

#[derive(Debug, MmdbDecode)]
pub struct Country<'a> {
    pub geoname_id: Option<u64>,
    pub iso_code: Option<&'a str>,
    pub names: Option<Names<'a>>,
}

#[derive(Debug, MmdbDecode)]
pub struct Names<'a> {
    pub de: Option<&'a str>,
    pub en: Option<&'a str>,
    pub es: Option<&'a str>,
    pub fr: Option<&'a str>,
    pub ja: Option<&'a str>,
    #[serde(rename = "pt-BR")]
    pub pt_br: Option<&'a str>,
    pub ru: Option<&'a str>,
    #[serde(rename = "zh-CN")]
    pub zh_cn: Option<&'a str>,
}

#[derive(Debug, MmdbDecode)]
pub struct Location<'a> {
    pub accuracy_radius: Option<u64>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub time_zone: Option<&'a str>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Keep the downloaded MMDB in this example package's target directory.
    let database_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("database")
        .join("GeoLite2-Country.mmdb");

    // download_file creates target/database automatically when it is missing.
    // Reuse an existing database file to avoid downloading it on every run.
    if !fs::try_exists(&database_file).await? {
        download_file(
            "https://github.com/P3TERX/GeoLite.mmdb/releases/download/2026.09.28/GeoLite2-Country.mmdb",
            &database_file,
        )
        .await?;
    }

    // Open the downloaded MaxMind DB with the library reader.
    let reader = Reader::open(&database_file)?;

    // Parse the query address once before looking it up.
    let ip: IpAddr = "8.8.8.8".parse()?;

    // Borrowed strings remain tied to the reader's MMDB bytes where possible.
    let record: GeoLite2Country<'_> = reader.lookup_borrowed(ip)?;
    println!("{record:#?}");

    Ok(())
}
```

</details>

<details>
<summary>GeoLite2 City — full example</summary>

Source: [`examples/geolite2_city.rs`](examples/geolite2_city.rs)<br>
Run with: `cargo run --example geolite2_city`

```rust
use anyhow::Result;
mod common;
use common::download_file;
use libmaxminddb_rs::{MmdbDecode, Reader};
use std::net::IpAddr;
use std::path::PathBuf;
use tokio::fs;

#[derive(Debug, MmdbDecode)]
pub struct GeoLite2City<'a> {
    pub continent: Option<Continent<'a>>,
    pub country: Option<Country<'a>>,
    pub city: Option<City<'a>>,
    pub location: Option<Location<'a>>,
    pub postal: Option<Postal<'a>>,
    pub registered_country: Option<Country<'a>>,
    pub subdivisions: Option<Vec<Subdivision<'a>>>,
    pub traits: Option<Traits>,
}

#[derive(Debug, MmdbDecode)]
pub struct Continent<'a> {
    pub code: Option<&'a str>,
    pub geoname_id: Option<u64>,
    pub names: Option<Names<'a>>,
}

#[derive(Debug, MmdbDecode)]
pub struct Country<'a> {
    pub geoname_id: Option<u64>,
    pub iso_code: Option<&'a str>,
    pub names: Option<Names<'a>>,
}

#[derive(Debug, MmdbDecode)]
pub struct City<'a> {
    pub geoname_id: Option<u64>,
    pub names: Option<Names<'a>>,
}

#[derive(Debug, MmdbDecode)]
pub struct Names<'a> {
    pub de: Option<&'a str>,
    pub en: Option<&'a str>,
    pub es: Option<&'a str>,
    pub fr: Option<&'a str>,
    pub ja: Option<&'a str>,
    #[serde(rename = "pt-BR")]
    pub pt_br: Option<&'a str>,
    pub ru: Option<&'a str>,
    #[serde(rename = "zh-CN")]
    pub zh_cn: Option<&'a str>,
}

#[derive(Debug, MmdbDecode)]
pub struct Location<'a> {
    pub accuracy_radius: Option<u64>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub time_zone: Option<&'a str>,
}

#[derive(Debug, MmdbDecode)]
pub struct Postal<'a> {
    pub code: Option<&'a str>,
}

#[derive(Debug, MmdbDecode)]
pub struct Subdivision<'a> {
    pub geoname_id: Option<u64>,
    pub iso_code: Option<&'a str>,
    pub names: Option<Names<'a>>,
}

#[derive(Debug, MmdbDecode)]
pub struct Traits {
    pub is_anonymous_proxy: Option<bool>,
    pub is_satellite_provider: Option<bool>,
    pub is_anycast: Option<bool>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Store the GeoLite2 City database under this package's target directory.
    let database_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("database")
        .join("GeoLite2-City.mmdb");

    // The shared downloader creates target/database and streams the file to disk.
    // Keep an existing database so later runs do not download it again.
    if !fs::try_exists(&database_file).await? {
        download_file(
            "https://github.com/P3TERX/GeoLite.mmdb/releases/download/2026.09.28/GeoLite2-City.mmdb",
            &database_file,
        )
        .await?;
    }

    // Open the MMDB reader, parse the address, and borrow record strings.
    let reader = Reader::open(&database_file)?;
    let ip: IpAddr = "8.8.8.8".parse()?;
    let record: GeoLite2City<'_> = reader.lookup_borrowed(ip)?;

    println!("{record:#?}");
    Ok(())
}
```

</details>

<details>
<summary>GeoLite2 ASN — full example</summary>

Source: [`examples/geolite2_asn.rs`](examples/geolite2_asn.rs)<br>
Run with: `cargo run --example geolite2_asn`

```rust
use anyhow::Result;
mod common;
use common::download_file;
use libmaxminddb_rs::{MmdbDecode, Reader};
use std::net::IpAddr;
use std::path::PathBuf;
use tokio::fs;

#[derive(Debug, MmdbDecode)]
pub struct GeoLite2Asn<'a> {
    pub autonomous_system_number: u64,
    pub autonomous_system_organization: &'a str,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Use a database-specific filename so another GeoLite2 database cannot be reused.
    let database_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("database")
        .join("GeoLite2-ASN.mmdb");

    // Download once; the shared helper creates target/database and streams to disk.
    if !fs::try_exists(&database_file).await? {
        download_file(
            "https://github.com/P3TERX/GeoLite.mmdb/releases/download/2026.09.28/GeoLite2-ASN.mmdb",
            &database_file,
        )
        .await?;
    }

    let reader = Reader::open(&database_file)?;
    let ip: IpAddr = "8.8.8.8".parse()?;

    // The organization string borrows from the MMDB data for this lookup.
    let record: GeoLite2Asn<'_> = reader.lookup_borrowed(ip)?;
    println!("{record:#?}");
    Ok(())
}
```

</details>

### ✍️ 2. Database Creation & Serialization

The writer constructs standard-compliant MMDB v2 binary databases in memory with automatic payload deduplication and optimal pointer sizing (24, 28, or 32 bits).

The `#[derive(MmdbEncode, MmdbRecord)]` derive macros provide a seamless one-object insertion API: the `#[mmdb(network)]` attribute marks the CIDR subnet key and automatically omits it from the serialized payload data.

*(Run this complete example with `cargo run --example custom_record`)*

```rust
use std::error::Error;
use libmaxminddb_rs::{IpNetwork, MetadataBuilder, MmdbEncode, MmdbRecord, Writer};

/// Custom record deriving both MmdbEncode and MmdbRecord.
/// The #[mmdb(network)] field designates the subnet without storing it in the payload.
#[derive(Debug, MmdbEncode, MmdbRecord)]
struct SecurityEntry<'a> {
    #[mmdb(network)]
    network: IpNetwork,
    country: &'a str,
    threat_score: u32,
    is_tor_exit: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    // 1. Configure database metadata (database type, IP version, languages, descriptions)
    let metadata = MetadataBuilder::new()
        .database_type("Security-Intelligence")
        .ip_version(4)
        .description("en", "IP Threat Intelligence Feed")
        .build()?;

    let mut writer = Writer::with_metadata(metadata);

    // 2. Insert records using the ergonomic one-object insertion API
    writer.insert_entry(&SecurityEntry {
        network: "203.0.113.0/24".parse()?,
        country: "FR",
        threat_score: 85,
        is_tor_exit: false,
    })?;

    writer.insert_entry(&SecurityEntry {
        network: "198.51.100.128/25".parse()?,
        country: "US",
        threat_score: 95,
        is_tor_exit: true,
    })?;

    // 3. Finalize into a contiguous in-memory byte buffer
    let mmdb_bytes: Vec<u8> = writer.finish()?;
    println!("Database serialized successfully: {} bytes", mmdb_bytes.len());

    // Alternatively, serialize directly to a file on disk:
    // writer.write_to_file("threat-intelligence.mmdb")?;

    Ok(())
}
```

### 🔀 3. Merging Databases (Deep Merge)

A common challenge in network telemetry is **combining multi-source intelligence** on identical or overlapping subnets — such as augmenting a baseline IP geolocation database with an external real-time threat intelligence feed.

By default, MMDB writers use `MergeStrategy::Replace`, where inserting an existing subnet completely wipes out the previous record, destroying any previously associated geographic coordinates or ISP metadata.

Configuring the writer with `MergeStrategy::DeepMerge` enables recursive hierarchical merging:
- **Nested Maps**: Recursively merged key-by-key. Keys present only in the base record are preserved (`country`, `city`, `details.timezone`). New keys are seamlessly added (`details.datacenter`, `security`). Conflicting nested scalar values are updated with the latest value (`details.accuracy_radius` `50` $\to$ `10`).
- **Arrays**: Concatenated in order (`["residential", "broadband"]` $+$ `["vpn_exit_node"]`).
- **Scalars**: Conflicting scalar values are replaced by the newest record.

*(Run this complete example with `cargo run --example deep_merge`)*

```rust
use std::error::Error;
use std::net::IpAddr;
use libmaxminddb_rs::{MergeStrategy, MetadataBuilder, Reader, Writer};

fn main() -> Result<(), Box<dyn Error>> {
    // 1. Initialize Writer configured with recursive DeepMerge strategy
    let metadata = MetadataBuilder::new()
        .database_type("Enriched-GeoIP-Threat")
        .ip_version(4)
        .build()?;

    let mut writer = Writer::with_metadata(metadata)
        .merge_strategy(MergeStrategy::DeepMerge);

    let target_subnet = "203.0.113.0/24".parse()?;

    // 2. Base geolocation feed: general geographic coordinates and ISP tags
    let base_geo_record = serde_json::json!({
        "country": "FR",
        "city": "Paris",
        "details": {
            "timezone": "Europe/Paris",
            "accuracy_radius": 50
        },
        "network_tags": ["residential", "broadband"]
    });
    writer.insert(target_subnet, &base_geo_record)?;

    // 3. Threat intelligence feed: enriches the same subnet with security telemetry
    let threat_intel_record = serde_json::json!({
        "details": {
            "accuracy_radius": 10,       // overrides conflicting scalar with higher precision
            "datacenter": "PAR-01"       // adds a new nested key into "details"
        },
        "network_tags": ["vpn_exit_node"], // appends new element to the existing array
        "security": {                    // adds an entirely new top-level nested map
            "is_proxy": true,
            "threat_score": 85
        }
    });
    writer.insert(target_subnet, &threat_intel_record)?;

    // 4. Finalize database and query with zero-copy Reader
    let mmdb_bytes = writer.finish()?;
    let reader = Reader::from_bytes(&mmdb_bytes)?;

    let target_ip: IpAddr = "203.0.113.42".parse()?;
    let (merged_value, prefix_len) = reader.lookup_value_with_prefix(target_ip)?;

    println!("Matched subnet prefix: /{prefix_len}");
    println!("{}", serde_json::to_string_pretty(&merged_value.to_json())?);

    Ok(())
}
```

#### 🔍 Verified Output

Executing the lookup yields the fully unified document combining both datasets:

```json
{
  "city": "Paris",
  "country": "FR",
  "details": {
    "accuracy_radius": 10,
    "datacenter": "PAR-01",
    "timezone": "Europe/Paris"
  },
  "network_tags": [
    "residential",
    "broadband",
    "vpn_exit_node"
  ],
  "security": {
    "is_proxy": true,
    "threat_score": 85
  }
}
```

#### 📋 Merge Strategy Comparison

| Strategy | Behavior on Subnet Collision | Real-World Use Case |
|:---|:---|:---|
| `Replace` *(default)* | Overwrites the entire subnet record with the new value | Replacing expired telemetry or full database rewrites |
| `Append` | Concatenates arrays; replaces conflicting non-array values | Appending audit logs, incident tickets, or historical IPs |
| `AppendUnique` | Appends new array items only if not already present; replaces non-arrays | Merging distinct category labels or tag sets without duplicates |
| `DeepMerge` | Recursively traverses maps, concatenates arrays, and replaces conflicting scalar leaves | Multi-source enrichment (e.g. GeoIP + ASN + Threat Intelligence) |


---
## 📊 Benchmarks

Reproducible cross-library benchmark suite comparing `libmaxminddb-rs` against industry standard implementations in Rust, C, and Go.

### ⚙️ Evaluated Libraries & Reproducibility Specification

| Library Name | Language | Role | Evaluated Version | Compiler & Build Flags | Upstream Repository |
|:---|:---:|:---:|:---:|:---|:---|
| 🦀 **`libmaxminddb-rs`** | Rust | Reader & Writer | 0.1.0 | `rustc 1.90.0` (opt-level=3, native) | Current Repository |
| 🏛️ **`libmaxminddb`** | C | Reader | 1.14.1 | `cc` (-O3 -march=native) | [maxmind/libmaxminddb](https://github.com/maxmind/libmaxminddb) |
| 📦 **`maxminddb-rust`** | Rust | Reader | 0.32.0 | `rustc 1.90.0` (release) | [maxminddb-rust](https://crates.io/crates/maxminddb) |
| 🚀 **`geoip2-rs`** | Rust | Reader | 0.1.8 | `rustc 1.90.0` (release) | [geoip2-rs](https://crates.io/crates/geoip2) |
| 🐹 **`maxminddb-golang`** | Go | Reader | v2.6.0 | `go go1.23.1 linux/amd64` (-ldflags="-s -w" -trimpath) | [oschwald/maxminddb-golang](https://github.com/oschwald/maxminddb-golang) |
| ✍️ **`mmdbwriter`** | Go | Writer | v1.2.0 | `go go1.23.1 linux/amd64` (-ldflags="-s -w" -trimpath) | [maxmind/mmdbwriter](https://github.com/maxmind/mmdbwriter) |

*Environment: Linux x86_64 · AMD Ryzen 7 PRO 7840U w/ Radeon 780M Graphics · rustc 1.90.0 · Deterministic SplitMix64 datasets with pre-allocated memory.*

Execute all benchmarks and regenerate reports with a single command:
```bash
make bench-compare
```

To run the same comparative suite with pinned Rust and Go toolchains in Docker, use `make bench-compare-docker` (Docker with Compose required). It selects the system `default` Docker context, even if Docker Desktop is the current CLI context; set `BENCH_DOCKER_CONTEXT=name` to choose another daemon. The command builds the image, runs `make bench-compare` in a temporary container, and prints the host paths of the generated HTML report, SVG charts, JSON/CSV results, and updated `README.md` when it finishes. Docker results and build caches remain under `target/docker-bench/`. Compare measurements only from compatible host CPUs and Docker resource limits.

Both benchmark commands generate an interactive HTML report at `benchmark-report/index.html` with all measured results, charts, and sortable tables. Open this local file after the run.

<details>
<summary>📊 Full HTML report preview (static image)</summary>

<img src="docs/images/benchmark-report-full.png" alt="Full benchmark report preview with every section, chart, and table" width="100%">

</details>

Database-size and writer benchmarks use 1K, 10K, 100K, 500K, 1M, 1.5M, 2M, 5M entries, with the same fixed seed, query workload and batch settings at every size.

Reader RSS is also compared at these eight sizes for the four Rust/C libraries, using identical databases and queries, mmap, and three isolated processes per point. The generated HTML report's Memory section shows RSS after open and the lookup peak, with exact hover values and explicit unavailable measurements. See [the memory protocol](docs/MEMORY_BENCHMARKS.md) for reproduction and interpretation.

### 🏁 Reader Performance Summary — p99 Tail Latency & Peak Throughput

*Measured on AMD Ryzen 7 PRO 7840U w/ Radeon 780M Graphics under Linux:*

| Scenario | Metric | 🦀 `libmaxminddb-rs` | 🏛️ `libmaxminddb` (C) | 📦 `maxminddb-rust` | 🚀 `geoip2-rs` | 🐹 `maxminddb-golang` | ✍️ `mmdbwriter` (Go) |
|:---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| IPv4 Random Lookup (1M) | p99 Tail Latency | 🏆 **471.0 ns** | 541.0 ns | <span style="color:#ff3b5c">702.0 ns</span> | 561.0 ns | 662.0 ns | — |
| IPv4 Random Throughput (1M) | Peak Throughput | 🏆 **44.68 M ops/s** | 21.29 M ops/s | 13.79 M ops/s | 21.93 M ops/s | <span style="color:#ff3b5c">12.81 M ops/s</span> | — |
| IPv6 Random Lookup (1M) | p99 Tail Latency | **191.0 ns** | 🏆 **60.0 ns** | 341.0 ns | <span style="color:#ff3b5c">351.0 ns</span> | 81.0 ns | — |
| IPv6 Random Throughput (1M) | Peak Throughput | 🏆 **116.69 M ops/s** | 69.31 M ops/s | 36.08 M ops/s | 48.65 M ops/s | <span style="color:#ff3b5c">29.65 M ops/s</span> | — |
| 16-Thread Concurrent IPv4 | Concurrent Throughput | 🏆 **173.71 M ops/s** | 106.94 M ops/s | 89.21 M ops/s | 124.05 M ops/s | <span style="color:#ff3b5c">66.79 M ops/s</span> | — |
| Database Open (mmap) | Median Latency | <span style="color:#ff3b5c">97.80 µs</span> | 34.45 µs | 11.99 µs | 🏆 **10.59 µs** | 13.16 µs | — |
| Writer Generation (5M) | Insert Throughput | 🏆 **1.05 M ops/s** | — | — | — | — | <span style="color:#ff3b5c">406.9 K ops/s</span> |
| Writer Total Time (5M) | Total Duration | 🏆 **4.76 s** | — | — | — | — | <span style="color:#ff3b5c">12.29 s</span> |
| Writer Peak RSS (5M) | Peak Memory (RSS) | <span style="color:#ff3b5c">569.72 MiB</span> | — | — | — | — | 🏆 **150.54 MiB** |

### 📈 Visual Benchmark Charts

<p align="center"><strong>Candlestick Percentile Rank — IPv4 Lookups</strong><br><img src="benchmarks/charts/candlestick-percentiles-ipv4.svg" alt="Candlestick Percentile Rank — IPv4 Lookups" width="100%"></p>

<p align="center"><strong>Candlestick Percentile Rank — IPv6 Lookups</strong><br><img src="benchmarks/charts/candlestick-percentiles-ipv6.svg" alt="Candlestick Percentile Rank — IPv6 Lookups" width="100%"></p>

<p align="center"><strong>IPv4 Throughput — Random Lookups (1M)</strong><br><img src="benchmarks/charts/throughput-ipv4-random.svg" alt="IPv4 Throughput — Random Lookups (1M)" width="100%"></p>

<p align="center"><strong>IPv6 Throughput — Random Lookups (1M)</strong><br><img src="benchmarks/charts/throughput-ipv6-random.svg" alt="IPv6 Throughput — Random Lookups (1M)" width="100%"></p>

<p align="center"><strong>IPv4 Throughput — Absent Keys (1M)</strong><br><img src="benchmarks/charts/throughput-ipv4-absent.svg" alt="IPv4 Throughput — Absent Keys (1M)" width="100%"></p>

<p align="center"><strong>IPv6 Throughput — Absent Keys (1M)</strong><br><img src="benchmarks/charts/throughput-ipv6-absent.svg" alt="IPv6 Throughput — Absent Keys (1M)" width="100%"></p>

<p align="center"><strong>Peak Concurrent Throughput — 16 Threads</strong><br><img src="benchmarks/charts/concurrent-throughput.svg" alt="Peak Concurrent Throughput — 16 Threads" width="100%"></p>

<p align="center"><strong>Peak Concurrent Throughput — 16 Threads (IPv6)</strong><br><img src="benchmarks/charts/concurrent-throughput-ipv6.svg" alt="Peak Concurrent Throughput — 16 Threads (IPv6)" width="100%"></p>

<p align="center"><strong>IPv4 Random Lookup — Worker Scaling by API</strong><br><img src="benchmarks/charts/lookup-api-ipv4-multithread.svg" alt="IPv4 Random Lookup — Worker Scaling by API" width="100%"></p>

<p align="center"><strong>IPv6 Random Lookup — Worker Scaling by API</strong><br><img src="benchmarks/charts/lookup-api-ipv6-multithread.svg" alt="IPv6 Random Lookup — Worker Scaling by API" width="100%"></p>

<p align="center"><strong>p99 Tail Latency vs Database Size</strong><br><img src="benchmarks/charts/database-size-scaling.svg" alt="p99 Tail Latency vs Database Size" width="100%"></p>

<p align="center"><strong>Peak RSS During Lookups (mmap)</strong><br><img src="benchmarks/charts/memory-rss-peak.svg" alt="Peak RSS During Lookups (mmap)" width="100%"></p>

## 🔥 FlameGraph

Profile all public lookup methods on IPv4 and IPv6 and regenerate the SVG with:

```bash
make flamegraph
```

The command writes the lookup-only profile to `target/flamegraph/lookup-flamegraph.svg` and updates the image below. The graph covers `lookup_borrowed`, `lookup_borrowed_opt`, `lookup_borrowed_map`, `lookup_value`, `lookup_value_with_prefix`, `lookup`, `lookup_many`, and `lookup_exists`.

<img src="docs/images/lookup-flamegraph.svg" alt="Flame graph of all IPv4 and IPv6 reader lookup methods" width="100%">

## Documentation and compatibility

The [Rust API documentation](https://docs.rs/libmaxminddb-rs) describes the reader, writer, value types, and derive macros. Runnable usage examples are in [`examples/`](examples/README.md). The crate supports MMDB v2 files, IPv4 and IPv6, and 24-, 28-, or 32-bit tree records. Its minimum supported Rust version is **1.98.1** (edition 2024). `Reader::open_mmap` is `unsafe` because callers must keep the mapped file unchanged while the reader exists.

For implementation details and performance protocols, see [`docs/`](docs/) and [`AGENTS.md`](AGENTS.md). The `derive` proc-macro crate is a separate package and must be published before the main crate.

## Development and tests

Run `make` or `make help` to see every available rule, grouped by task.

```bash
cargo build --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo doc --no-deps --all-features
make bench-performance
make bench-writer
make bench-micro
```

`make publish-check` checks formatting, linting, tests, documentation, and both package archives. `make release X.Y.Z` synchronizes version references; after those edits are committed, rerun it to push `main`, publish the proc-macro crate and main crate to crates.io, then create the Git tag and GitHub Release. Run the release command only when you intend to publish. The [contribution guide](CONTRIBUTING.md) explains the development workflow.

## Code Coverage & Tests

Run the complete test suite with the Makefile target. It includes the workspace (including `derive`), documentation tests, feature-isolated builds, and the benchmark tooling crates:

```bash
make tests
```

Install [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov), then generate the HTML report and per-file summary with:

```bash
make coverage
```

Open `target/coverage/html/index.html` in a browser. Both Makefile targets refresh this section and the test and code coverage badges; `make tests` runs coverage after its other tests. Documentation tests are run separately and are not included in these coverage figures because instrumenting them requires a nightly Rust toolchain.

<!-- coverage-summary:start -->
The latest local coverage run measured **97.55% overall line coverage** and **96.03% region coverage**.
<!-- coverage-summary:end -->

<!-- coverage-table:start -->
<details>
<summary>Code coverage by file (line, region, and function coverage)</summary>

| File | Lines | Regions | Functions |
| --- | ---: | ---: | ---: |
| [`derive/src/lib.rs`](derive/src/lib.rs) | 97.01% (292/301) | 96.47% (465/482) | 100.00% (41/41) |
| [`src/decoder/ascii.rs`](src/decoder/ascii.rs) | 97.35% (147/151) | 98.35% (298/303) | 100.00% (12/12) |
| [`src/decoder/mod.rs`](src/decoder/mod.rs) | 96.86% (524/541) | 95.11% (895/941) | 96.55% (28/29) |
| [`src/decoder/raw.rs`](src/decoder/raw.rs) | 98.29% (518/527) | 95.51% (999/1046) | 94.00% (47/50) |
| [`src/encoder.rs`](src/encoder.rs) | 100.00% (113/113) | 94.30% (215/228) | 100.00% (8/8) |
| [`src/metadata.rs`](src/metadata.rs) | 97.02% (228/235) | 96.59% (340/352) | 100.00% (26/26) |
| [`src/reader/marker.rs`](src/reader/marker.rs) | 96.79% (211/218) | 97.00% (420/433) | 100.00% (18/18) |
| [`src/reader/mod.rs`](src/reader/mod.rs) | 97.50% (781/801) | 94.94% (1407/1482) | 98.53% (67/68) |
| [`src/reader/tree.rs`](src/reader/tree.rs) | 96.98% (835/861) | 96.27% (1576/1637) | 100.00% (54/54) |
| [`src/traits.rs`](src/traits.rs) | 100.00% (354/354) | 97.82% (629/643) | 100.00% (59/59) |
| [`src/value.rs`](src/value.rs) | 98.67% (593/601) | 97.78% (750/767) | 98.15% (106/108) |
| [`src/writer/mod.rs`](src/writer/mod.rs) | 96.65% (865/895) | 95.34% (1412/1481) | 95.24% (60/63) |
| **Total** | **97.55% (5461/5598)** | **96.03% (9406/9795)** | **98.13% (526/536)** |

</details>
<!-- coverage-table:end -->

## Contributing

Issues and pull requests are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) before changing public APIs, parser invariants, or hot paths. User-visible changes should be recorded in [CHANGELOG.md](CHANGELOG.md).

## License

Licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
