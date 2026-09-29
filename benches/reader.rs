use std::hint::black_box;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use criterion::{Criterion, criterion_group, criterion_main};
use libmaxminddb_rs::{MetadataBuilder, MmdbDecode, Reader, Writer};
use tempfile::NamedTempFile;

#[allow(dead_code)]
#[derive(Debug, MmdbDecode)]
struct BenchRecord<'a> {
    id: u32,
    name: &'a str,
}

#[allow(dead_code)]
#[derive(Debug, MmdbDecode)]
struct CityNames<'a> {
    en: &'a str,
    fr: Option<&'a str>,
}

#[allow(dead_code)]
#[derive(Debug, MmdbDecode)]
struct CityCountry<'a> {
    geoname_id: u32,
    iso_code: &'a str,
    is_in_european_union: bool,
    names: CityNames<'a>,
}

#[allow(dead_code)]
#[derive(Debug, MmdbDecode)]
struct CityLocation {
    latitude: f64,
    longitude: f64,
    accuracy_radius: u16,
}

#[allow(dead_code)]
#[derive(Debug, MmdbDecode)]
struct CityRecord<'a> {
    country: CityCountry<'a>,
    location: CityLocation,
}

fn fixture() -> Vec<u8> {
    let metadata = MetadataBuilder::new()
        .database_type("bench")
        .ip_version(6)
        .build()
        .unwrap();
    let mut writer = Writer::with_metadata(metadata);
    for i in 0_u32..4096 {
        let a = (i >> 8) as u8;
        let b = i as u8;
        let network = format!("10.{a}.{b}.0/24").parse().unwrap();
        writer
            .insert(
                network,
                &serde_json::json!({"id": i, "name": "benchmark", "tags": ["a", "b"]}),
            )
            .unwrap();
    }
    for i in 0_u32..1024 {
        writer
            .insert(
                format!("2001:db8:0:{i:x}::/64").parse().unwrap(),
                &serde_json::json!({"id": 10000 + i, "name": "ipv6"}),
            )
            .unwrap();
    }
    writer.finish().unwrap()
}

fn fixture_file(bytes: &[u8]) -> NamedTempFile {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(bytes).unwrap();
    file.flush().unwrap();
    file
}

fn city_record_json() -> serde_json::Value {
    serde_json::json!({
        "country": {
            "geoname_id": 2635167,
            "is_in_european_union": true,
            "iso_code": "GB",
            "names": {"en": "United Kingdom", "fr": "Royaume-Uni"}
        },
        "location": {
            "accuracy_radius": 50,
            "latitude": 51.5074,
            "longitude": -0.1278
        }
    })
}

fn city_fixture() -> Vec<u8> {
    let metadata = MetadataBuilder::new()
        .database_type("city-bench")
        .ip_version(4)
        .build()
        .unwrap();
    let mut writer = Writer::with_metadata(metadata);
    writer
        .insert("10.11.0.0/24".parse().unwrap(), &city_record_json())
        .unwrap();
    writer.finish().unwrap()
}

fn lookup_ips_v4() -> Vec<IpAddr> {
    (0_u32..4096)
        .map(|i| {
            let shuffled = i.wrapping_mul(2654435761) & 4095;
            let a = (shuffled >> 8) as u8;
            let b = shuffled as u8;
            IpAddr::V4(Ipv4Addr::new(10, a, b, 42))
        })
        .collect()
}

fn lookup_ips_v6() -> Vec<IpAddr> {
    (0_u32..1024)
        .map(|i| {
            let shuffled = (i.wrapping_mul(2654435761) & 1023) as u16;
            IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, shuffled, 0, 0, 0, 1))
        })
        .collect()
}

