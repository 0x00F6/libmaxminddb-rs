#![cfg(all(feature = "reader", feature = "writer", feature = "derive"))]

use libmaxminddb_rs::{Error, MetadataBuilder, MmdbDecode, Reader, Value, Writer};
use std::collections::{BTreeMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Barrier, Mutex};

#[derive(MmdbDecode)]
struct Record<'a> {
    file: &'a str,
    bytes: &'a [u8],
}

fn payload(file: &str, bytes: &[u8]) -> Value {
    Value::Map(BTreeMap::from([
        ("file".into(), Value::Utf8(file.into())),
        ("bytes".into(), Value::Bytes(bytes.into())),
    ]))
}

fn fixture(version: u16) -> Vec<u8> {
    let mut writer =
        Writer::with_metadata(MetadataBuilder::new().ip_version(version).build().unwrap());
    for i in 0..512_u32 {
        let network = if version == 4 {
            format!("{}/16", std::net::Ipv4Addr::from(i << 16))
        } else {
            format!("2001:db8:{i:x}::/48")
        };
        // Keep payload sharing while varying network prefixes.
        writer
            .insert_value(network.parse().unwrap(), payload("shared", &[0, 1, 255]))
            .unwrap();
    }
    if version == 6 {
        writer
            .insert_value("192.0.2.0/31".parse().unwrap(), payload("v4", &[42]))
            .unwrap();
        writer
            .insert_value(
                "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff/128"
                    .parse()
                    .unwrap(),
                payload("last", &[42]),
            )
            .unwrap();
    }
    writer.finish().unwrap()
}

#[test]
fn parallel_matches_sequential_and_point_lookups_for_all_backends() {
    for version in [4, 6] {
        let bytes = fixture(version);
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), &bytes).unwrap();
        // SAFETY: this test owns the file and keeps it alive and unchanged until readers are dropped.
        let mmap = unsafe { Reader::open_mmap(file.path()) }.unwrap();
        for reader in [
            Reader::from_bytes(&bytes).unwrap(),
            Reader::from_vec(bytes.clone()).unwrap(),
            Reader::open(file.path()).unwrap(),
            mmap,
        ] {
            let mut expected = BTreeMap::new();
            reader
                .visit_borrowed_records(|network, record: Record<'_>| {
                    assert!(
                        expected
                            .insert(network, (record.file, record.bytes))
                            .is_none()
                    );
                    Ok(())
                })
                .unwrap();
            for workers in [1, 2, 4, 16] {
                let actual = Mutex::new(BTreeMap::new());
                reader
                    .visit_borrowed_records_parallel_with_workers(
                        NonZeroUsize::new(workers).unwrap(),
                        |network, record: Record<'_>| {
                            for borrowed in [record.file.as_bytes(), record.bytes] {
                                let source = reader.as_bytes();
                                let offset = borrowed.as_ptr() as usize - source.as_ptr() as usize;
                                assert_eq!(&source[offset..offset + borrowed.len()], borrowed);
                            }
                            let value = reader.lookup_value(network.broadcast())?;
                            assert_eq!(
                                value.get("file"),
                                Some(&libmaxminddb_rs::ValueRef::Utf8(record.file))
                            );
                            assert!(
                                actual
                                    .lock()
                                    .unwrap()
                                    .insert(network, (record.file, record.bytes))
                                    .is_none()
                            );
                            Ok(())
                        },
                    )
                    .unwrap();
                assert_eq!(actual.into_inner().unwrap(), expected);
            }
        }
    }
}

#[test]
fn adaptive_fallback_and_parallel_scans_preserve_all_records() {
    let caller = std::thread::current().id();
    let available = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    for version in [4, 6] {
        for routes in [16_000_u32, 24_000] {
            let mut writer =
                Writer::with_metadata(MetadataBuilder::new().ip_version(version).build().unwrap());
            for i in 0..routes {
                let network = if version == 4 {
                    format!("{}/24", std::net::Ipv4Addr::from(0x0a00_0000 + (i << 8)))
                } else {
                    format!("2001:db8:0:{i:x}::/64")
                };
                writer
                    .insert_value(
                        network.parse().unwrap(),
                        payload(&format!("route-{i}"), &[0, 1, 255]),
                    )
                    .unwrap();
            }
            let bytes = writer.finish().unwrap();
            let reader = Reader::from_bytes(&bytes).unwrap();
            let should_spawn = available > 1 && routes == 24_000;
            assert_eq!(reader.metadata().node_count >= 24_000, routes == 24_000);
            let count = AtomicUsize::new(0);
            let off_caller = AtomicUsize::new(0);
            reader
                .visit_borrowed_records_parallel(|_, record: Record<'_>| {
                    assert!(record.file.starts_with("route-"));
                    assert_eq!(record.bytes, &[0, 1, 255]);
                    count.fetch_add(1, Ordering::Relaxed);
                    if std::thread::current().id() != caller {
                        off_caller.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(())
                })
                .unwrap();
            assert_eq!(count.into_inner(), routes as usize);
            assert_eq!(
                off_caller.into_inner(),
                if should_spawn { routes as usize } else { 0 }
            );
        }
    }
}

