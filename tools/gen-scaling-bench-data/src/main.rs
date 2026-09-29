//! Generator for GeoIP2-City-compatible MMDB databases of various sizes
//! for scaling benchmarks.
//!
//! This tool generates deterministic databases that are compatible with:
//! - libmaxminddb-rs
//! - maxminddb-rust
//! - geoip2-rs
//! - libmaxminddb (C)
//!
//! Memory optimizations:
//! - Generates IP addresses on-the-fly using deterministic SplitMix64 PRNG
//! - Binary IP representations (u32 for IPv4, [u8; 16] for IPv6)
//! - Single shared Arc<Value> record reused for all insertions
//! - Zero unnecessary allocations or string formatting
//! - Batch processing with configurable batch size
//! - Streaming insertion without accumulating all data in memory
//! - Memory usage monitoring via /proc/self/statm

mod absent;
mod memory;

use std::collections::BTreeMap;
use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use libmaxminddb_rs::{IpNetwork, MetadataBuilder, Value, Writer};

fn map(pairs: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Map(BTreeMap::from_iter(
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)),
    ))
}

/// Creates a shared Arc<Value> record to avoid cloning for each insertion.
/// This is the single most important optimization for memory usage.
fn shared_record_arc() -> Arc<Value> {
    Arc::new(map([
        (
            "city",
            map([
                ("geoname_id", Value::Uint32(2643743)),
                (
                    "names",
                    map([("en", Value::Utf8("Benchmark City".to_string()))]),
                ),
            ]),
        ),
        (
            "continent",
            map([
                ("code", Value::Utf8("NA".to_string())),
                ("geoname_id", Value::Uint32(6255149)),
                (
                    "names",
                    map([("en", Value::Utf8("North America".to_string()))]),
                ),
            ]),
        ),
        (
            "country",
            map([
                ("iso_code", Value::Utf8("US".to_string())),
                ("geoname_id", Value::Uint32(6252001)),
                (
                    "names",
                    map([("en", Value::Utf8("United States".to_string()))]),
                ),
            ]),
        ),
        (
            "location",
            map([
                ("accuracy_radius", Value::Uint16(1000)),
                ("latitude", Value::Double(37.751)),
                ("longitude", Value::Double(-97.822)),
                ("time_zone", Value::Utf8("America/Chicago".to_string())),
            ]),
        ),
    ]))
}

/// Deterministic splitmix64 generator with a fixed seed (reproducible runs).
#[derive(Clone)]
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    #[inline(always)]
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    #[inline(always)]
    fn next_ipv4(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    #[inline(always)]
    fn next_ipv6(&mut self) -> [u8; 16] {
        let mut octets = [0u8; 16];
        let (high, low) = octets.split_at_mut(8);
        high.copy_from_slice(&self.next_u64().to_be_bytes());
        low.copy_from_slice(&self.next_u64().to_be_bytes());
        octets
    }
}

fn parse_seed(raw: &str) -> u64 {
    let clean = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(raw);
    u64::from_str_radix(clean, 16)
        .unwrap_or_else(|_| clean.parse::<u64>().unwrap_or(0x1234_5678_9ABC_DEF0))
}

/// Memory monitoring utilities
#[cfg(target_os = "linux")]
fn current_rss_bytes() -> u64 {
    if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
        let parts: Vec<&str> = content.split_whitespace().collect();
        if parts.len() >= 2 {
            if let Ok(pages) = parts[1].parse::<u64>() {
                let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 };
                return pages.saturating_mul(page_size);
            }
        }
    }
    0
}

