# libmaxminddb-rs

A pure Rust reader and writer for MaxMind DB (MMDB) v2 files. Read IPv4 and IPv6 data with borrowed values, or build deterministic databases with longest-prefix matching.

## Install

```toml
[dependencies]
libmaxminddb-rs = "0.2.1"
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

```rust,no_run
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

## More examples

The project's [examples directory](https://github.com/0x00F6/libmaxminddb-rs/tree/main/examples) contains runnable Reader and Writer programs, including [quickstart](https://github.com/0x00F6/libmaxminddb-rs/blob/main/examples/quickstart.rs), [fast IPv4/IPv6 lookups](https://github.com/0x00F6/libmaxminddb-rs/blob/main/examples/fast_ip_lookup.rs), and [custom database writing](https://github.com/0x00F6/libmaxminddb-rs/blob/main/examples/custom_database.rs). Run one with `cargo run --example quickstart`.

See the [API documentation](https://docs.rs/libmaxminddb-rs) for all lookup and writer methods. Minimum supported Rust version: **1.98.1**. Licensed under **MIT or Apache-2.0**, at your option.
