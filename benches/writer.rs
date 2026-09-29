use std::hint::black_box;
use std::net::{Ipv4Addr, Ipv6Addr};

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use libmaxminddb_rs::{
    IpNetwork, MergeStrategy, Metadata, MetadataBuilder, MmdbEncode, MmdbRecord, Value, Writer,
};

#[derive(Clone, Debug, PartialEq, MmdbRecord, MmdbEncode)]
struct BenchGeoRecord<'a> {
    #[mmdb(network)]
    network: IpNetwork,
    country: &'a str,
    city: &'a str,
    asn: u32,
}

#[derive(Clone, Debug, PartialEq, MmdbEncode)]
struct BenchPayload<'a> {
    country: &'a str,
    city: &'a str,
    asn: u32,
}

fn metadata_for_ip_version(ip_version: u16) -> Metadata {
    MetadataBuilder::new()
        .database_type("bench")
        .ip_version(ip_version)
        .build()
        .unwrap()
}

fn pregenerate_v4_entries(count: u32) -> Vec<(IpNetwork, serde_json::Value)> {
    (0..count)
        .map(|i| {
            let a = (i >> 8) as u8;
            let b = i as u8;
            let net = IpNetwork::new(Ipv4Addr::new(10, a, b, 0).into(), 24).unwrap();
            let val = serde_json::json!({"id": i, "category": "bench", "active": true});
            (net, val)
        })
        .collect()
}

fn pregenerate_v6_entries(count: u32) -> Vec<(IpNetwork, serde_json::Value)> {
    (0..count)
        .map(|i| {
            let seg = i as u16;
            let net = IpNetwork::new(Ipv6Addr::new(0x2001, 0x0db8, 0, seg, 0, 0, 0, 0).into(), 64)
                .unwrap();
            let val = serde_json::json!({"id": i, "region": "bench"});
            (net, val)
        })
        .collect()
}

fn pregenerate_derived_records(count: u32) -> Vec<BenchGeoRecord<'static>> {
    (0..count)
        .map(|i| {
            let a = (i >> 8) as u8;
            let b = i as u8;
            BenchGeoRecord {
                network: IpNetwork::new(Ipv4Addr::new(10, a, b, 0).into(), 24).unwrap(),
                country: "FR",
                city: "Paris",
                asn: 13335 + i,
            }
        })
        .collect()
}

fn pregenerate_encoded_payloads(count: u32) -> Vec<(IpNetwork, BenchPayload<'static>)> {
    (0..count)
        .map(|i| {
            let a = (i >> 8) as u8;
            let b = i as u8;
            let net = IpNetwork::new(Ipv4Addr::new(10, a, b, 0).into(), 24).unwrap();
            let payload = BenchPayload {
                country: "FR",
                city: "Lyon",
                asn: 20000 + i,
            };
            (net, payload)
        })
        .collect()
}

fn prebuilt_populated_writer(entries: &[(IpNetwork, serde_json::Value)]) -> Writer {
    let mut writer = Writer::with_metadata(metadata_for_ip_version(4));
    for (net, val) in entries {
        writer.insert(*net, val).unwrap();
    }
    writer
}

fn benches(c: &mut Criterion) {
    let v4_entries = pregenerate_v4_entries(1024);
    let v6_entries = pregenerate_v6_entries(1024);
    let derived_entries = pregenerate_derived_records(1024);
    let encoded_entries = pregenerate_encoded_payloads(1024);

    let merge_network: IpNetwork = "192.0.2.0/24".parse().unwrap();
    let merge_records: Vec<serde_json::Value> = (0..100)
        .map(|i| serde_json::json!({"values": [i], "seq": i}))
        .collect();
    let append_records: Vec<serde_json::Value> =
        (0..100).map(|i| serde_json::json!([i % 16])).collect();

    // ------------------------------------------------------------------------
    // 1. Radix trie node insertions (isolated from text/CIDR parsing)
    // ------------------------------------------------------------------------
    c.bench_function("writer/insert_1024_ipv4", |b| {
        b.iter(|| {
            let mut writer = Writer::with_metadata(metadata_for_ip_version(4));
            for (net, val) in &v4_entries {
                writer.insert(*net, val).unwrap();
            }
            black_box(writer)
        })
    });

    c.bench_function("writer/insert_1024_ipv6", |b| {
        b.iter(|| {
            let mut writer = Writer::with_metadata(metadata_for_ip_version(6));
            for (net, val) in &v6_entries {
                writer.insert(*net, val).unwrap();
            }
            black_box(writer)
        })
    });

    // ------------------------------------------------------------------------
    // 2. High-performance native Rust paths (derive macro & pre-encoded payloads)
    // ------------------------------------------------------------------------
    c.bench_function("writer/insert_derived_record_1024", |b| {
        b.iter(|| {
            let mut writer = Writer::with_metadata(metadata_for_ip_version(4));
            for entry in &derived_entries {
                writer.insert_entry(entry).unwrap();
            }
            black_box(writer)
        })
    });

    c.bench_function("writer/insert_encoded_payload_1024", |b| {
        b.iter(|| {
            let mut writer = Writer::with_metadata(metadata_for_ip_version(4));
            for (net, payload) in &encoded_entries {
                writer.insert_encoded(*net, payload).unwrap();
            }
            black_box(writer)
        })
    });

    // ------------------------------------------------------------------------
    // 3. Serialization and trie compaction (isolated from insertion)
    // ------------------------------------------------------------------------
    c.bench_function("writer/finish_serialization_1024", |b| {
        b.iter_batched(
            || prebuilt_populated_writer(&v4_entries),
            |writer| black_box(writer.finish().unwrap()),
            BatchSize::SmallInput,
        );
    });

    c.bench_function("writer/build_full_database_1024", |b| {
        b.iter(|| {
            let mut writer = Writer::with_metadata(metadata_for_ip_version(4));
            for (net, val) in &v4_entries {
                writer.insert(*net, val).unwrap();
            }
            black_box(writer.finish().unwrap())
        })
    });

    // ------------------------------------------------------------------------
    // 4. Overlapping CIDR merge strategies
    // ------------------------------------------------------------------------
    c.bench_function("writer/merge_deep_merge_100", |b| {
        b.iter(|| {
            let mut writer = Writer::with_metadata(metadata_for_ip_version(4))
                .merge_strategy(MergeStrategy::DeepMerge);
            for val in &merge_records {
                writer.insert(merge_network, val).unwrap();
            }
            black_box(writer.finish().unwrap())
        })
    });

    c.bench_function("writer/merge_append_unique_100", |b| {
        b.iter(|| {
            let mut writer = Writer::with_metadata(metadata_for_ip_version(4))
                .merge_strategy(MergeStrategy::AppendUnique);
            for val in &append_records {
                writer.insert(merge_network, val).unwrap();
            }
            black_box(writer.finish().unwrap())
        })
    });

    // ------------------------------------------------------------------------
    // 5. Data encoding primitives
    // ------------------------------------------------------------------------
    c.bench_function("writer/serialize_json_value", |b| {
        let record = serde_json::json!({
            "country": "FR",
            "category": "datacenter",
            "score": 99,
            "tags": ["cloud", "eu", "fast"]
        });
        b.iter(|| black_box(Value::from_serialize(black_box(&record)).unwrap()))
    });
}

fn configured_criterion() -> Criterion {
    Criterion::default()
        .warm_up_time(std::time::Duration::from_millis(100))
        .measurement_time(std::time::Duration::from_millis(250))
        .sample_size(15)
}

criterion_group! {
    name = writer_benches;
    config = configured_criterion();
    targets = benches
}
criterion_main!(writer_benches);
