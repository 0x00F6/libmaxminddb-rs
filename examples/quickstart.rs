//! # Quickstart Example for libmaxminddb-rs
//!
//! This example demonstrates the most common workflow:
//! 1. Defining a custom record structure for your IP data.
//! 2. Creating an in-memory MaxMind DB (MMDB) database using `Writer`.
//! 3. Inserting IP network ranges with associated structured metadata.
//! 4. Opening the database using `Reader` with zero-copy decoding.
//! 5. Looking up IP addresses and extracting strongly typed records.
//!
//! Run this example with:
//! ```bash
//! cargo run --example quickstart
//! ```

use std::error::Error;
use std::net::IpAddr;

use libmaxminddb_rs::{MetadataBuilder, MmdbDecode, MmdbEncode, Reader, Writer};

/// Record structure used to store and retrieve geolocation data.
///
/// By deriving both `MmdbEncode` and `MmdbDecode`, this struct can be
/// serialized into the MMDB data section and deserialized directly back.
///
/// Notice the use of borrowed string slices (`&'a str`):
/// during lookup, string data points directly into the database bytes,
/// completely avoiding heap allocations!
#[derive(Debug, PartialEq, MmdbEncode, MmdbDecode)]
struct CityRecord<'a> {
    country: &'a str,
    city: &'a str,
    postal_code: Option<&'a str>,
    latitude: f64,
    longitude: f64,
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== libmaxminddb-rs Quickstart Example ===\n");

    // -----------------------------------------------------------------------
    // Step 1: Build the MMDB database in memory
    // -----------------------------------------------------------------------
    println!("1. Initializing database metadata...");
    let metadata = MetadataBuilder::new()
        .database_type("Quickstart-City-DB")
        .ip_version(4)
        .language("en")
        .language("fr")
        .description("en", "Quickstart geolocation example database")
        .description("fr", "Exemple de base de données de géolocalisation")
        .build()?;

    let mut writer = Writer::with_metadata(metadata);

    // -----------------------------------------------------------------------
    // Step 2: Insert IP networks with associated records
    // -----------------------------------------------------------------------
    println!("2. Inserting IP ranges...");

    // Paris, France: 203.0.113.0/24
    writer.insert_encoded(
        "203.0.113.0/24".parse()?,
        &CityRecord {
            country: "FR",
            city: "Paris",
            postal_code: Some("75001"),
            latitude: 48.8566,
            longitude: 2.3522,
        },
    )?;

    // Tokyo, Japan: 198.51.100.0/24
    writer.insert_encoded(
        "198.51.100.0/24".parse()?,
        &CityRecord {
            country: "JP",
            city: "Tokyo",
            postal_code: Some("100-0001"),
            latitude: 35.6762,
            longitude: 139.6503,
        },
    )?;

    // Finalize the database into a contiguous byte buffer.
    // In production, you can also use `writer.write_to_file("output.mmdb")`.
    let mmdb_bytes: Vec<u8> = writer.finish()?;
    println!(
        "   Database generated in memory (size: {} bytes).\n",
        mmdb_bytes.len()
    );

    // -----------------------------------------------------------------------
    // Step 3: Open the database with Reader
    // -----------------------------------------------------------------------
    println!("3. Opening database with Reader...");
    let reader = Reader::from_bytes(&mmdb_bytes)?;

    // -----------------------------------------------------------------------
    // Step 4: Perform zero-copy lookups
    // -----------------------------------------------------------------------
    println!("4. Performing lookups:\n");

    let test_ips: &[(&str, &str)] = &[
        ("203.0.113.42", "Paris, France subnet"),
        ("198.51.100.10", "Tokyo, Japan subnet"),
    ];

    for &(ip_str, note) in test_ips {
        let ip: IpAddr = ip_str.parse()?;

        // Perform zero-copy borrowed lookup:
        // `lookup_borrowed` yields a struct whose string slices reference
        // `mmdb_bytes` directly, with zero allocations!
        let record: CityRecord<'_> = reader.lookup_borrowed(ip)?;

        println!("   IP: {} ({})", ip, note);
        println!("   ↳ Country     : {}", record.country);
        println!("   ↳ City        : {}", record.city);
        println!("   ↳ Postal Code : {:?}", record.postal_code);
        println!(
            "   ↳ Coordinates : ({:.4}, {:.4})\n",
            record.latitude, record.longitude
        );
    }

    // -----------------------------------------------------------------------
    // Step 5: Handling unknown / unmapped IP addresses
    // -----------------------------------------------------------------------
    let unmapped_ip: IpAddr = "192.0.2.1".parse()?;
    match reader.lookup_value(unmapped_ip) {
        Ok(value) => println!("   Unexpected record for {}: {:?}", unmapped_ip, value),
        Err(err) => println!(
            "5. Looked up unmapped IP {} -> Expected error: {}\n",
            unmapped_ip, err
        ),
    }

    println!("Quickstart example completed successfully!");
    Ok(())
}
