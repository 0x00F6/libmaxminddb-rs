//! Replay the comparison's exact typed lookup, with enough iterations to time
//! nanosecond miss paths without a clock read around every individual lookup.
use std::{hint::black_box, net::IpAddr, path::PathBuf, time::Duration};

use criterion::{Criterion, criterion_group, criterion_main};
use libmaxminddb_rs::{Error, Reader};

#[path = "../tools/rust-competitor-bench/src/city.rs"]
mod city;

fn benchmarks(c: &mut Criterion) {
    let dir = PathBuf::from(
        std::env::var_os("MMDB_BENCH_DIR").unwrap_or_else(|| "target/bench-data".into()),
    );
    for family in ["ipv4", "ipv6"] {
        for pattern in ["hot", "random", "sequential", "absent"] {
            let large = family == "ipv6" && pattern == "absent";
            let workloads = dir.join(if large { "ipv6-absent-v1" } else { "workloads" });
            let path = if large {
                workloads.join("ipv6.mmdb")
            } else {
                dir.join("GeoIP2-City-Bench.mmdb")
            };
            // SAFETY: these generated fixtures remain immutable throughout the run.
            let reader = unsafe { Reader::open_mmap(path).unwrap() };
            let ips: Vec<IpAddr> = if pattern == "hot" {
                vec![
                    if family == "ipv4" {
                        "81.2.69.160"
                    } else {
                        "2001:db8:123::1"
                    }
                    .parse()
                    .unwrap(),
                ]
            } else {
                std::fs::read_to_string(workloads.join(format!("{family}-{pattern}.txt")))
                    .unwrap()
                    .lines()
                    .map(|line| line.parse().unwrap())
                    .collect()
            };
            // Warm preparation and verify every query before the timing loop.
            for &ip in &ips {
                match city::lookup(&reader, ip) {
                    Ok(_) => assert_ne!(pattern, "absent"),
                    Err(Error::NotFound) => assert_ne!(pattern, "hot"),
                    Err(error) => panic!("invalid fixture: {error}"),
                }
            }
            c.bench_function(&format!("lookup_borrowed/{family}/{pattern}"), |b| {
                let mut index = 0;
                b.iter(|| {
                    let ip = ips[index];
                    index += 1;
                    if index == ips.len() {
                        index = 0;
                    }
                    if let Ok(record) = city::lookup(&reader, black_box(ip)) {
                        black_box(record);
                    }
                });
            });
        }
    }
}

criterion_group! {
    name = borrowed;
    config = Criterion::default().warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_secs(2)).sample_size(50).nresamples(10_000);
    targets = benchmarks
}
criterion_main!(borrowed);
