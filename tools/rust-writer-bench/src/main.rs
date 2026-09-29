//! Standalone MMDB writer benchmark harness for libmaxminddb-rs.
//!
//! Produces identical JSON metrics for comparison against mmdbwriter (Go).
//!
//! Memory optimizations:
//! - Single shared Arc<Value> record reused for all insertions
//! - Batch processing with configurable batch size
//! - Streaming IP generation without accumulation
//! - Memory usage monitoring throughout

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::hint::black_box;
use std::io::Write;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use libmaxminddb_rs::{IpNetwork, MetadataBuilder, Value, Writer};
use serde::Serialize;

fn map(pairs: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Map(BTreeMap::from_iter(
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)),
    ))
}

/// Creates a single shared Arc<Value> to avoid cloning for each insertion.
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
                ("accuracy_radius", Value::Uint16(50)),
                ("latitude", Value::Double(37.751)),
                ("longitude", Value::Double(-122.42)),
                ("time_zone", Value::Utf8("America/Los_Angeles".to_string())),
            ]),
        ),
    ]))
}

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    fn next_ipv4_net(&mut self) -> IpNetwork {
        let ip_u32 = self.next_u32();
        let prefix = 16 + (self.next_u32() % 16) as u8; // /16 to /31
        let mask = if prefix == 0 {
            0
        } else {
            !0u32 << (32 - prefix)
        };
        let net_ip = Ipv4Addr::from(ip_u32 & mask);
        IpNetwork::new(net_ip.into(), prefix).unwrap()
    }

    fn next_ipv6_net(&mut self) -> IpNetwork {
        let hi = self.next_u64();
        let lo = self.next_u64();
        let ip_u128 = ((hi as u128) << 64) | (lo as u128);
        let prefix = 48 + (self.next_u32() % 48) as u8; // /48 to /95
        let mask = if prefix == 0 {
            0
        } else {
            !0u128 << (128 - prefix)
        };
        let net_ip = Ipv6Addr::from(ip_u128 & mask);
        IpNetwork::new(net_ip.into(), prefix).unwrap()
    }
}

fn parse_seed(s: &str) -> u64 {
    let s = s.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        u64::from_str_radix(&s[2..], 16).unwrap_or(0x5EED2024CAFEBABE)
    } else {
        s.parse::<u64>().unwrap_or(0x5EED2024CAFEBABE)
    }
}

fn current_rss_bytes() -> u64 {
    if let Ok(content) = fs::read_to_string("/proc/self/statm") {
        let parts: Vec<&str> = content.split_whitespace().collect();
        if parts.len() >= 2 {
            if let Ok(pages) = parts[1].parse::<u64>() {
                let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 };
                return pages.saturating_mul(page_size);
            }
        }
    }
    peak_rss_bytes()
}

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

fn percentile(sorted: &[u64], q: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[index]
}

/// Memory stats tracker for detailed reporting
#[derive(Debug, Clone, Default)]
struct MemoryStats {
    initial_rss: u64,
    peak_rss: u64,
    samples: Vec<(usize, u64)>, // (inserted_count, rss)
}

impl MemoryStats {
    fn new() -> Self {
        Self {
            initial_rss: current_rss_bytes(),
            peak_rss: 0,
            samples: Vec::with_capacity(100),
        }
    }

    fn sample(&mut self, inserted: usize) {
        let rss = current_rss_bytes();
        if rss > self.peak_rss {
            self.peak_rss = rss;
        }
        if inserted % 100_000 == 0 || inserted == 1 {
            self.samples.push((inserted, rss));
        }
    }

    fn finish(&mut self) {
        let final_peak = peak_rss_bytes();
        if final_peak > self.peak_rss {
            self.peak_rss = final_peak;
        }
    }

    fn report(&self, total_entries: usize) -> (u64, u64, u64, i64) {
        let final_rss = current_rss_bytes();
        let peak = self.peak_rss.max(peak_rss_bytes());
        let delta = final_rss as i64 - self.initial_rss as i64;
        
        // Print memory report
        eprintln!("\n=== Memory Usage ===");
        eprintln!("Initial RSS:    {:>12} bytes ({:.2} MB)", 
                 self.initial_rss, self.initial_rss as f64 / 1_048_576.0);
        eprintln!("Final RSS:      {:>12} bytes ({:.2} MB)", 
                 final_rss, final_rss as f64 / 1_048_576.0);
        eprintln!("Peak RSS:       {:>12} bytes ({:.2} MB)", 
                 peak, peak as f64 / 1_048_576.0);
        eprintln!("Peak - Initial: {:>12} bytes ({:.2} MB)", 
                 peak.saturating_sub(self.initial_rss),
                 (peak.saturating_sub(self.initial_rss)) as f64 / 1_048_576.0);
        
        if total_entries > 0 {
            eprintln!("Peak per entry: {:.2} bytes", peak as f64 / total_entries as f64);
        }
        
        (self.initial_rss, final_rss, peak, delta)
    }
}

