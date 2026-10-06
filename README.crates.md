# libmaxminddb-rs

A pure Rust reader and writer for MaxMind DB (MMDB) v2 files. Read IPv4 and IPv6 data with borrowed values, or build deterministic databases with longest-prefix matching.

## Install

```toml
[dependencies]
libmaxminddb-rs = "0.4.0"
```

Default features include the reader, writer, derive macros, SIMD ASCII scanning, and the prepared search tree. For a smaller reader-only build, use `default-features = false` with `features = ["reader", "derive"]`.

## GeoLite2 examples

The repository includes runnable examples for the GeoLite2 Country, City, and ASN databases. Each example downloads its database on first run from the latest GitHub release, stores it under `target/database`, and uses `lookup_borrowed` to borrow string data from the reader. The snippets below show the record structs and lookup for each database; they expect the corresponding `.mmdb` file in the current directory. Download those files directly with:

```bash
curl -fL -o GeoLite2-Country.mmdb https://github.com/P3TERX/GeoLite.mmdb/releases/latest/download/GeoLite2-Country.mmdb
curl -fL -o GeoLite2-City.mmdb https://github.com/P3TERX/GeoLite.mmdb/releases/latest/download/GeoLite2-City.mmdb
curl -fL -o GeoLite2-ASN.mmdb https://github.com/P3TERX/GeoLite.mmdb/releases/latest/download/GeoLite2-ASN.mmdb
```

Run the complete examples from a checkout with `cargo run --example geolite2_country`, `cargo run --example geolite2_city`, or `cargo run --example geolite2_asn`. More examples are available in the project's [`examples` directory](https://github.com/0x00F6/libmaxminddb-rs/tree/main/examples).

### GeoLite2 Country

```rust
use libmaxminddb_rs::{MmdbDecode, Reader};
use std::net::IpAddr;

#[derive(Debug, MmdbDecode)]
struct GeoLite2Country<'a> {
    continent: Option<Continent<'a>>,
    country: Option<Country<'a>>,
    location: Option<Location<'a>>,
    registered_country: Option<Country<'a>>,
}

#[derive(Debug, MmdbDecode)]
struct Continent<'a> {
    code: Option<&'a str>,
    geoname_id: Option<u64>,
    names: Option<Names<'a>>,
}

#[derive(Debug, MmdbDecode)]
struct Country<'a> {
    geoname_id: Option<u64>,
    iso_code: Option<&'a str>,
    names: Option<Names<'a>>,
}

#[derive(Debug, MmdbDecode)]
struct Names<'a> {
    en: Option<&'a str>,
}

#[derive(Debug, MmdbDecode)]
struct Location<'a> {
    accuracy_radius: Option<u64>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    time_zone: Option<&'a str>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let reader = Reader::open("GeoLite2-Country.mmdb")?;
    let ip: IpAddr = "8.8.8.8".parse()?;
    let record: GeoLite2Country<'_> = reader.lookup_borrowed(ip)?;
    println!("{record:#?}");
    Ok(())
}
```

### GeoLite2 City

```rust
use libmaxminddb_rs::{MmdbDecode, Reader};
use std::net::IpAddr;

#[derive(Debug, MmdbDecode)]
struct GeoLite2City<'a> {
    continent: Option<Continent<'a>>,
    country: Option<Country<'a>>,
    city: Option<City<'a>>,
    location: Option<Location<'a>>,
    postal: Option<Postal<'a>>,
    registered_country: Option<Country<'a>>,
    subdivisions: Option<Vec<Subdivision<'a>>>,
    traits: Option<Traits>,
}

#[derive(Debug, MmdbDecode)]
struct Continent<'a> {
    code: Option<&'a str>,
    geoname_id: Option<u64>,
    names: Option<Names<'a>>,
}

#[derive(Debug, MmdbDecode)]
struct Country<'a> {
    geoname_id: Option<u64>,
    iso_code: Option<&'a str>,
    names: Option<Names<'a>>,
}

#[derive(Debug, MmdbDecode)]
struct City<'a> {
    geoname_id: Option<u64>,
    names: Option<Names<'a>>,
}

#[derive(Debug, MmdbDecode)]
struct Names<'a> {
    en: Option<&'a str>,
}

#[derive(Debug, MmdbDecode)]
struct Location<'a> {
    accuracy_radius: Option<u64>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    time_zone: Option<&'a str>,
}

#[derive(Debug, MmdbDecode)]
struct Postal<'a> {
    code: Option<&'a str>,
}

#[derive(Debug, MmdbDecode)]
struct Subdivision<'a> {
    geoname_id: Option<u64>,
    iso_code: Option<&'a str>,
    names: Option<Names<'a>>,
}

#[derive(Debug, MmdbDecode)]
struct Traits {
    is_anonymous_proxy: Option<bool>,
    is_satellite_provider: Option<bool>,
    is_anycast: Option<bool>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let reader = Reader::open("GeoLite2-City.mmdb")?;
    let ip: IpAddr = "8.8.8.8".parse()?;
    let record: GeoLite2City<'_> = reader.lookup_borrowed(ip)?;
    println!("{record:#?}");
    Ok(())
}
```

