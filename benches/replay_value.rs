//! Identical value-decode replay benchmark for historical readers and candidate fixes.
//! Uses the public `lookup_value` API (present in every version).
//! Set MMDB_REGRESSION_DIR to one shared, immutable fixture directory.
use std::{hint::black_box, net::IpAddr, path::PathBuf, time::Duration};

use criterion::{Criterion, criterion_group, criterion_main};
use libmaxminddb_rs::Reader;

fn read_fixture() -> (Vec<u8>, Vec<IpAddr>, Vec<IpAddr>) {
    let dir =
        PathBuf::from(std::env::var("MMDB_REGRESSION_DIR").expect("shared fixture directory"));
    let path = dir.join("mixed.mmdb");
    let v4: Vec<IpAddr> = (0..4096_u32)
        .map(|i| {
            let n = i.wrapping_mul(2654435761) & 4095;
            format!("10.{}.{}.42", n >> 8, n & 255).parse().unwrap()
        })
        .collect();
    let v6: Vec<IpAddr> = (0..4096_u32)
        .map(|i| {
            let n = i.wrapping_mul(2654435761) & 4095;
            format!("2001:db8:0:{n:x}::1").parse().unwrap()
        })
        .collect();
    (std::fs::read(path).unwrap(), v4, v6)
}

fn benchmarks(c: &mut Criterion) {
    let (bytes, v4, v6) = read_fixture();
    let reader = Reader::from_bytes(&bytes).unwrap();
    for ip in v4.iter().chain(&v6) {
        assert!(reader.lookup_value(*ip).is_ok());
    }
    for (family, ips) in [("ipv4", &v4), ("ipv6", &v6)] {
        for random in [false, true] {
            let pattern = if random { "random" } else { "hot" };
            c.bench_function(&format!("replay_value/lookup_{family}_{pattern}"), |b| {
                let mut index = 0;
                b.iter(|| {
                    let ip = ips[index & 4095];
                    index += usize::from(random);
                    black_box(reader.lookup_value(black_box(ip)).unwrap())
                })
            });
        }
        let miss: IpAddr = if family == "ipv4" {
            "192.0.2.1"
        } else {
            "3001::1"
        }
        .parse()
        .unwrap();
        assert!(reader.lookup_value(miss).is_err());
        c.bench_function(&format!("replay_value/lookup_{family}_miss"), |b| {
            b.iter(|| black_box(reader.lookup_value(black_box(miss)).is_err()))
        });
    }
    c.bench_function("replay_value/open", |b| {
        b.iter(|| black_box(Reader::from_bytes(black_box(&bytes)).unwrap()))
    });
}

criterion_group! {
    name = replay;
    config = Criterion::default().warm_up_time(Duration::from_millis(120))
        .measurement_time(Duration::from_millis(300)).sample_size(30);
    targets = benchmarks
}
criterion_main!(replay);