#[derive(Debug, Serialize)]
struct WriterBenchResult {
    schema_version: u32,
    implementation: &'static str,
    language: &'static str,
    version: &'static str,
    scenario: &'static str,
    family: String,
    database_size: usize,
    workload_size: usize,
    entries: usize,
    batch_size: usize,
    wall_time_ns: u64,
    insert_time_ns: u64,
    build_time_ns: u64,
    throughput_ops_s: f64,
    mean_ns: f64,
    median_ns: f64,
    min_ns: u64,
    max_ns: u64,
    p50_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
    stddev_ns: f64,
    database_bytes: u64,
    rss_before_bytes: u64,
    rss_after_bytes: u64,
    rss_delta_bytes: i64,
    avg_rss_bytes: u64,
    peak_rss_bytes: u64,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut size: usize = 10_000;
    let mut batch_size: usize = 100_000;
    let mut family = "ipv4".to_string();
    let mut seed: u64 = 0x5EED2024CAFEBABE;
    let mut output_path: Option<PathBuf> = None;

    let mut idx = 1;
    while idx < args.len() {
        match args[idx].as_str() {
            "--size" | "--entries" | "-size" | "-entries" => {
                if idx + 1 < args.len() {
                    size = args[idx + 1].parse().unwrap_or(size);
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            "--batch-size" | "--batch" | "-batch-size" | "-batch" => {
                if idx + 1 < args.len() {
                    batch_size = args[idx + 1].parse().unwrap_or(batch_size);
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            "--family" | "-family" => {
                if idx + 1 < args.len() {
                    family = args[idx + 1].to_ascii_lowercase();
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            "--seed" | "-seed" => {
                if idx + 1 < args.len() {
                    seed = parse_seed(&args[idx + 1]);
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            "--output" | "-output" => {
                if idx + 1 < args.len() {
                    output_path = Some(PathBuf::from(&args[idx + 1]));
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            _ => {
                idx += 1;
            }
        }
    }

    if batch_size == 0 {
        batch_size = 100_000;
    }

    let ip_version = if family == "ipv6" || family == "6" {
        6
    } else {
        4
    };
    
    // KEY OPTIMIZATION: Single shared Arc<Value>
    let shared_rec = shared_record_arc();
    let mut rng = SplitMix64::new(seed);

    let max_samples = 25_000.min(size);
    let sample_stride = (size / max_samples).max(1);
    let mut sample_latencies: Vec<u64> = Vec::with_capacity(max_samples + 16);

    let _rss_before = current_rss_bytes();
    let mut rss_sum: u64 = 0;
    let mut rss_samples: u64 = 0;
    let mut memory_stats = MemoryStats::new();

    let t_total_start = Instant::now();

    // 1. Initialize Writer
    let metadata = MetadataBuilder::new()
        .ip_version(ip_version)
        .database_type("GeoIP2-City")
        .build()
        .expect("failed to build metadata");
    
    // Pre-allocate with estimated capacity
    let estimated_nodes = if ip_version == 4 {
        size * 24
    } else {
        size * 128
    };
    let mut writer = Writer::with_metadata_and_capacity(
        metadata,
        estimated_nodes.min(1_000_000)
    );

    // 2. Perform Batch Insertions with streaming approach
    // OPTIMIZATION: Reusable batch buffer, no accumulation of all data
    let mut batch_buffer: Vec<IpNetwork> = Vec::with_capacity(batch_size);
    let t_insert_start = Instant::now();
    let mut inserted = 0;
    
    eprintln!("Starting insertion of {} entries (batch size: {})", size, batch_size);
    
    while inserted < size {
        let chunk = (size - inserted).min(batch_size);
        
        // Generate batch of networks (only network structures, not values)
        batch_buffer.clear();
        for _ in 0..chunk {
            let net = if ip_version == 4 {
                rng.next_ipv4_net()
            } else {
                rng.next_ipv6_net()
            };
            batch_buffer.push(net);
        }
        
        // Insert batch using shared Arc<Value> - ZERO COPY for values!
        for (i_in_chunk, &net) in batch_buffer.iter().enumerate() {
            let global_idx = inserted + i_in_chunk;
            if global_idx % sample_stride == 0 && sample_latencies.len() < max_samples {
                let t0 = Instant::now();
                writer
                    .insert_value_shared(net, Arc::clone(&shared_rec))
                    .unwrap();
                let elapsed_ns = t0.elapsed().as_nanos() as u64;
                sample_latencies.push(elapsed_ns);
            } else {
                writer
                    .insert_value_shared(net, Arc::clone(&shared_rec))
                    .unwrap();
            }
        }
        
        inserted += chunk;
        
        // Memory monitoring
        memory_stats.sample(inserted);
        let cur_rss = current_rss_bytes();
        rss_sum += cur_rss;
        rss_samples += 1;
        
        // Progress reporting for large datasets
        if size >= 100_000 && inserted % (size / 10) == 0 {
            eprintln!("  {} entries inserted...", inserted);
        }
    }
    
    let insert_time_ns = t_insert_start.elapsed().as_nanos() as u64;

    // 3. Finish / Serialize database & write to disk
    eprintln!("Finalizing database...");
    let t_build_start = Instant::now();
    let db_bytes = writer.finish().expect("failed to serialize database");
    black_box(&db_bytes);

    let target_path = output_path.clone().unwrap_or_else(|| {
        env::temp_dir().join(format!("libmaxminddb_rs_bench_{}.mmdb", std::process::id()))
    });
    if let Some(parent) = target_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = File::create(&target_path).expect("failed to create output file");
    file.write_all(&db_bytes).expect("failed to write MMDB");
    file.sync_all().expect("failed to sync MMDB");
    drop(file);
    if output_path.is_none() {
        let _ = fs::remove_file(&target_path);
    }

    let build_time_ns = t_build_start.elapsed().as_nanos() as u64;
    let wall_time_ns = t_total_start.elapsed().as_nanos() as u64;
    let rss_after = current_rss_bytes();
    
    // Get detailed memory report
    memory_stats.finish();
    let (rss_before_detailed, rss_after_detailed, peak_rss, _rss_delta) = memory_stats.report(size);
    
    // Use the detailed memory stats
    let rss_before = rss_before_detailed;
    let peak_rss = peak_rss;
    let rss_delta = rss_after_detailed as i64 - rss_before_detailed as i64;
    let database_bytes = db_bytes.len() as u64;

    let avg_rss_bytes = if rss_samples > 0 {
        rss_sum / rss_samples
    } else {
        rss_after
    };

    // Compute stats
    sample_latencies.sort_unstable();
    let n = sample_latencies.len() as f64;
    let sum: u64 = sample_latencies.iter().sum();
    let mean_ns = if n > 0.0 { sum as f64 / n } else { 0.0 };
    let variance = if n > 1.0 {
        sample_latencies
            .iter()
            .map(|&v| {
                let diff = v as f64 - mean_ns;
                diff * diff
            })
            .sum::<f64>()
            / (n - 1.0)
    } else {
        0.0
    };
    let stddev_ns = variance.sqrt();

    let p50_ns = percentile(&sample_latencies, 0.50);
    let p95_ns = percentile(&sample_latencies, 0.95);
    let p99_ns = percentile(&sample_latencies, 0.99);
    let median_ns = p50_ns as f64;
    let min_ns = sample_latencies.first().copied().unwrap_or(0);
    let max_ns = sample_latencies.last().copied().unwrap_or(0);

    let throughput_ops_s = if wall_time_ns > 0 {
        (size as f64 / wall_time_ns as f64) * 1_000_000_000.0
    } else {
        0.0
    };

    let result = WriterBenchResult {
        schema_version: 2,
        implementation: "libmaxminddb-rs",
        language: "rust",
        version: "0.1.0",
        scenario: "writer_generation",
        family,
        database_size: size,
        workload_size: size,
        entries: size,
        batch_size,
        wall_time_ns,
        insert_time_ns,
        build_time_ns,
        throughput_ops_s,
        mean_ns,
        median_ns,
        min_ns,
        max_ns,
        p50_ns,
        p95_ns,
        p99_ns,
        stddev_ns,
        database_bytes,
        rss_before_bytes: rss_before,
        rss_after_bytes: rss_after,
        rss_delta_bytes: rss_delta,
        avg_rss_bytes,
        peak_rss_bytes: peak_rss,
    };

    eprintln!("\n=== Performance Summary ===");
    eprintln!("Total time:      {:.2}s", wall_time_ns as f64 / 1_000_000_000.0);
    eprintln!("Insert time:     {:.2}s", insert_time_ns as f64 / 1_000_000_000.0);
    eprintln!("Build time:      {:.2}s", build_time_ns as f64 / 1_000_000_000.0);
    eprintln!("Throughput:     {:.1} ops/s", throughput_ops_s);
    eprintln!("Mean latency:   {:.1} ns", mean_ns);
    eprintln!("Database size:  {} bytes ({:.2} MB)", database_bytes, database_bytes as f64 / 1_048_576.0);
    
    println!("{}", serde_json::to_string(&result).unwrap());
}