### GeoLite2 ASN

```rust
use libmaxminddb_rs::{MmdbDecode, Reader};
use std::net::IpAddr;

#[derive(Debug, MmdbDecode)]
struct GeoLite2Asn<'a> {
    autonomous_system_number: u64,
    autonomous_system_organization: &'a str,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let reader = Reader::open("GeoLite2-ASN.mmdb")?;
    let ip: IpAddr = "8.8.8.8".parse()?;
    let record: GeoLite2Asn<'_> = reader.lookup_borrowed(ip)?;
    println!("{record:#?}");
    Ok(())
}
```

## Build and query a database

This self-contained example writes IPv4 and IPv6 networks, then decodes a typed record whose strings borrow directly from the MMDB buffer:

```rust
use libmaxminddb_rs::{MetadataBuilder, MmdbDecode, MmdbEncode, Reader, Writer};

#[derive(Debug, MmdbEncode, MmdbDecode)]
struct Network<'a> {
    country: &'a str,
    asn: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let metadata = MetadataBuilder::new().ip_version(6).build()?;
    let mut writer = Writer::with_metadata(metadata);
    writer.insert_encoded(
        "198.51.100.0/24".parse()?,
        &Network { country: "US", asn: 64512 },
    )?;
    writer.insert_encoded(
        "2001:db8:1::/48".parse()?,
        &Network { country: "FR", asn: 64513 },
    )?;

    let bytes = writer.finish()?;
    let reader = Reader::from_bytes(&bytes)?;
    let network: Network<'_> = reader.lookup_borrowed("2001:db8:1::42".parse()?)?;
    assert_eq!((network.country, network.asn), ("FR", 64513));

    // Return only the field needed by the caller; an absent IP returns None.
    let asn = reader.lookup_borrowed_map(
        "198.51.100.7".parse()?,
        |record: Network<'_>| record.asn,
    )?;
    assert_eq!(asn, Some(64512));
    Ok(())
}
```

## Write a struct to an MMDB file

`insert_encoded` serializes a struct, and `write_to_file` finalizes the database on disk. The same struct can be decoded after reopening the file:

```rust
use libmaxminddb_rs::{MetadataBuilder, MmdbDecode, MmdbEncode, Reader, Writer};

#[derive(Debug, MmdbEncode, MmdbDecode)]
struct Location<'a> {
    country: &'a str,
    city: &'a str,
    population: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = "target/locations.mmdb";
    std::fs::create_dir_all("target")?;

    let metadata = MetadataBuilder::new().ip_version(4).build()?;
    let mut writer = Writer::with_metadata(metadata);
    writer.insert_encoded(
        "203.0.113.0/24".parse()?,
        &Location {
            country: "FR",
            city: "Paris",
            population: 2_100_000,
        },
    )?;
    writer.write_to_file(path)?;

    let reader = Reader::open(path)?;
    let location: Location<'_> = reader.lookup_borrowed("203.0.113.42".parse()?)?;
    assert_eq!(
        (location.country, location.city, location.population),
        ("FR", "Paris", 2_100_000),
    );
    Ok(())
}
```

## Read an existing MMDB file

`lookup_value` returns a borrowed generic value. A missing address returns `Error::NotFound`:

```rust
use libmaxminddb_rs::{Error, Reader};
use std::net::IpAddr;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let reader = Reader::open("GeoIP.mmdb")?;
    let ip: IpAddr = "8.8.8.8".parse()?;
    match reader.lookup_value(ip) {
        Ok(value) => println!("{value:?}"),
        Err(Error::NotFound) => println!("No matching network"),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
```

