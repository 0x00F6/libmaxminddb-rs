//! # Batch IP Lookups Example (`lookup_many`) for libmaxminddb-rs
//!
//! This example demonstrates batch IP lookups using `Reader::lookup_many`.
//!
//! ## Key Capabilities
//! - **Order-Preserving**: Returns a `Vec<Result<ValueRef<'a>>>` where each slot
//!   matches the exact position of the input `ips` slice.
//! - **Zero-Copy Borrowing**: Returned `ValueRef` items borrow string and byte
//!   slices directly from the underlying database buffer.
//! - **Automatic Adaptive Parallelism**:
//!   - For small batches (< 4,096 IPs) or single-core machines, lookups are
//!     executed sequentially on the calling thread with zero thread-spawning overhead.
//!   - For large batches (>= 4,096 IPs) on multi-core systems, work is
//!     automatically partitioned across available CPU cores via `std::thread::scope`.
//! - **Lock-Free Concurrency**: Operates with zero lock or mutex contention.
//!
//! Run this example with:
//! ```bash
//! cargo run --example lookup_many
//! ```

use std::error::Error;
use std::net::IpAddr;
use std::time::Instant;

use libmaxminddb_rs::{MetadataBuilder, MmdbDecode, MmdbEncode, Reader, ValueRef, Writer};

#[derive(Debug, MmdbEncode, MmdbDecode)]
struct LocationRecord<'a> {
    country: &'a str,
    city: &'a str,
    asn: u32,
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== libmaxminddb-rs Batch Lookups Example (`lookup_many`) ===\n");

    // -----------------------------------------------------------------------
    // 1. Build an in-memory MMDB database with IPv4 and IPv6 networks
    // -----------------------------------------------------------------------
    println!("1. Building sample in-memory database...");
    let metadata = MetadataBuilder::new()
        .database_type("Batch-Lookup-Demo-DB")
        .ip_version(6)
        .description("en", "Demo database for batch IP lookups")
        .build()?;

    let mut writer = Writer::with_metadata(metadata);

    // Populate several subnets with location records
    writer.insert_encoded(
        "1.1.1.0/24".parse()?,
        &LocationRecord {
            country: "AU",
            city: "Sydney",
            asn: 13335,
        },
    )?;
    writer.insert_encoded(
        "8.8.8.0/24".parse()?,
        &LocationRecord {
            country: "US",
            city: "Mountain View",
            asn: 15169,
        },
    )?;
    writer.insert_encoded(
        "93.184.216.0/24".parse()?,
        &LocationRecord {
            country: "US",
            city: "Norwell",
            asn: 15133,
        },
    )?;
    writer.insert_encoded(
        "2001:db8:cafe::/48".parse()?,
        &LocationRecord {
            country: "FR",
            city: "Paris",
            asn: 64496,
        },
    )?;

    let db_bytes = writer.finish()?;
    println!("   Database created ({} bytes).\n", db_bytes.len());

    // -----------------------------------------------------------------------
    // 2. Open the database using Reader
    // -----------------------------------------------------------------------
    let reader = Reader::from_bytes(&db_bytes)?;

    // -----------------------------------------------------------------------
    // 3. Small Batch Lookup (< 4,096 IPs): Sequential execution
    // -----------------------------------------------------------------------
    println!("2. Performing small batch lookup (sequential fast path)...");
    let query_ips: Vec<IpAddr> = vec![
        "1.1.1.1".parse()?,          // Found (AU / Sydney)
        "8.8.8.8".parse()?,          // Found (US / Mountain View)
        "192.0.2.42".parse()?,       // Not found
        "93.184.216.34".parse()?,    // Found (US / Norwell)
        "2001:db8:cafe::1".parse()?, // Found IPv6 (FR / Paris)
        "2001:db8:ffff::1".parse()?, // Not found IPv6
    ];

    let start = Instant::now();
    let batch_results = reader.lookup_many(&query_ips);
    let elapsed = start.elapsed();

    println!("   Looked up {} IPs in {:?}", query_ips.len(), elapsed);
    println!("   Results (1-to-1 mapped with input IPs):");

    for (ip, res) in query_ips.iter().zip(&batch_results) {
        match res {
            Ok(value_ref) => {
                // Extract fields directly from borrowed ValueRef without heap allocation
                let country = match value_ref.get("country") {
                    Some(ValueRef::Utf8(s)) => *s,
                    _ => "unknown",
                };
                let city = match value_ref.get("city") {
                    Some(ValueRef::Utf8(s)) => *s,
                    _ => "unknown",
                };
                let asn = match value_ref.get("asn") {
                    Some(ValueRef::Uint32(n)) => *n,
                    _ => 0,
                };
                println!("   - {:<18} => Found: {city}, {country} (AS{asn})", ip);
            }
            Err(e) => {
                println!("   - {:<18} => Not Found ({e})", ip);
            }
        }
    }
    println!();

    // -----------------------------------------------------------------------
    // 4. Large Batch Lookup (>= 4,096 IPs): Multi-threaded parallel execution
    // -----------------------------------------------------------------------
    println!("3. Performing large batch lookup (>= 4,096 IPs, parallel execution)...");
    const BATCH_SIZE: usize = 100_000;
    println!("   Generating {} test IP addresses...", BATCH_SIZE);

    // Create a large batch rotating between known IPs and unmapped IPs
    let base_ips = [
        "1.1.1.1".parse::<IpAddr>()?,
        "8.8.8.8".parse::<IpAddr>()?,
        "93.184.216.34".parse::<IpAddr>()?,
        "198.51.100.1".parse::<IpAddr>()?,
    ];

    let large_ip_batch: Vec<IpAddr> = (0..BATCH_SIZE)
        .map(|i| base_ips[i % base_ips.len()])
        .collect();

    // Run parallel lookup_many
    let start_parallel = Instant::now();
    let large_results = reader.lookup_many(&large_ip_batch);
    let parallel_elapsed = start_parallel.elapsed();

    let found_count = large_results.iter().filter(|r| r.is_ok()).count();
    let throughput = (BATCH_SIZE as f64) / parallel_elapsed.as_secs_f64();

    println!(
        "   Completed {} lookups in {:?}",
        BATCH_SIZE, parallel_elapsed
    );
    println!("   Found records: {} / {}", found_count, BATCH_SIZE);
    println!(
        "   Throughput   : {:.2} M lookups/sec",
        throughput / 1_000_000.0
    );
    println!("\n✅ lookup_many demonstrated successfully!");

    Ok(())
}
