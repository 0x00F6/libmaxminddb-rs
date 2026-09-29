//! # Metadata Inspection Example for libmaxminddb-rs
//!
//! This example shows how to inspect the header and metadata section of any
//! MaxMind DB (MMDB) file.
//!
//! The metadata section is stored at the end of the MMDB file and contains
//! crucial information about the database topology, IP version support,
//! languages, build timestamp, and descriptions.
//!
//! Run this example with:
//! ```bash
//! cargo run --example metadata
//! ```

use std::error::Error;
use std::time::{Duration, UNIX_EPOCH};

use libmaxminddb_rs::{MetadataBuilder, Reader, Writer};

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== libmaxminddb-rs Metadata Inspection Example ===\n");

    // 1. Build a representative database with rich metadata
    let epoch_timestamp = 1716500000; // Sample build epoch
    let metadata = MetadataBuilder::new()
        .database_type("GeoIP2-City-Example")
        .ip_version(6)
        .language("en")
        .language("fr")
        .language("de")
        .language("ja")
        .description("en", "MaxMind DB Geolocation Database")
        .description("fr", "Base de données de géolocalisation MaxMind")
        .description("de", "MaxMind Geolocation-Datenbank")
        .build_epoch(epoch_timestamp)
        .build()?;

    let mut writer = Writer::with_metadata(metadata);
    writer.insert(
        "2001:db8::/32".parse()?,
        &serde_json::json!({"city": "Documentation"}),
    )?;
    let db_bytes = writer.finish()?;

    // 2. Open database and inspect metadata
    let reader = Reader::from_bytes(&db_bytes)?;
    let meta = reader.metadata();

    println!("Database Header & Metadata:");
    println!("--------------------------------------------------");
    println!("Database Type        : {}", meta.database_type);
    println!(
        "Binary Format Version: {}.{}",
        meta.binary_format_major_version, meta.binary_format_minor_version
    );
    println!("IP Version           : IPv{}", meta.ip_version);
    println!("Record Size          : {} bits", meta.record_size);
    println!("Search Tree Nodes    : {}", meta.node_count);

    // Convert epoch to formatted human-readable UTC time
    let build_time = UNIX_EPOCH + Duration::from_secs(meta.build_epoch);
    println!(
        "Build Epoch          : {} ({:?})",
        meta.build_epoch, build_time
    );

    println!("\nSupported Languages ({}):", meta.languages.len());
    for lang in &meta.languages {
        println!("  ↳ {}", lang);
    }

    println!("\nLocalized Descriptions ({}):", meta.description.len());
    for (lang, desc) in &meta.description {
        println!("  ↳ [{:<2}] {}", lang, desc);
    }
    println!("--------------------------------------------------");

    println!("\nMetadata inspection completed successfully!");
    Ok(())
}
