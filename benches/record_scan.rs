use std::collections::BTreeMap;
use std::hint::black_box;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use libmaxminddb_rs::{IpNetwork, MetadataBuilder, MmdbDecode, Reader, Value, ValueRef, Writer};

#[path = "support/allocations.rs"]
mod allocations;

const ROUTES: u32 = 100_000;

#[derive(MmdbDecode)]
struct Record<'a> {
    file: &'a str,
    category: &'a str,
    score: u32,
}

fn fixture(version: u16) -> Vec<u8> {
    let metadata = MetadataBuilder::new()
        .ip_version(version)
        .database_type("record-scan-v1")
        .build_epoch(1_700_000_000)
        .build()
        .unwrap();
    let mut writer = Writer::with_metadata(metadata);
    for i in 0..ROUTES {
        let (address, prefix) = if version == 4 {
            (IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + (i << 8))), 24)
        } else {
            (
                IpAddr::V6(Ipv6Addr::from(
                    0x2001_0db8_0000_0000_0000_0000_0000_0000 + (u128::from(i) << 64),
                )),
                64,
            )
        };
        writer
            .insert_value(
                IpNetwork::new(address, prefix).unwrap(),
                Value::Map(BTreeMap::from([
                    ("file".into(), Value::Utf8(format!("list-{i:06}.ipset"))),
                    (
                        "category".into(),
                        Value::Utf8(["malware", "abuse", "spam", "custom"][i as usize % 4].into()),
                    ),
                    ("score".into(), Value::Uint32(i)),
                ])),
            )
            .unwrap();
    }
    writer.finish().unwrap()
}

fn generic(reader: &Reader<'_>) -> (u64, u64) {
    let mut count = 0;
    let mut checksum = 0;
    reader
        .visit_records(|network, value| {
            let Some(ValueRef::Utf8(file)) = value.get("file") else {
                panic!("missing file")
            };
            let Some(ValueRef::Utf8(category)) = value.get("category") else {
                panic!("missing category")
            };
            let Some(ValueRef::Uint32(score)) = value.get("score") else {
                panic!("missing score")
            };
            checksum += file.len() as u64
                + category.len() as u64
                + u64::from(*score)
                + u64::from(network.prefix_len());
            count += 1;
            Ok(())
        })
        .unwrap();
    (count, checksum)
}

fn typed(reader: &Reader<'_>) -> (u64, u64) {
    let mut count = 0;
    let mut checksum = 0;
    reader
        .visit_borrowed_records(|network, record: Record<'_>| {
            checksum += record.file.len() as u64
                + record.category.len() as u64
                + u64::from(record.score)
                + u64::from(network.prefix_len());
            count += 1;
            Ok(())
        })
        .unwrap();
    (count, checksum)
}

fn bench_scans(c: &mut Criterion) {
    let mut group = c.benchmark_group("record_scan_v1");
    group.throughput(Throughput::Elements(u64::from(ROUTES)));
    for version in [4, 6] {
        // Construction, opening and correctness verification are outside timing.
        let bytes = fixture(version);
        let output = std::path::PathBuf::from(
            std::env::var_os("CARGO_TARGET_DIR").unwrap_or_else(|| "target".into()),
        )
        .join("record-scan");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(output.join(format!("ipv{version}.mmdb")), &bytes).unwrap();
        let reader = Reader::from_bytes(&bytes).unwrap();
        let expected = generic(&reader);
        assert_eq!(expected.0, u64::from(ROUTES));
        assert_eq!(typed(&reader), expected);
        for (name, scan) in [
            ("generic", generic as fn(&Reader<'_>) -> _),
            ("typed", typed),
        ] {
            let (result, stats) = allocations::measure(|| scan(&reader));
            assert_eq!(result, expected);
            println!(
                "scan_allocation_sample family=ipv{version} method={name} routes={ROUTES} database_bytes={} allocations={} requested_bytes={}",
                bytes.len(),
                stats.allocations,
                stats.requested_bytes
            );
            group.bench_with_input(
                BenchmarkId::new(name, format!("ipv{version}")),
                &reader,
                |b, reader| b.iter(|| black_box(scan(black_box(reader)))),
            );
        }
    }
    group.finish();
}

criterion_group! {
    name = scans;
    config = Criterion::default().sample_size(20).warm_up_time(Duration::from_secs(1)).measurement_time(Duration::from_secs(3));
    targets = bench_scans
}
criterion_main!(scans);
