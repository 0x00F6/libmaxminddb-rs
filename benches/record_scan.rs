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

fn fixture(version: u16, routes: u32) -> Vec<u8> {
    let metadata = MetadataBuilder::new()
        .ip_version(version)
        .database_type("record-scan-v1")
        .build_epoch(1_700_000_000)
        .build()
        .unwrap();
    let mut writer = Writer::with_metadata(metadata);
    for i in 0..routes {
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
        let bytes = fixture(version, ROUTES);
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

// Publish each worker's count/checksum once, on thread exit. Unlike one
// shared atomic per record, this measures traversal/decode scaling without
// turning the callback into a cache-line contention benchmark.
#[derive(Default)]
struct Totals {
    count: std::sync::atomic::AtomicU64,
    checksum: std::sync::atomic::AtomicU64,
}
struct LocalTotals {
    totals: std::sync::Arc<Totals>,
    count: u64,
    checksum: u64,
}
impl Drop for LocalTotals {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        self.totals.count.fetch_add(self.count, Relaxed);
        self.totals.checksum.fetch_add(self.checksum, Relaxed);
    }
}
thread_local! {
    static LOCAL_TOTALS: std::cell::RefCell<Option<LocalTotals>> = const { std::cell::RefCell::new(None) };
}

fn parallel(reader: &Reader<'_>, workers: Option<usize>) -> (u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    let totals = std::sync::Arc::new(Totals::default());
    let visitor = |network: IpNetwork, record: Record<'_>| {
        LOCAL_TOTALS.with_borrow_mut(|slot| {
            let local = slot.get_or_insert_with(|| LocalTotals {
                totals: std::sync::Arc::clone(&totals),
                count: 0,
                checksum: 0,
            });
            local.count += 1;
            local.checksum += record.file.len() as u64
                + record.category.len() as u64
                + u64::from(record.score)
                + u64::from(network.prefix_len());
        });
        Ok(())
    };
    if let Some(workers) = workers {
        reader
            .visit_borrowed_records_parallel_with_workers(
                std::num::NonZeroUsize::new(workers).unwrap(),
                visitor,
            )
            .unwrap();
    } else {
        reader.visit_borrowed_records_parallel(visitor).unwrap();
    }
    // The 1-worker/adaptive fallback ran on the caller, whose TLS outlives this scan.
    LOCAL_TOTALS.with_borrow_mut(|slot| {
        slot.take();
    });
    (totals.count.load(Relaxed), totals.checksum.load(Relaxed))
}

fn bench_parallel_scans(c: &mut Criterion) {
    bench_parallel_sizes(
        c,
        "parallel_record_scan_v1",
        &[1_000, 100_000, 1_000_000],
        &[1, 2, 4, 8, 16],
        false,
    );
}

fn bench_parallel_crossover(c: &mut Criterion) {
    bench_parallel_sizes(
        c,
        "parallel_record_scan_crossover_v1",
        &[
            1_000, 2_000, 4_000, 8_000, 12_000, 16_000, 24_000, 32_000, 64_000,
        ],
        &[1, 2, 4, 8],
        true,
    );
}

fn bench_parallel_sizes(
    c: &mut Criterion,
    name: &str,
    sizes: &[u32],
    worker_counts: &[usize],
    crossover: bool,
) {
    let output = std::path::PathBuf::from(
        std::env::var_os("CARGO_TARGET_DIR").unwrap_or_else(|| "target".into()),
    )
    .join("record-scan");
    std::fs::create_dir_all(&output).unwrap();
    for &routes in sizes {
        let mut group = c.benchmark_group(format!("{name}/{routes}"));
        if crossover {
            group
                .sample_size(30)
                .warm_up_time(Duration::from_millis(500))
                .measurement_time(Duration::from_secs(2));
        }
        group.throughput(Throughput::Elements(u64::from(routes)));
        for version in [4, 6] {
            let path = output.join(format!("ipv{version}-{routes}.mmdb"));
            let bytes = if path.exists() {
                std::fs::read(&path).unwrap()
            } else {
                let bytes = fixture(version, routes);
                std::fs::write(&path, &bytes).unwrap();
                bytes
            };
            let reader = Reader::from_bytes(&bytes).unwrap();
            let expected = typed(&reader);
            assert_eq!(expected.0, u64::from(routes));
            assert_eq!(generic(&reader), expected);
            println!(
                "parallel_scan_fixture family=ipv{version} routes={routes} nodes={} bytes={} count={} checksum={}",
                reader.metadata().node_count,
                bytes.len(),
                expected.0,
                expected.1
            );
            group.bench_with_input(
                BenchmarkId::new("sequential", format!("ipv{version}")),
                &reader,
                |b, reader| b.iter(|| black_box(typed(black_box(reader)))),
            );
            for &workers in worker_counts {
                assert_eq!(parallel(&reader, Some(workers)), expected);
                group.bench_with_input(
                    BenchmarkId::new(format!("workers-{workers}"), format!("ipv{version}")),
                    &reader,
                    |b, reader| b.iter(|| black_box(parallel(black_box(reader), Some(workers)))),
                );
            }
            assert_eq!(parallel(&reader, None), expected);
            group.bench_with_input(
                BenchmarkId::new("adaptive", format!("ipv{version}")),
                &reader,
                |b, reader| b.iter(|| black_box(parallel(black_box(reader), None))),
            );
        }
        group.finish();
    }
}

criterion_group! {
    name = scans;
    config = Criterion::default().sample_size(20).warm_up_time(Duration::from_secs(1)).measurement_time(Duration::from_secs(3));
    targets = bench_scans, bench_parallel_scans, bench_parallel_crossover
}
criterion_main!(scans);