fn benches(c: &mut Criterion) {
    let bytes = fixture();
    let file = fixture_file(&bytes);
    let reader = Reader::from_bytes(&bytes).unwrap();

    let ipv4_hit: IpAddr = "10.7.3.42".parse().unwrap();
    let ipv6_hit: IpAddr = "2001:db8:0:1a::1".parse().unwrap();
    let ipv4_miss: IpAddr = "192.0.2.1".parse().unwrap();
    let ipv6_miss: IpAddr = "3001::1".parse().unwrap();

    let ips_v4 = lookup_ips_v4();
    let ips_v6 = lookup_ips_v6();
    let batch_ips: Vec<IpAddr> = ips_v4.iter().copied().take(256).collect();

    // ------------------------------------------------------------------------
    // 1. Hot-path lookups (L1 cache hit on repeatedly queried address)
    // ------------------------------------------------------------------------
    c.bench_function("reader/lookup_borrowed_ipv4_hot", |b| {
        b.iter(|| {
            black_box(
                reader
                    .lookup_borrowed::<BenchRecord<'_>>(black_box(ipv4_hit))
                    .unwrap(),
            )
        })
    });
    c.bench_function("reader/lookup_borrowed_ipv6_hot", |b| {
        b.iter(|| {
            black_box(
                reader
                    .lookup_borrowed::<BenchRecord<'_>>(black_box(ipv6_hit))
                    .unwrap(),
            )
        })
    });

    // ------------------------------------------------------------------------
    // 2. Trie rejection (unmapped addresses exiting early without decoding)
    // ------------------------------------------------------------------------
    c.bench_function("reader/lookup_borrowed_ipv4_miss", |b| {
        b.iter(|| {
            black_box(
                reader
                    .lookup_borrowed::<BenchRecord<'_>>(black_box(ipv4_miss))
                    .is_err(),
            )
        })
    });
    c.bench_function("reader/lookup_borrowed_ipv6_miss", |b| {
        b.iter(|| {
            black_box(
                reader
                    .lookup_borrowed::<BenchRecord<'_>>(black_box(ipv6_miss))
                    .is_err(),
            )
        })
    });
    c.bench_function("reader/lookup_borrowed_map_ipv6_miss", |b| {
        b.iter(|| {
            black_box(
                reader
                    .lookup_borrowed_map(black_box(ipv6_miss), |record: BenchRecord<'_>| {
                        black_box(record.id)
                    })
                    .unwrap(),
            )
        })
    });
    c.bench_function("reader/lookup_borrowed_map_ipv6_hit", |b| {
        b.iter(|| {
            black_box(
                reader
                    .lookup_borrowed_map(black_box(ipv6_hit), |record: BenchRecord<'_>| {
                        black_box(record.id)
                    })
                    .unwrap(),
            )
        })
    });

    // ------------------------------------------------------------------------
    // 3. Random address lookups across realistic network distributions
    // ------------------------------------------------------------------------
    c.bench_function("reader/lookup_borrowed_ipv4_random", |b| {
        let mut index = 0_usize;
        let mask = ips_v4.len() - 1;
        b.iter(|| {
            let ip = ips_v4[index & mask];
            index = index.wrapping_add(1);
            black_box(
                reader
                    .lookup_borrowed::<BenchRecord<'_>>(black_box(ip))
                    .unwrap(),
            )
        })
    });
    c.bench_function("reader/lookup_borrowed_ipv6_random", |b| {
        let mut index = 0_usize;
        let mask = ips_v6.len() - 1;
        b.iter(|| {
            let ip = ips_v6[index & mask];
            index = index.wrapping_add(1);
            black_box(
                reader
                    .lookup_borrowed::<BenchRecord<'_>>(black_box(ip))
                    .unwrap(),
            )
        })
    });

    // ------------------------------------------------------------------------
    // 4. Critical performance paths: existence and typed borrowed decoding
    // ------------------------------------------------------------------------
    c.bench_function("reader/lookup_exists_ipv4", |b| {
        b.iter(|| black_box(reader.lookup_exists(black_box(ipv4_hit))))
    });
    c.bench_function("reader/decode_borrowed_struct", |b| {
        b.iter(|| {
            let value: BenchRecord<'_> = reader.lookup_borrowed(black_box(ipv4_hit)).unwrap();
            black_box((value.id, value.name))
        })
    });

    // ------------------------------------------------------------------------
    // 5. Real-world City schema: zero-copy borrowed vs generic AST ValueRef
    // ------------------------------------------------------------------------
    let city_bytes = city_fixture();
    let city_reader = Reader::from_bytes(&city_bytes).unwrap();
    let city_ip: IpAddr = "10.11.0.42".parse().unwrap();

    c.bench_function("reader/city_lookup_value", |b| {
        b.iter(|| black_box(city_reader.lookup_value(black_box(city_ip)).unwrap()))
    });
    c.bench_function("reader/city_lookup_borrowed", |b| {
        b.iter(|| {
            let record: CityRecord<'_> = city_reader.lookup_borrowed(black_box(city_ip)).unwrap();
            black_box(record)
        })
    });

    // ------------------------------------------------------------------------
    // 6. Batch processing of pre-parsed IP addresses
    // ------------------------------------------------------------------------
    c.bench_function("reader/lookup_many_256", |b| {
        b.iter(|| black_box(reader.lookup_many(black_box(&batch_ips))))
    });

    // ------------------------------------------------------------------------
    // 7. Database opening modes (warm parsing of headers & accelerator tables)
    // ------------------------------------------------------------------------
    c.bench_function("reader/open_from_bytes", |b| {
        b.iter(|| black_box(Reader::from_bytes(black_box(&bytes)).unwrap()))
    });
    c.bench_function("reader/open_owned_file", |b| {
        b.iter(|| black_box(Reader::open(black_box(file.path())).unwrap()))
    });
    c.bench_function("reader/open_mmap", |b| {
        b.iter(|| {
            // SAFETY: the NamedTempFile stays alive and unmodified during the benchmark.
            black_box(unsafe { Reader::open_mmap(black_box(file.path())).unwrap() })
        })
    });
}

fn configured_criterion() -> Criterion {
    Criterion::default()
        .warm_up_time(std::time::Duration::from_millis(100))
        .measurement_time(std::time::Duration::from_millis(250))
        .sample_size(15)
}

criterion_group! {
    name = reader_benches;
    config = configured_criterion();
    targets = benches
}
criterion_main!(reader_benches);