## Scan all stored network ranges

`Reader::visit_records` exposes a complete, checked network/value scan with the
`reader` feature alone. `Reader::visit_borrowed_records` decodes directly into
`MmdbDecode` types and avoids generic map/array containers for borrowed scalar
fields. Deriving those types additionally requires `derive`; handwritten trait
implementations do not.

Build a small synthetic FireHOL-style database, deep-merge records for three
IPv4/IPv6 addresses, and scan its unique files/categories with assertions
(requires `reader`, `writer` and `derive`):

```rust
use libmaxminddb_rs::{MergeStrategy, MetadataBuilder, MmdbDecode, Reader, Writer};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Mutex;

#[derive(MmdbDecode)]
struct Record<'a> {
    files: Vec<&'a str>,
    categories: Vec<&'a str>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let metadata = MetadataBuilder::new().ip_version(6).build()?;
    let mut writer = Writer::with_metadata(metadata).merge_strategy(MergeStrategy::DeepMerge);
    let networks = [
        "192.0.2.1/32".parse()?,
        "198.51.100.42/32".parse()?,
        "2001:db8::1/128".parse()?,
    ];

    for network in networks {
        writer.insert(
            network,
            &json!({"files": ["base.ipset"], "categories": ["other"]}),
        )?;
        writer.insert(
            network,
            &json!({"files": ["malware.ipset"], "categories": ["malware", "other"]}),
        )?;
    }

    let bytes = writer.finish()?;
    let reader = Reader::from_bytes(&bytes)?;
    let mut seen = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut categories = BTreeSet::new();

    reader.visit_borrowed_records(|network, record: Record<'_>| {
        assert_eq!(record.files, ["base.ipset", "malware.ipset"]);
        assert_eq!(record.categories, ["other", "malware", "other"]);
        assert!(seen.insert(network));
        files.extend(record.files);
        categories.extend(record.categories);
        Ok(())
    })?;

    assert_eq!(seen, BTreeSet::from(networks));
    assert_eq!(files, BTreeSet::from(["base.ipset", "malware.ipset"]));
    assert_eq!(categories, BTreeSet::from(["malware", "other"]));
    // Parallel callbacks use Fn + Sync; borrowed fields can be retained.
    // This tiny demo uses the adaptive sequential fallback. On larger trees,
    // decoding runs concurrently and the callback order is unspecified.
    let parallel_files = Mutex::new(BTreeSet::new());
    reader.visit_borrowed_records_parallel(|_network, record: Record<'_>| {
        parallel_files.lock().unwrap().extend(record.files);
        Ok(())
    })?;
    assert_eq!(parallel_files.into_inner().unwrap(), files);
    println!("Files: {files:?}");
    println!("Categories: {categories:?}");
    Ok(())
}
```