#[test]
fn callbacks_run_on_multiple_workers_and_all_finish_before_return() {
    let bytes = fixture(4);
    let reader = Reader::from_bytes(&bytes).unwrap();
    let threads = Mutex::new(HashSet::new());
    let first = AtomicUsize::new(0);
    let active = AtomicUsize::new(0);
    let barrier = Barrier::new(2);
    reader
        .visit_borrowed_records_parallel_with_workers(
            NonZeroUsize::new(2).unwrap(),
            |_, _: Record<'_>| {
                active.fetch_add(1, Ordering::SeqCst);
                threads.lock().unwrap().insert(std::thread::current().id());
                if first.fetch_add(1, Ordering::SeqCst) < 2 {
                    barrier.wait();
                    assert!(active.load(Ordering::SeqCst) >= 1);
                }
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(active.into_inner(), 0);
    assert_eq!(threads.into_inner().unwrap().len(), 2);
}

#[test]
fn errors_cancel_and_workers_are_joined_and_panics_propagate() {
    let bytes = fixture(4);
    let reader = Reader::from_bytes(&bytes).unwrap();
    let calls = AtomicUsize::new(0);
    let result = reader.visit_borrowed_records_parallel_with_workers(
        NonZeroUsize::new(4).unwrap(),
        |_, _: Record<'_>| {
            calls.fetch_add(1, Ordering::Relaxed);
            Err(Error::InvalidDatabase("callback failed"))
        },
    );
    assert!(matches!(
        result,
        Err(Error::InvalidDatabase("callback failed"))
    ));
    assert!((1..=4).contains(&calls.load(Ordering::Relaxed)));
    let completed = calls.load(Ordering::Relaxed);
    std::thread::yield_now();
    assert_eq!(calls.load(Ordering::Relaxed), completed);
    let result = std::panic::catch_unwind(|| {
        reader.visit_borrowed_records_parallel_with_workers(
            NonZeroUsize::new(4).unwrap(),
            |_, _: Record<'_>| -> libmaxminddb_rs::Result<()> { panic!("callback panic") },
        )
    });
    assert!(result.is_err());
}

#[test]
fn decode_errors_empty_databases_and_zero_prefix_are_preserved() {
    for version in [4, 6] {
        let metadata = MetadataBuilder::new().ip_version(version).build().unwrap();
        let bytes = Writer::with_metadata(metadata.clone()).finish().unwrap();
        let reader = Reader::from_bytes(&bytes).unwrap();
        reader
            .visit_borrowed_records_parallel_with_workers(
                NonZeroUsize::new(4).unwrap(),
                |_, _: Record<'_>| panic!("empty database"),
            )
            .unwrap();
        let mut writer = Writer::with_metadata(metadata.clone());
        writer
            .insert_value("0.0.0.0/0".parse().unwrap(), payload("all", &[1]))
            .unwrap();
        let bytes = writer.finish().unwrap();
        let reader = Reader::from_bytes(&bytes).unwrap();
        let coverage = std::sync::atomic::AtomicU64::new(0);
        reader
            .visit_borrowed_records_parallel_with_workers(
                NonZeroUsize::new(4).unwrap(),
                |network, record: Record<'_>| {
                    assert_eq!(record.file, "all");
                    coverage.fetch_add(1_u64 << (32 - network.prefix_len()), Ordering::Relaxed);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(coverage.into_inner(), 1_u64 << 32);
        let mut writer = Writer::with_metadata(metadata);
        writer
            .insert(
                "192.0.2.0/24".parse().unwrap(),
                &serde_json::json!({"file": 7, "bytes": []}),
            )
            .unwrap();
        let bytes = writer.finish().unwrap();
        let reader = Reader::from_bytes(&bytes).unwrap();
        assert!(matches!(
            reader.visit_borrowed_records_parallel_with_workers(
                NonZeroUsize::new(4).unwrap(),
                |_, _: Record<'_>| Ok(())
            ),
            Err(Error::DecodingError(_))
        ));
    }
}
