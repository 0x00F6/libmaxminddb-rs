//! # Concurrent Multi-Threaded Lookups Example for libmaxminddb-rs
//!
//! This example demonstrates high-performance concurrent IP lookups across
//! multiple threads.
//!
//! ## Architectural Highlights
//! - `Reader` is completely read-only, lock-free, and thread-safe (`Send + Sync`).
//! - No `Mutex`, `RwLock`, or atomic reference counter contention on the lookup hot path.
//! - The cache-aligned fast tree is ready at open, before worker threads start.
//! - Multiple threads can share an `Arc<Reader<'static>>` or reference a `&Reader`
//!   inside `std::thread::scope` without any synchronization bottlenecks.
//!
//! Run this example with:
//! ```bash
//! cargo run --example concurrent_lookups
//! ```

use std::error::Error;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use libmaxminddb_rs::{MetadataBuilder, MmdbDecode, MmdbEncode, Reader, Writer};

#[derive(Debug, MmdbEncode, MmdbDecode)]
struct RoutingRecord<'a> {
    asn: u32,
    datacenter: &'a str,
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== libmaxminddb-rs Concurrent Multi-Threaded Lookups Example ===\n");

    // -----------------------------------------------------------------------
    // 1. Build a test database with several subnets
    // -----------------------------------------------------------------------
    println!("1. Building test database...");
    let metadata = MetadataBuilder::new()
        .database_type("Concurrent-Test-DB")
        .ip_version(4)
        .description("en", "Concurrent multi-threaded benchmark database")
        .build()?;

    let mut writer = Writer::with_metadata(metadata);
    writer.insert_encoded(
        "10.0.0.0/8".parse()?,
        &RoutingRecord {
            asn: 1001,
            datacenter: "us-east-1",
        },
    )?;
    writer.insert_encoded(
        "172.16.0.0/12".parse()?,
        &RoutingRecord {
            asn: 1002,
            datacenter: "eu-central-1",
        },
    )?;
    writer.insert_encoded(
        "192.168.0.0/16".parse()?,
        &RoutingRecord {
            asn: 1003,
            datacenter: "ap-northeast-1",
        },
    )?;

    let db_bytes = writer.finish()?;
    println!("   Database created ({} bytes).\n", db_bytes.len());

    // -----------------------------------------------------------------------
    // 2. Open the database into an owned reader and wrap in Arc
    // -----------------------------------------------------------------------
    println!("2. Wrapping Reader in Arc for lock-free cross-thread sharing...");
    // `Reader::from_vec` creates a Reader<'static> that can be shared across threads.
    let reader = Arc::new(Reader::from_vec(db_bytes)?);

    // -----------------------------------------------------------------------
    // 3. Spawn worker threads and perform concurrent lookups
    // -----------------------------------------------------------------------
    let num_threads = 4;
    let lookups_per_thread = 250_000;
    let total_lookups = num_threads * lookups_per_thread;
    let successful_lookups = Arc::new(AtomicUsize::new(0));

    let sample_ips: Vec<IpAddr> = vec![
        "10.1.2.3".parse()?,
        "172.20.10.5".parse()?,
        "192.168.1.100".parse()?,
    ];

    println!(
        "3. Spawning {} threads to execute {} total lookups...",
        num_threads, total_lookups
    );

    let start_time = Instant::now();

    std::thread::scope(|scope| {
        for thread_idx in 0..num_threads {
            let reader = Arc::clone(&reader);
            let counter = Arc::clone(&successful_lookups);
            let sample_ips = &sample_ips;

            scope.spawn(move || {
                let mut local_hits = 0;
                for i in 0..lookups_per_thread {
                    let ip = sample_ips[(thread_idx + i) % sample_ips.len()];
                    // `lookup_value` is completely lock-free:
                    if reader.lookup_value(ip).is_ok() {
                        local_hits += 1;
                    }
                }
                counter.fetch_add(local_hits, Ordering::Relaxed);
            });
        }
    });

    let elapsed = start_time.elapsed();
    let hits = successful_lookups.load(Ordering::SeqCst);
    let throughput = (hits as f64) / elapsed.as_secs_f64();

    println!("\n4. Benchmark results:");
    println!("   ↳ Total lookups     : {}", hits);
    println!("   ↳ Elapsed time      : {:.2?}", elapsed);
    println!(
        "   ↳ Average latency   : {:.1?} per lookup",
        elapsed / hits as u32
    );
    println!(
        "   ↳ Total Throughput  : {:.2} M ops/s\n",
        throughput / 1_000_000.0
    );

    println!("Concurrent lookups completed successfully with zero lock contention!");
    Ok(())
}
