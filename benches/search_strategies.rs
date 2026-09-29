use std::hint::black_box;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use libmaxminddb_rs::{
    IpNetwork, MetadataBuilder, MmdbDecode, MmdbEncode, MmdbRecord, Reader, Writer,
};

#[derive(Clone, Debug, PartialEq, MmdbRecord, MmdbEncode)]
struct InsertSearchRecord<'a> {
    #[mmdb(network)]
    network: IpNetwork,
    id: u32,
    country: &'a str,
    city: &'a str,
}

#[derive(Clone, Debug, PartialEq, MmdbDecode)]
struct DecodedSearchLocation<'a> {
    id: u32,
    country: &'a str,
    city: &'a str,
}

#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
struct OwnedLocation {
    id: u32,
    country: String,
    city: String,
}

fn build_search_database() -> Vec<u8> {
    let metadata = MetadataBuilder::new()
        .database_type("search-bench")
        .ip_version(6)
        .build()
        .unwrap();

    let mut writer = Writer::with_metadata(metadata);

    // 4096 IPv4 subnets
    for i in 0_u32..4096 {
        let a = (i >> 8) as u8;
        let b = i as u8;
        let network = IpNetwork::new(Ipv4Addr::new(10, a, b, 0).into(), 24).unwrap();
        let entry = InsertSearchRecord {
            network,
            id: i,
            country: "FR",
            city: "Paris",
        };
        writer.insert_entry(&entry).unwrap();
    }

    // 1024 IPv6 subnets
    for i in 0_u32..1024 {
        let seg = i as u16;
        let network =
            IpNetwork::new(Ipv6Addr::new(0x2001, 0x0db8, 0, seg, 0, 0, 0, 0).into(), 64).unwrap();
        let entry = InsertSearchRecord {
            network,
            id: 10_000 + i,
            country: "US",
            city: "New York",
        };
        writer.insert_entry(&entry).unwrap();
    }

    writer.finish().unwrap()
}

fn generate_hit_queries_v4(count: usize) -> Vec<IpAddr> {
    (0..count)
        .map(|i| {
            let shuffled = (i.wrapping_mul(2654435761) % 4096) as u32;
            let a = (shuffled >> 8) as u8;
            let b = shuffled as u8;
            IpAddr::V4(Ipv4Addr::new(10, a, b, 42))
        })
        .collect()
}

fn generate_miss_queries_v4(count: usize) -> Vec<IpAddr> {
    (0..count)
        .map(|i| {
            let shuffled = (i.wrapping_mul(2654435761) % 250 + 1) as u8;
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, shuffled))
        })
        .collect()
}

fn generate_hit_queries_v6(count: usize) -> Vec<IpAddr> {
    (0..count)
        .map(|i| {
            let seg = (i.wrapping_mul(2654435761) % 1024) as u16;
            IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, seg, 0, 0, 0, 1))
        })
        .collect()
}

fn generate_miss_queries_v6(count: usize) -> Vec<IpAddr> {
    (0..count)
        .map(|i| {
            let seg = (i.wrapping_mul(2654435761) % 65535 + 1) as u16;
            IpAddr::V6(Ipv6Addr::new(0x3001, seg, 0, 0, 0, 0, 0, 1))
        })
        .collect()
}

