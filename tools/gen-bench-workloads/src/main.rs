//! Fast workload generator for MMDB benchmarks.

use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::time::Instant;

struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_ipv4(&mut self) -> Ipv4Addr {
        Ipv4Addr::from((self.next_u64() >> 32) as u32)
    }

    fn next_ipv6(&mut self) -> Ipv6Addr {
        let hi = self.next_u64();
        let lo = self.next_u64();
        Ipv6Addr::from(((hi as u128) << 64) | lo as u128)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    
    let mut workload_dir: Option<PathBuf> = None;
    let mut count: usize = 1_000_000;
    let mut patterns: Vec<String> = vec!["random".to_string(), "sequential".to_string(), "hot".to_string(), "absent".to_string()];
    let mut families: Vec<String> = vec!["ipv4".to_string(), "ipv6".to_string()];
    
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--workload-dir" => {
                i += 1;
                if i < args.len() {
                    workload_dir = Some(PathBuf::from(&args[i]));
                }
            }
            "--count" => {
                i += 1;
                if i < args.len() {
                    count = args[i].parse()?;
                }
            }
            "--patterns" => {
                i += 1;
                if i < args.len() {
                    patterns = args[i].split(',').map(|s| s.to_string()).collect();
                }
            }
            "--families" => {
                i += 1;
                if i < args.len() {
                    families = args[i].split(',').map(|s| s.to_string()).collect();
                }
            }
            _ => {}
        }
        i += 1;
    }
    
    let workload_dir = workload_dir.expect("Error: --workload-dir is required");
    
    fs::create_dir_all(&workload_dir)?;
    
    let start = Instant::now();
    println!("Generating workloads to {}...", workload_dir.display());
    
    let seed = 0x5EED2024CAFEBABE;
    
    for family in &families {
        for pattern in &patterns {
            let filename = format!("{}-{}.txt", family, pattern);
            let filepath = workload_dir.join(&filename);
            println!("  Generating {}...", filename);
            generate_workload(&filepath, family, pattern, count, seed)?;
        }
    }
    
    let elapsed = start.elapsed();
    println!("Workload generation complete in {:.2}s", elapsed.as_secs_f64());
    
    Ok(())
}

fn generate_workload(
    filepath: &PathBuf,
    family: &str,
    pattern: &str,
    count: usize,
    seed: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(filepath)?;
    let mut writer = BufWriter::with_capacity(1024 * 1024, file);
    
    let mut rng = SplitMix64::new(seed);
    
    if family == "ipv4" {
        match pattern {
            "sequential" => {
                let base: u32 = 0x51000000; // 81.0.0.0
                for i in 0..count {
                    let ip = Ipv4Addr::from(base.wrapping_add(i as u32));
                    writeln!(writer, "{}", ip)?;
                }
            }
            "hot" => {
                let ip = Ipv4Addr::new(81, 2, 69, 160);
                for _ in 0..count {
                    writeln!(writer, "{}", ip)?;
                }
            }
            "absent" => {
                // 240.0.0.0/4 (reserved, never present in GeoIP2 database)
                for _ in 0..count {
                    let suffix = (rng.next_u64() as u32) & 0x0FFF_FFFF;
                    let ip = Ipv4Addr::from(0xF000_0000 | suffix);
                    writeln!(writer, "{}", ip)?;
                }
            }
            _ => { // "random"
                for _ in 0..count {
                    let ip = rng.next_ipv4();
                    writeln!(writer, "{}", ip)?;
                }
            }
        }
    } else {
        match pattern {
            "sequential" => {
                let base: u128 = 0x2001_0db8_0000_0000_0000_0000_0000_0000;
                for i in 0..count {
                    let ip = Ipv6Addr::from(base.wrapping_add(i as u128));
                    writeln!(writer, "{}", ip)?;
                }
            }
            "hot" => {
                let ip: Ipv6Addr = "2001:db8:123::1".parse()?;
                for _ in 0..count {
                    writeln!(writer, "{}", ip)?;
                }
            }
            "absent" => {
                // 3fff::/20 (unassigned / documentation, never present in GeoIP2 database)
                for _ in 0..count {
                    let hi = 0x3FFF_0000_0000_0000_u64 | (rng.next_u64() & 0x0000_FFFF_FFFF_FFFF);
                    let lo = rng.next_u64();
                    let ip = Ipv6Addr::from(((hi as u128) << 64) | lo as u128);
                    writeln!(writer, "{}", ip)?;
                }
            }
            _ => { // "random"
                for _ in 0..count {
                    let ip = rng.next_ipv6();
                    writeln!(writer, "{}", ip)?;
                }
            }
        }
    }
    
    writer.flush()?;
    Ok(())
}
