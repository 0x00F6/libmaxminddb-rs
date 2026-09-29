//! # Read and Write Example for libmaxminddb-rs
//!
//! This example shows how to write a custom database and read records back
//! using separate write and read models:
//! - `StoredRecord`: An owned or borrowed struct derived with `MmdbEncode` for database insertion.
//! - `GeoRecord`: A zero-copy borrowed struct derived with `MmdbDecode` for fast querying.
//!
//! Run this example with:
//! ```bash
//! cargo run --example read_write
//! ```

use std::error::Error;
use std::net::IpAddr;

use libmaxminddb_rs::{MetadataBuilder, MmdbDecode, MmdbEncode, Reader, Writer};

/// Record struct used when decoding during lookup.
///
/// Notice the use of `Option<&'a str>` for optional fields and `&'a str`
/// for borrowed string slices referencing the database memory directly.
#[derive(Debug, PartialEq, MmdbDecode)]
struct GeoRecord<'a> {
    country: &'a str,
    city: Option<&'a str>,
    latitude: f64,
    longitude: f64,
}

/// Record struct used when writing data into the MMDB database.
#[derive(Debug, PartialEq, MmdbEncode)]
struct StoredRecord<'a> {
    country: &'a str,
    city: &'a str,
    latitude: f64,
    longitude: f64,
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== libmaxminddb-rs Read / Write Example ===\n");

    // 1. Build metadata
    println!("1. Configuring metadata...");
    let metadata = MetadataBuilder::new()
        .database_type("libmaxminddb-rs-example")
        .ip_version(4)
        .language("en")
        .description("en", "Read/Write example database")
        .build()?;

    let mut writer = Writer::with_metadata(metadata);

    // 2. Insert typed records
    println!("2. Inserting records with `insert_encoded`...");
    writer.insert_encoded(
        "203.0.113.0/24".parse()?,
        &StoredRecord {
            country: "FR",
            city: "Paris",
            latitude: 48.8566,
            longitude: 2.3522,
        },
    )?;

    // 3. Serialize into memory buffer
    let bytes = writer.finish()?;
    println!("   Database created ({} bytes).\n", bytes.len());

    // 4. Open with Reader
    println!("3. Reading back record with `lookup_borrowed`...");
    let reader = Reader::from_bytes(&bytes)?;

    let target_ip: IpAddr = "203.0.113.7".parse()?;
    let record: GeoRecord<'_> = reader.lookup_borrowed(target_ip)?;

    println!("   IP: {}", target_ip);
    println!("   ↳ Country     : {}", record.country);
    println!("   ↳ City        : {:?}", record.city);
    println!(
        "   ↳ Coordinates : ({:.4}, {:.4})\n",
        record.latitude, record.longitude
    );

    println!("Read/Write example completed successfully!");
    Ok(())
}