fn bench_search_strategies(c: &mut Criterion) {
    let db_bytes = build_search_database();
    let reader = Reader::from_bytes(&db_bytes).unwrap();

    let hit_v4 = generate_hit_queries_v4(4096);
    let miss_v4 = generate_miss_queries_v4(4096);
    let hit_v6 = generate_hit_queries_v6(1024);
    let miss_v6 = generate_miss_queries_v6(1024);

    let scenarios: [(&str, &[IpAddr], bool); 4] = [
        ("ipv4_hit", &hit_v4, true),
        ("ipv4_miss", &miss_v4, false),
        ("ipv6_hit", &hit_v6, true),
        ("ipv6_miss", &miss_v6, false),
    ];

    for (scenario_name, query_ips, is_hit) in scenarios {
        for &ip in query_ips {
            assert_eq!(reader.lookup_exists(ip), is_hit);
        }
        let mut group = c.benchmark_group(format!("search_strategies/{scenario_name}"));
        group.throughput(Throughput::Elements(1));
        let mask = query_ips.len() - 1;

        // Strategy 1: lookup_exists (pure trie traversal, stops at node, zero payload decoding)
        group.bench_function(BenchmarkId::new("lookup_exists", scenario_name), |b| {
            let mut i = 0_usize;
            b.iter(|| {
                let ip = query_ips[i & mask];
                i = i.wrapping_add(1);
                black_box(reader.lookup_exists(black_box(ip)))
            });
        });

        // If the scenario is an existing hit, test the payload retrieval & decoding strategies
        if is_hit {
            // Typed borrowed decoding avoids generic map containers.
            group.bench_function(BenchmarkId::new("lookup_borrowed", scenario_name), |b| {
                let mut i = 0_usize;
                b.iter(|| {
                    let ip = query_ips[i & mask];
                    i = i.wrapping_add(1);
                    let rec: DecodedSearchLocation<'_> =
                        reader.lookup_borrowed(black_box(ip)).unwrap();
                    black_box(rec)
                });
            });

            // Generic dynamic ValueRef tree.
            group.bench_function(BenchmarkId::new("lookup_value", scenario_name), |b| {
                let mut i = 0_usize;
                b.iter(|| {
                    let ip = query_ips[i & mask];
                    i = i.wrapping_add(1);
                    black_box(reader.lookup_value(black_box(ip)).unwrap())
                });
            });

            // Serde lookup deserializes into an owned struct.
            group.bench_function(BenchmarkId::new("lookup_serde_owned", scenario_name), |b| {
                let mut i = 0_usize;
                b.iter(|| {
                    let ip = query_ips[i & mask];
                    i = i.wrapping_add(1);
                    let rec: OwnedLocation = reader.lookup(black_box(ip)).unwrap();
                    black_box(rec)
                });
            });
        }

        group.finish();
    }

    // ------------------------------------------------------------------------
    // Batch processing comparison
    // ------------------------------------------------------------------------
    let batch_slice: Vec<IpAddr> = hit_v4.iter().copied().take(256).collect();
    let mut batch_group = c.benchmark_group("search_strategies/batch");
    batch_group.throughput(Throughput::Elements(batch_slice.len() as u64));

    batch_group.bench_function("lookup_many_batch_256", |b| {
        b.iter(|| black_box(reader.lookup_many(black_box(&batch_slice))))
    });

    batch_group.bench_function("sequential_lookup_value_256", |b| {
        b.iter(|| {
            for &ip in &batch_slice {
                black_box(reader.lookup_value(black_box(ip)).unwrap());
            }
        });
    });

    batch_group.finish();
}

fn bench_longest_prefix(c: &mut Criterion) {
    // Interior addresses exercise overlapping prefixes and fallback to their parent.
    // The writer materializes inherited values on sibling branches. Returned
    // prefixes describe those serialized leaves, not the original parent network.
    for (family, version, networks, queries) in [
        (
            "ipv4",
            4,
            ["10.0.0.0/8", "10.20.0.0/16", "10.20.30.0/24"],
            [
                ("10.128.30.42", 9),
                ("10.20.128.42", 17),
                ("10.20.30.42", 24),
            ],
        ),
        (
            "ipv6",
            6,
            [
                "2001:db8::/32",
                "2001:db8:abcd::/48",
                "2001:db8:abcd:1234::/64",
            ],
            [
                ("2001:db8:1::42", 33),
                ("2001:db8:abcd:8000::42", 49),
                ("2001:db8:abcd:1234::42", 64),
            ],
        ),
    ] {
        let mut writer = Writer::with_metadata(
            MetadataBuilder::new()
                .ip_version(version)
                .database_type("cidr-micro-benchmark")
                .build_epoch(1_700_000_000)
                .build()
                .unwrap(),
        );
        for (id, network) in networks.iter().enumerate() {
            writer
                .insert_entry(&InsertSearchRecord {
                    network: network.parse().unwrap(),
                    id: id as u32,
                    country: "FR",
                    city: "Paris",
                })
                .unwrap();
        }
        let reader = Reader::from_vec(writer.finish().unwrap()).unwrap();
        let ips: Vec<IpAddr> = queries
            .iter()
            .enumerate()
            .map(|(id, (ip, prefix))| {
                let ip = ip.parse().unwrap();
                let record: DecodedSearchLocation<'_> = reader.lookup_borrowed(ip).unwrap();
                assert_eq!(record.id, id as u32);
                assert_eq!(reader.lookup_value_with_prefix(ip).unwrap().1, *prefix);
                ip
            })
            .collect();
        let mut group = c.benchmark_group("search_strategies/cidr");
        group.throughput(Throughput::Elements(1));
        group.bench_function(format!("longest_prefix_{family}"), |b| {
            let mut index = 0;
            b.iter(|| {
                let ip = ips[index];
                index = (index + 1) % ips.len();
                black_box(reader.lookup_value_with_prefix(black_box(ip)).unwrap())
            });
        });
        group.finish();
    }
}

fn configured_criterion() -> Criterion {
    Criterion::default()
        .warm_up_time(std::time::Duration::from_millis(60))
        .measurement_time(std::time::Duration::from_millis(180))
        .sample_size(12)
}

criterion_group! {
    name = search_strategy_benches;
    config = configured_criterion();
    targets = bench_search_strategies, bench_longest_prefix
}
criterion_main!(search_strategy_benches);
