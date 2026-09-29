//! # Custom Record Derive Example for libmaxminddb-rs
//!
//! This example shows how to use `#[derive(MmdbRecord)]` with the `#[mmdb(network)]`
//! attribute.
//!
//! When a struct derives `MmdbRecord` and tags its network field with `#[mmdb(network)]`:
//! - The struct encapsulates both the IP range (`IpNetwork`) and its payload data.
//! - You can use `writer.insert_entry(&record)` directly without separating the network
//!   parameter from the data object.
//!
//! Run this example with:
//! ```bash
//! cargo run --example custom_record
//! ```

use std::error::Error;
use std::net::IpAddr;

use libmaxminddb_rs::{
    IpNetwork, MetadataBuilder, MmdbDecode, MmdbEncode, MmdbRecord, Reader, Writer,
};

/// A security threat intelligence record.
///
/// The `#[mmdb(network)]` attribute informs the derive macro which field
/// defines the CIDR network prefix for `insert_entry`.
#[derive(Debug, PartialEq, MmdbEncode, MmdbRecord)]
struct ThreatEntry<'a> {
    #[mmdb(network)]
    network: IpNetwork,
    country: &'a str,
    threat_level: &'a str,
    score: u32,
    category: &'a str,
}

/// The corresponding decode struct for reading records back out.
#[derive(Debug, PartialEq, MmdbDecode)]
struct DecodedThreat<'a> {
    country: &'a str,
    threat_level: &'a str,
    score: u32,
    category: &'a str,
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== libmaxminddb-rs Custom Record Derive Example ===\n");

    // 1. Initialize database metadata
    let metadata = MetadataBuilder::new()
        .database_type("Threat-Intel-DB")
        .ip_version(4)
        .description("en", "Threat intelligence database using MmdbRecord derive")
        .build()?;

    let mut writer = Writer::with_metadata(metadata);

    // 2. Prepare threat entries with embedded network ranges
    println!("1. Inserting entries using `writer.insert_entry`...");
    let entries = vec![
        ThreatEntry {
            network: "198.51.100.0/24".parse()?,
            country: "US",
            threat_level: "low",
            score: 15,
            category: "cloud_provider",
        },
        ThreatEntry {
            network: "203.0.113.64/26".parse()?,
            country: "FR",
            threat_level: "critical",
            score: 95,
            category: "botnet_c2",
        },
        ThreatEntry {
            network: "192.0.2.0/25".parse()?,
            country: "DE",
            threat_level: "medium",
            score: 50,
            category: "open_proxy",
        },
    ];

    // Insert directly using `insert_entry(&entry)`:
    for entry in &entries {
        println!("   ↳ Inserting entry for subnet: {}", entry.network);
        writer.insert_entry(entry)?;
    }

    // 3. Finalize database in memory
    let db_bytes = writer.finish()?;
    println!(
        "\n2. Database built in memory ({} bytes).\n",
        db_bytes.len()
    );

    // 4. Open with Reader and verify lookups
    let reader = Reader::from_bytes(&db_bytes)?;

    let test_ips: &[(&str, &str)] = &[
        ("198.51.100.22", "Expected: cloud_provider"),
        ("203.0.113.80", "Expected: botnet_c2"),
        ("192.0.2.10", "Expected: open_proxy"),
    ];

    println!("3. Performing lookups on inserted records:");
    for &(ip_str, note) in test_ips {
        let ip: IpAddr = ip_str.parse()?;
        let threat: DecodedThreat<'_> = reader.lookup_borrowed(ip)?;
        println!("   IP: {} ({})", ip, note);
        println!("   ↳ Threat Level : {}", threat.threat_level);
        println!("   ↳ Threat Score : {}/100", threat.score);
        println!("   ↳ Category     : {}", threat.category);
        println!("   ↳ Country      : {}\n", threat.country);
    }

    println!("Custom record derive example completed successfully!");
    Ok(())
}
