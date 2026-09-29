#![cfg(all(feature = "reader", feature = "writer", feature = "derive"))]

#[allow(dead_code)]
#[path = "../benches/support/memory.rs"]
mod memory;
#[allow(dead_code)]
#[path = "../benches/support/million.rs"]
mod million;

use libmaxminddb_rs::{Error, MetadataBuilder, Reader, Writer};
use million::{Family, Fixture, LABEL, Record, SEED, generate_misses, shuffle, validate_queries};

#[test]
fn fixtures_are_deterministic_and_preserve_every_distinct_cidr() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    for family in Family::ALL {
        let a = Fixture::new(first.path(), family, 257);
        let b = Fixture::new(second.path(), family, 257);
        a.prepare().unwrap();
        b.prepare().unwrap();
        assert!(a.cached());
        for name in ["database.mmdb", "hits.bin", "misses.bin", "manifest.json"] {
            assert_eq!(
                std::fs::read(a.dir.join(name)).unwrap(),
                std::fs::read(b.dir.join(name)).unwrap()
            );
        }
        let reader = Reader::open(a.database()).unwrap();
        assert_eq!(reader.metadata().ip_version, family.version());
        let hits = a.queries("hits").unwrap();
        let misses = a.queries("misses").unwrap();
        validate_queries(&reader, family, &hits, &misses).unwrap();
        // Hosts are interior addresses. The network base must decode to the same ID.
        for (id, ip) in hits.iter().enumerate() {
            let network = libmaxminddb_rs::IpNetwork::new(*ip, family.prefix()).unwrap();
            let base = network.network();
            assert_ne!(*ip, base);
            assert_eq!(
                reader.lookup_borrowed::<Record<'_>>(base).unwrap().id,
                id as u32
            );
        }
        assert_eq!(a.manifest().unwrap().entries, hits.len());
        a.prepare().unwrap(); // Reusing a complete fixture does not rewrite it.
    }
}

#[test]
fn random_absences_exclude_whole_cidrs_not_just_inserted_addresses() {
    for (family, cidr) in [(Family::V4, "0.0.0.0/1"), (Family::V6, "::/1")] {
        let mut writer = Writer::with_metadata(
            MetadataBuilder::new()
                .ip_version(family.version())
                .build()
                .unwrap(),
        );
        writer
            .insert_encoded(
                cidr.parse().unwrap(),
                &Record {
                    id: 0,
                    label: LABEL,
                },
            )
            .unwrap();
        let reader = Reader::from_vec(writer.finish().unwrap()).unwrap();
        let misses = generate_misses(&reader, family, 1000).unwrap();
        assert_eq!(misses, generate_misses(&reader, family, 1000).unwrap());
        for ip in misses {
            match ip {
                std::net::IpAddr::V4(ip) => assert!(ip.octets()[0] >= 128),
                std::net::IpAddr::V6(ip) => assert!(ip.octets()[0] >= 128),
            }
            assert!(matches!(
                reader.lookup_borrowed::<Record<'_>>(ip),
                Err(Error::NotFound)
            ));
        }
    }
}

#[test]
fn verification_rejects_hits_disguised_as_misses_and_duplicate_queries() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(dir.path(), Family::V4, 64);
    fixture.prepare().unwrap();
    let reader = Reader::open(fixture.database()).unwrap();
    let hits = fixture.queries("hits").unwrap();
    let mut misses = fixture.queries("misses").unwrap();
    misses[0] = hits[0];
    assert!(validate_queries(&reader, Family::V4, &hits, &misses).is_err());
    misses = fixture.queries("misses").unwrap();
    misses[1] = misses[0];
    assert!(validate_queries(&reader, Family::V4, &hits, &misses).is_err());
    std::fs::write(fixture.dir.join("misses.bin"), [0; 3]).unwrap();
    assert!(!fixture.cached());
    assert!(fixture.queries("misses").is_err());
}

#[test]
fn shuffled_hits_keep_the_exact_same_workload() {
    let mut a: Vec<_> = (0..1000).collect();
    let mut b = a.clone();
    shuffle(&mut a, SEED);
    shuffle(&mut b, SEED);
    assert_eq!(a, b);
    assert!(a.windows(2).any(|pair| pair[0] > pair[1]));
    a.sort_unstable();
    assert_eq!(a, (0..1000).collect::<Vec<_>>());
}

#[test]
fn rss_units_are_kibibytes_and_missing_samples_are_errors() {
    let sample =
        "Name:\ttest\nVmRSS:\t2048 kB\nVmHWM:\t4096 kB\nRssAnon:\t1536 kB\nRssFile:\t512 kB\n";
    let rss = memory::Snapshot::parse(sample).unwrap();
    assert_eq!(rss.rss_bytes, 2 * 1024 * 1024);
    assert_eq!(rss.peak_rss_bytes, 4 * 1024 * 1024);
    assert_eq!(rss.anonymous_rss_bytes + rss.file_rss_bytes, rss.rss_bytes);
    assert!(memory::Snapshot::parse("").is_err());
    assert!(memory::Snapshot::parse(&sample.replace("kB", "bytes")).is_err());
}

#[test]
fn memory_worker_requires_a_prepared_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(dir.path(), Family::V6, 16);
    assert!(memory::measure(&fixture, "owned").is_err());
    assert!(!fixture.dir.exists());
}