#[cfg(target_os = "linux")]
fn peak_rss_bytes() -> u64 {
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0 {
            (usage.ru_maxrss as u64).saturating_mul(1024)
        } else {
            0
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn current_rss_bytes() -> u64 {
    0
}

#[cfg(not(target_os = "linux"))]
fn peak_rss_bytes() -> u64 {
    0
}

/// Memory stats tracker
#[derive(Debug, Clone, Default)]
struct MemoryStats {
    initial_rss: u64,
    current_rss: u64,
    peak_rss: u64,
    samples: Vec<(usize, u64)>, // (inserted_count, rss_at_that_point)
}

impl MemoryStats {
    fn new() -> Self {
        Self {
            initial_rss: current_rss_bytes(),
            current_rss: 0,
            peak_rss: 0,
            samples: Vec::with_capacity(1000),
        }
    }

    fn sample(&mut self, inserted: usize) {
        let rss = current_rss_bytes();
        self.current_rss = rss;
        if rss > self.peak_rss {
            self.peak_rss = rss;
        }
        // Sample every 100k entries or at specific milestones
        if inserted % 100_000 == 0 || inserted == 1 {
            self.samples.push((inserted, rss));
        }
    }

    fn finish(&mut self) {
        self.current_rss = current_rss_bytes();
        let final_peak = peak_rss_bytes();
        if final_peak > self.peak_rss {
            self.peak_rss = final_peak;
        }
    }

    fn report(&self, total_entries: usize) {
        println!("\n=== Memory Usage Report ===");
        println!(
            "Initial RSS:    {:>12} bytes ({:.2} MB)",
            self.initial_rss,
            self.initial_rss as f64 / 1_048_576.0
        );
        println!(
            "Final RSS:      {:>12} bytes ({:.2} MB)",
            self.current_rss,
            self.current_rss as f64 / 1_048_576.0
        );
        println!(
            "Peak RSS:       {:>12} bytes ({:.2} MB)",
            self.peak_rss,
            self.peak_rss as f64 / 1_048_576.0
        );
        println!(
            "Peak - Initial: {:>12} bytes ({:.2} MB)",
            self.peak_rss.saturating_sub(self.initial_rss),
            (self.peak_rss.saturating_sub(self.initial_rss)) as f64 / 1_048_576.0
        );

        if !self.samples.is_empty() {
            println!("\nMemory growth samples:");
            println!("{:>12} entries -> {:>12} bytes", 0, self.initial_rss);
            for (count, rss) in &self.samples {
                println!(
                    "{:>12} entries -> {:>12} bytes ({:.2} MB)",
                    count,
                    rss,
                    *rss as f64 / 1_048_576.0
                );
            }
        }

        // Calculate bytes per entry at peak
        if total_entries > 0 {
            let bytes_per_entry = self.peak_rss as f64 / total_entries as f64;
            println!("\nPeak memory per entry: {:.2} bytes", bytes_per_entry);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage(&args[0]);
        std::process::exit(2);
    }

    match args[1].as_str() {
        "memory" => {
            let output = PathBuf::from(args.get(2).expect("output directory"));
            let count = args
                .get(3)
                .expect("entry count")
                .parse()
                .expect("valid count");
            memory::generate(&output, count).expect("generate verified RSS fixture");
        }
        "ipv6-absent" => {
            let output = PathBuf::from(args.get(2).expect("output directory"));
            let count = args
                .get(3)
                .map(|s| s.parse().expect("count"))
                .unwrap_or(1_000_000);
            absent::generate(&output, count).expect("generate verified IPv6 fixture");
        }
        "workload" => {
            if args.len() < 5 {
                eprintln!(
                    "usage: {} workload <output_file> <ip_version: 4|6> <count> [hex_seed]",
                    args[0]
                );
                std::process::exit(2);
            }
            let output_file = PathBuf::from(&args[2]);
            let family = args[3].to_ascii_lowercase();
            let count: usize = args[4].parse().expect("valid count");
            let seed = if args.len() >= 6 {
                parse_seed(&args[5])
            } else {
                0x1234_5678_9ABC_DEF0
            };

            generate_workload(&output_file, &family, count, seed);
        }
        "db" => {
            if args.len() < 5 {
                eprintln!(
                    "usage: {} db <output_mmdb> <size> <ip_version: 4|6> [hex_seed] [batch_size]",
                    args[0]
                );
                std::process::exit(2);
            }
            let output_mmdb = PathBuf::from(&args[2]);
            let size: usize = args[3].parse().expect("valid size");
            let family = args[4].to_ascii_lowercase();
            let seed = if args.len() >= 6 {
                parse_seed(&args[5])
            } else {
                0x1234_5678_9ABC_DEF0
            };
            // Parse optional batch size, default to 1000 for good memory/performance balance
            let batch_size = if args.len() >= 7 {
                args[6].parse().unwrap_or(100_000)
            } else {
                1000
            };

            generate_db(&output_mmdb, size, &family, seed, batch_size);
        }
        // Backward-compatible mode: <output_dir> <size> <ip_version: 4|6> [hex_seed] [batch_size]
        _ => {
            if args.len() < 4 {
                print_usage(&args[0]);
                std::process::exit(2);
            }
            let output_dir = PathBuf::from(&args[1]);
            let size: usize = args[2].parse().expect("valid size");
            let family = args[3].to_ascii_lowercase();
            let seed = if args.len() >= 5 {
                parse_seed(&args[4])
            } else {
                0x1234_5678_9ABC_DEF0
            };
            let batch_size = if args.len() >= 6 {
                args[5].parse().unwrap_or(100_000)
            } else {
                1000
            };

            let sub = output_dir.join(format!("size_{size}_{family}"));
            std::fs::create_dir_all(&sub).expect("create output directory");
            let mmdb_path = sub.join(format!("{family}.mmdb"));
            let workload_path = sub.join(format!("{family}-random.txt"));

            generate_workload(&workload_path, &family, 1000.min(size), seed);
            generate_db(&mmdb_path, size, &family, seed, batch_size);
        }
    }
}

fn print_usage(prog: &str) {
    eprintln!("usage: {prog} workload <output_file> <ip_version: 4|6> <count> [hex_seed]");
    eprintln!("       {prog} db <output_mmdb> <size> <ip_version: 4|6> [hex_seed] [batch_size]");
    eprintln!("       {prog} <output_dir> <size> <ip_version: 4|6> [hex_seed] [batch_size]");
    eprintln!("\nOptimized MMDB generator with memory-efficient batch processing.");
    eprintln!("Default batch size: 100_000 entries (adjust for memory/performance tradeoff)");
}

fn generate_workload(path: &PathBuf, family: &str, count: usize, seed: u64) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create workload dir");
    }
    let file = File::create(path).expect("create workload file");
    let mut writer = BufWriter::with_capacity(64 * 1024, file);
    let mut rng = SplitMix64::new(seed);

    match family {
        "4" | "ipv4" => {
            for _ in 0..count {
                let ip = Ipv4Addr::from(rng.next_ipv4());
                writeln!(writer, "{ip}").expect("write ipv4");
            }
        }
        _ => {
            for _ in 0..count {
                let ip = Ipv6Addr::from(rng.next_ipv6());
                writeln!(writer, "{ip}").expect("write ipv6");
            }
        }
    }
    writer.flush().expect("flush workload");
}

/// Optimized database generation with streaming batch processing.
///
/// Key optimizations:
/// 1. Single Arc<Value> shared across all insertions (zero-copy for value data)
/// 2. Batch processing with configurable batch size
/// 3. IP addresses generated on-the-fly (no storage)
/// 4. Reusable batch buffer (cleared between batches)
/// 5. Memory monitoring throughout the process
fn generate_db(path: &PathBuf, size: usize, family: &str, seed: u64, batch_size: usize) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create db dir");
    }

    let ip_version = if family == "4" || family == "ipv4" {
        4
    } else {
        6
    };

    println!(
        "Generating {} MMDB with {} entries (batch size: {})",
        family, size, batch_size
    );
    println!("Using optimized streaming insertion...");

    let start_time = Instant::now();
    let mut memory_stats = MemoryStats::new();

    let metadata = MetadataBuilder::new()
        .ip_version(ip_version)
        .database_type("GeoIP2-City")
        .build()
        .expect("metadata builder");

    // SINGLE SHARED RECORD - This is the key optimization!
    // Instead of cloning a Value 50M times, we create ONE Arc<Value> and reuse it.
    let shared_rec = shared_record_arc();

    // Pre-allocate writer with estimated capacity
    // For IPv4: ~24 nodes per /32 entry (worst case)
    // For IPv6: ~128 nodes per /128 entry (worst case)
    let estimated_nodes = if ip_version == 4 {
        size * 24
    } else {
        size * 128
    };
    let mut writer = Writer::with_metadata_and_capacity(metadata, estimated_nodes.min(1_000_000));

    let mut rng = SplitMix64::new(seed);

    // Reusable batch buffer - allocated once, cleared and reused
    let mut batch_buffer: Vec<IpNetwork> = Vec::with_capacity(batch_size);

    let mut inserted = 0;
    let mut last_report = 0;
    let report_interval = (size / 10).max(1);

    while inserted < size {
        let chunk = (size - inserted).min(batch_size);

        // Phase 1: Generate IP networks for this batch
        // This allocates only the network structures, not the values
        batch_buffer.clear();
        for _ in 0..chunk {
            if ip_version == 4 {
                let addr = rng.next_ipv4();
                let net = IpNetwork::V4(ipnet::Ipv4Net::new(Ipv4Addr::from(addr), 32).unwrap());
                batch_buffer.push(net);
            } else {
                let octets = rng.next_ipv6();
                let net = IpNetwork::V6(ipnet::Ipv6Net::new(Ipv6Addr::from(octets), 128).unwrap());
                batch_buffer.push(net);
            }
        }

        // Phase 2: Insert all networks in batch using shared Arc<Value>
        writer
            .insert_batch_shared(batch_buffer.iter().copied(), &shared_rec)
            .expect("insert");
        inserted += chunk;

        // Sample memory usage periodically
        memory_stats.sample(inserted);

        // Progress reporting
        if inserted - last_report >= report_interval || inserted == size {
            let elapsed = start_time.elapsed();
            let rate = inserted as f64 / elapsed.as_secs_f64();
            println!(
                "  {} entries ({:.1}%) - {:.1} entries/sec - RSS: {} MB",
                inserted,
                (inserted as f64 * 100.0 / size as f64),
                rate,
                current_rss_bytes() as f64 / 1_048_576.0
            );
            last_report = inserted;
        }
    }

    // Finalize and write database
    println!("\nFinalizing database...");
    let finish_start = Instant::now();
    let bytes = writer.finish().expect("writer finish");
    let finish_time = finish_start.elapsed();

    let mut file = File::create(path).expect("create mmdb file");
    file.write_all(&bytes).expect("write mmdb");
    file.flush().expect("flush mmdb");
    drop(file);

    memory_stats.finish();
    memory_stats.report(size);

    let total_time = start_time.elapsed();
    let insert_time = total_time - finish_time;

    println!("\n=== Generation Summary ===");
    println!("Total entries:    {}", size);
    println!(
        "Database size:   {} bytes ({:.2} MB)",
        bytes.len(),
        bytes.len() as f64 / 1_048_576.0
    );
    println!("Insert time:     {:.2}s", insert_time.as_secs_f64());
    println!("Finish time:     {:.2}s", finish_time.as_secs_f64());
    println!("Total time:      {:.2}s", total_time.as_secs_f64());
    println!(
        "Insert rate:     {:.1} entries/sec",
        size as f64 / insert_time.as_secs_f64()
    );
    println!("Batch size:      {}", batch_size);

    // Suggest optimal batch size based on memory usage
    let peak_memory_mb = memory_stats.peak_rss as f64 / 1_048_576.0;
    if peak_memory_mb > 2048.0 {
        println!(
            "\n⚠️  WARNING: Peak memory usage is {:.0} MB",
            peak_memory_mb
        );
        println!(
            "  Consider reducing batch size from {} to reduce memory pressure",
            batch_size
        );
        let suggested = (batch_size as f64 * 0.7) as usize;
        println!("  Suggested batch size: {}", suggested);
    }
}