Run [the complete example](https://github.com/0x00F6/libmaxminddb-rs/blob/main/examples/unique_fields_scan_records.rs) with `cargo run --example unique_fields_scan_records`.
It builds and scans its own in-memory database. Strings borrow the reader;
containers and database construction allocate.

`visit_borrowed_records_parallel` uses scoped threads from 24,000 tree nodes;
smaller trees or one CPU use a sequential scan. Set a worker limit and bypass
the size heuristic with `visit_borrowed_records_parallel_with_workers(NonZeroUsize, visitor)`.
Callbacks require `Fn + Sync` and run in unspecified order. Errors cancel
cooperatively: in-flight callbacks may finish before all workers are joined.

Records stream without collection; strings/bytes stay borrowed, and `T` needs
neither `Send` nor `Sync`. Setup allocates, but borrowed scalar decoding has no
per-record allocations. Prefer worker-local accumulation for throughput.

Two synthetic IPv4/IPv6 benchmark passes on an 8-CPU Xeon VM measured
**3.26–3.41x faster scans at 100k ranges** and **4.14–4.26x at 1M ranges**,
including thread setup. Gains depend on the schema and callback. See
[scan details](https://github.com/0x00F6/libmaxminddb-rs/blob/feature/parallel-borrowed-record-scan/docs/record-scan.md)
for requirements, safety limits, full results and the small sequential
generic-IPv6 tradeoff.

## 🔄 Hot In-Memory Database Updates

`Editor::from_reader` shares an existing reader and stages updates or deletions.
`finish()` returns rebuilt MMDB bytes; `write_to_file()` writes them to disk.
With `reader` and `writer` enabled, `ReloadableReader::commit` publishes the rebuilt
database while other threads continue reading.

- ⚡ **Fast reads:** `ArcSwap` avoids a global `Mutex`/`RwLock` on the lookup path.
  Hold one `load()` guard per query or batch; borrowed fields cannot outlive it.
- 🧩 **Typed updates:** pass an owned `Value` or a reference to a custom
  `MmdbEncode` struct, plus an explicit `MergeStrategy`.
- 🔀 **Merge behavior:** `Replace` replaces values; `DeepMerge` merges maps,
  appends arrays and replaces scalars. `Append`/`AppendUnique` append array items.
  Updates run in order on exact prefixes exported from the MMDB tree, without
  merging inherited parent values or more-specific children.
- 🛡️ **Safe publication:** `commit` returns `false` for stale editors; retry with
  a fresh snapshot. Rebuild errors leave the active reader unchanged.
- 🗑️ **Memory:** old owned buffers are released after the last guard, snapshot
  or editor drops. Rebuilding temporarily holds both generations; RSS may not
  decrease immediately. Publication does not persist a file. Never overwrite
  or truncate a mapped file while a reader still uses it.

This complete example updates a custom record, preserves its country field,
and verifies that the old database is destroyed after commit:

```rust
use std::sync::Arc;

use libmaxminddb_rs::{
    Editor, MergeStrategy, MetadataBuilder, MmdbDecode, MmdbEncode, Reader, ReloadableReader,
    Writer,
};

#[derive(Debug, MmdbEncode, MmdbDecode)]
struct Record<'a> {
    country: &'a str,
    score: u32,
}

#[derive(MmdbEncode)]
struct Patch {
    score: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let network = "198.51.100.0/24".parse()?;
    let ip = "198.51.100.7".parse()?;
    let metadata = MetadataBuilder::new().ip_version(4).build()?;
    let mut writer = Writer::with_metadata(metadata);
    writer.insert_encoded(
        network,
        &Record {
            country: "FR",
            score: 1,
        },
    )?;

    // Transfer ownership without retaining an extra Arc to the old database.
    let database = ReloadableReader::new(Reader::from_vec(writer.finish()?)?);
    {
        let guard = database.load();
        let record: Record<'_> = guard.lookup_borrowed(ip)?;
        println!("Before update: {record:?}");
    } // Borrowed fields and their guard are dropped before publication.

    let mut editor = Editor::from_reader(database.snapshot());
    // Weak observes destruction without keeping the old Reader alive.
    let old_lifetime = Arc::downgrade(editor.source_reader());
    editor.update_value(network, &Patch { score: 42 }, MergeStrategy::DeepMerge)?;

    // Rebuild, then publish atomically. A stale editor would return false.
    assert!(database.commit(editor)?, "Unexpected publication conflict");
    assert!(old_lifetime.upgrade().is_none());
    drop(old_lifetime);
    println!("Committed: the old Reader and its owned buffers were released.");

    let guard = database.load();
    let record: Record<'_> = guard.lookup_borrowed(ip)?;
    assert_eq!((record.country, record.score), ("FR", 42));
    println!("After DeepMerge: {record:?}");
    Ok(())
}
```

Run `cargo run --example editor_merge` for this example or
`cargo run --example concurrent_editor` for concurrent readers and a writer.

## More examples

Explore the [examples directory](https://github.com/0x00F6/libmaxminddb-rs/tree/main/examples):
[quickstart](https://github.com/0x00F6/libmaxminddb-rs/blob/main/examples/quickstart.rs),
[concurrent updates](https://github.com/0x00F6/libmaxminddb-rs/blob/main/examples/concurrent_editor.rs),
[DeepMerge and memory reclamation](https://github.com/0x00F6/libmaxminddb-rs/blob/main/examples/editor_merge.rs),
and [custom database writing](https://github.com/0x00F6/libmaxminddb-rs/blob/main/examples/custom_database.rs).
Run one with `cargo run --example quickstart`.

See the [API documentation](https://docs.rs/libmaxminddb-rs) for all methods.
Minimum Rust version: **1.98.1**. License: **MIT or Apache-2.0**.
