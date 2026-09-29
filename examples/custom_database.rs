//! # Custom Database Builder Example for libmaxminddb-rs
//!
//! This example shows how to build and configure a custom MMDB file from scratch:
//! - Configuring database metadata (record size, IP version, multi-language descriptions).
//! - Inserting both IPv4 and IPv6 networks into the same database.
//! - Writing the database directly to a disk file.
//! - Opening the resulting file with memory mapping (`open_mmap`).
//!
//! Run this example with:
//! ```bash
//! cargo run --example custom_database
//! ```

use std::error::Error;
use std::net::IpAddr;
use std::path::Path;

use libmaxminddb_rs::{MetadataBuilder, MmdbDecode, MmdbEncode, Reader, Writer};

#[derive(Debug, PartialEq, MmdbEncode, MmdbDecode)]
struct NetworkPolicy<'a> {
    zone: &'a str,
    rate_limit_rps: u32,
    allowed_protocols: Vec<&'a str>,
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== libmaxminddb-rs Custom Database Builder Example ===\n");

    let db_path = Path::new("target/custom_policy.mmdb");

    // -----------------------------------------------------------------------
    // 1. Build metadata with custom specifications
    // -----------------------------------------------------------------------
    println!("1. Configuring database metadata...");
    let metadata = MetadataBuilder::new()
        .database_type("Network-Policy-DB")
        // An IPv6 database can store both IPv4 (mapped) and IPv6 subnets:
        .ip_version(6)
        .build_epoch(1700000000)
        .language("en")
        .language("fr")
        .description("en", "Custom network policy routing database")
        .description("fr", "Base de données de politiques réseau")
        .build()?;

    let mut writer = Writer::with_metadata(metadata);

    // -----------------------------------------------------------------------
    // 2. Insert IPv4 and IPv6 network policies
    // -----------------------------------------------------------------------
    println!("2. Inserting network policy rules...");

    // IPv4 internal zone
    writer.insert_encoded(
        "10.0.0.0/8".parse()?,
        &NetworkPolicy {
            zone: "internal_trusted",
            rate_limit_rps: 50_000,
            allowed_protocols: vec!["https", "ssh", "grpc"],
        },
    )?;

    // IPv4 external DMZ zone
    writer.insert_encoded(
        "198.51.100.0/24".parse()?,
        &NetworkPolicy {
            zone: "dmz_public",
            rate_limit_rps: 1_000,
            allowed_protocols: vec!["https"],
        },
    )?;

    // IPv6 internal management zone
    writer.insert_encoded(
        "2001:db8:acad::/48".parse()?,
        &NetworkPolicy {
            zone: "ipv6_management",
            rate_limit_rps: 10_000,
            allowed_protocols: vec!["https", "ssh"],
        },
    )?;

    // -----------------------------------------------------------------------
    // 3. Write database to disk
    // -----------------------------------------------------------------------
    println!("3. Writing database to: {}", db_path.display());
    std::fs::create_dir_all(db_path.parent().unwrap())?;
    writer.write_to_file(db_path)?;

    let file_size = std::fs::metadata(db_path)?.len();
    println!(
        "   File successfully created (size: {} bytes).\n",
        file_size
    );

    // -----------------------------------------------------------------------
    // 4. Open via memory mapping (open_mmap)
    // -----------------------------------------------------------------------
    println!("4. Re-opening with zero-copy memory mapping (`open_mmap`)...");
    // SAFETY: The file is not modified while open.
    let reader = unsafe { Reader::open_mmap(db_path)? };

    // Inspect metadata
    let meta = reader.metadata();
    println!("   ↳ Database Type : {}", meta.database_type);
    println!("   ↳ IP Version    : IPv{}", meta.ip_version);
    println!("   ↳ Record Size   : {} bits", meta.record_size);
    println!("   ↳ Node Count    : {}", meta.node_count);

    // -----------------------------------------------------------------------
    // 5. Query IPv4 and IPv6 addresses
    // -----------------------------------------------------------------------
    println!("\n5. Querying policies across address families:");

    let queries: &[(&str, &str)] = &[
        ("10.50.1.20", "IPv4 internal address"),
        ("198.51.100.5", "IPv4 DMZ address"),
        ("2001:db8:acad::1", "IPv6 management address"),
    ];

    for &(ip_str, desc) in queries {
        let ip: IpAddr = ip_str.parse()?;
        let policy: NetworkPolicy<'_> = reader.lookup_borrowed(ip)?;
        println!("   IP: {} ({})", ip, desc);
        println!("   ↳ Zone       : {}", policy.zone);
        println!("   ↳ Rate Limit : {} rps", policy.rate_limit_rps);
        println!("   ↳ Protocols  : {:?}\n", policy.allowed_protocols);
    }

    // Clean up temporary file
    let _ = std::fs::remove_file(db_path);

    println!("Custom database builder example completed successfully!");
    Ok(())
}
