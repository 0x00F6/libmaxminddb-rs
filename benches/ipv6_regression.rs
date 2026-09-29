//! Identical replay benchmark for historical readers and candidate fixes.
//! Set MMDB_REGRESSION_DIR to one shared, immutable fixture directory.
use std::{hint::black_box, net::IpAddr, path::PathBuf, time::Duration};

use criterion::{Criterion, criterion_group, criterion_main};
use libmaxminddb_rs::{MetadataBuilder, Reader, Writer};

fn fixtures() -> (Vec<u8>, Vec<IpAddr>, Vec<IpAddr>) {
    let dir =
        PathBuf::from(std::env::var("MMDB_REGRESSION_DIR").expect("shared fixture directory"));
    std::fs::create_dir_all(&dir).unwrap();
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
    if !path.exists() {
        let metadata = MetadataBuilder::new()
            .ip_version(6)
            .build_epoch(1_700_000_000)
            .build()
            .unwrap();
        let mut writer = Writer::with_metadata(metadata);
        for (ips, prefix) in [(&v4, 24), (&v6, 128)] {
            for (i, &ip) in ips.iter().enumerate() {
                writer
                    .insert(
                        libmaxminddb_rs::IpNetwork::new(ip, prefix).unwrap(),
                        &serde_json::json!({"id": i as u32, "name": "benchmark"}),
                    )
                    .unwrap();
            }
        }
        std::fs::write(&path, writer.finish().unwrap()).unwrap();
    }
    (std::fs::read(path).unwrap(), v4, v6)
}

fn benchmarks(c: &mut Criterion) {
    let (bytes, v4, v6) = fixtures();
    let reader = Reader::from_bytes(&bytes).unwrap();
    // Verify every query and warm the lookup path; index preparation ran at open.
    for ip in v4.iter().chain(&v6) {
        assert!(reader.lookup_value(*ip).is_ok());
    }
    for (family, ips) in [("ipv4", &v4), ("ipv6", &v6)] {
        for random in [false, true] {
            let pattern = if random { "random" } else { "hot" };
            c.bench_function(&format!("replay/tree_{family}_{pattern}"), |b| {
                let mut index = 0;
                b.iter(|| {
                    let ip = ips[index & (ips.len() - 1)];
                    index += usize::from(random);
                    black_box(reader.lookup_exists(black_box(ip)))
                })
            });
            c.bench_function(&format!("replay/value_{family}_{pattern}"), |b| {
                let mut index = 0;
                b.iter(|| {
                    let ip = ips[index & (ips.len() - 1)];
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
        assert!(!reader.lookup_exists(miss));
        c.bench_function(&format!("replay/tree_{family}_miss"), |b| {
            b.iter(|| black_box(!reader.lookup_exists(black_box(miss))))
        });
    }
    c.bench_function("replay/open", |b| {
        b.iter(|| black_box(Reader::from_bytes(black_box(&bytes)).unwrap()))
    });
    c.bench_function("replay/open_first_ipv6", |b| {
        b.iter(|| {
            let r = Reader::from_bytes(black_box(&bytes)).unwrap();
            black_box(r.lookup_exists(black_box(v6[0])))
        })
    });
}

criterion_group! {
    name = replay;
    config = Criterion::default().warm_up_time(Duration::from_millis(150))
        .measurement_time(Duration::from_millis(400)).sample_size(30);
    targets = benchmarks
}
criterion_main!(replay);
