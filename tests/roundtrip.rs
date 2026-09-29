#![cfg(all(feature = "reader", feature = "writer"))]

use std::collections::BTreeMap;
use std::net::IpAddr;

use libmaxminddb_rs::{MetadataBuilder, MmdbDecode, Reader, Value, ValueRef, Writer};

#[derive(Debug, MmdbDecode)]
struct Borrowed<'a> {
    country: &'a str,
    city: Option<&'a str>,
    score: u32,
}

fn metadata(ip_version: u16) -> libmaxminddb_rs::Metadata {
    MetadataBuilder::new()
        .database_type("roundtrip-test")
        .ip_version(ip_version)
        .languages(["en".to_string()])
        .description("en", "roundtrip")
        .build()
        .unwrap()
}

#[cfg(feature = "derive")]
#[test]
fn derive_single_pass_preserves_first_duplicate_key_semantics() {
    #[derive(Debug, MmdbDecode)]
    struct Pair<'a> {
        country: &'a str,
        score: u32,
    }

    // `ValueRef::get` historically returned the first duplicate. The generated
    // one-pass decoder must keep that observable behavior while avoiding one
    // complete map scan per struct field.
    let value = ValueRef::Map(vec![
        ("country", ValueRef::Utf8("FR-first")),
        ("country", ValueRef::Utf8("FR-second")),
        ("ignored", ValueRef::Bool(true)),
        ("score", ValueRef::Uint32(7)),
    ]);
    let decoded = Pair::decode(&value).unwrap();
    assert_eq!(decoded.country, "FR-first");
    assert_eq!(decoded.score, 7);
}

#[cfg(feature = "derive")]
#[test]
fn derive_decode_honors_serde_field_renames() {
    #[derive(Debug, MmdbDecode)]
    struct LocalizedNames<'a> {
        #[serde(rename = "pt-BR")]
        pt_br: &'a str,
        #[serde(rename(deserialize = "zh-CN", serialize = "zh-CN"))]
        zh_cn: String,
    }

    let value = ValueRef::Map(vec![
        ("pt-BR", ValueRef::Utf8("Brasil")),
        ("zh-CN", ValueRef::Utf8("中国")),
    ]);
    let decoded = LocalizedNames::decode(&value).unwrap();
    assert_eq!(decoded.pt_br, "Brasil");
    assert_eq!(decoded.zh_cn, "中国");
}

#[test]
fn ipv4_roundtrip_and_longest_prefix() {
    let mut writer = Writer::with_metadata(metadata(4));
    writer
        .insert_value(
            "10.0.0.0/8".parse().unwrap(),
            Value::Map(BTreeMap::from([
                ("country".into(), Value::Utf8("FR".into())),
                ("score".into(), Value::Uint32(1)),
            ])),
        )
        .unwrap();
    writer
        .insert_value(
            "10.1.0.0/16".parse().unwrap(),
            Value::Map(BTreeMap::from([
                ("country".into(), Value::Utf8("FR".into())),
                ("city".into(), Value::Utf8("Paris".into())),
                ("score".into(), Value::Uint32(2)),
            ])),
        )
        .unwrap();

    let bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    let record: Borrowed<'_> = reader
        .lookup_borrowed("10.1.2.3".parse::<IpAddr>().unwrap())
        .unwrap();
    assert_eq!(record.country, "FR");
    assert_eq!(record.city, Some("Paris"));
    assert_eq!(record.score, 2);

    let broader: Borrowed<'_> = reader
        .lookup_borrowed("10.9.2.3".parse::<IpAddr>().unwrap())
        .unwrap();
    assert_eq!(broader.city, None);
    assert_eq!(broader.score, 1);

    let city_len = reader
        .lookup_borrowed_map("10.1.2.3".parse().unwrap(), |record: Borrowed<'_>| {
            record.city.unwrap().len()
        })
        .unwrap();
    assert_eq!(city_len, Some(5));

    let mut called = false;
    let missing = reader
        .lookup_borrowed_map("11.0.0.1".parse().unwrap(), |_: Borrowed<'_>| {
            called = true;
        })
        .unwrap();
    assert_eq!(missing, None);
    assert!(!called);
}

#[test]
fn ipv6_and_ipv4_in_v6_database() {
    #[derive(MmdbDecode)]
    struct Name<'a> {
        name: &'a str,
    }

    let mut writer = Writer::with_metadata(metadata(6));
    writer
        .insert(
            "2001:db8::/32".parse().unwrap(),
            &serde_json::json!({"name":"v6"}),
        )
        .unwrap();
    writer
        .insert(
            "192.0.2.0/24".parse().unwrap(),
            &serde_json::json!({"name":"v4"}),
        )
        .unwrap();
    let bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    let v6 = reader.lookup_value("2001:db8::1".parse().unwrap()).unwrap();
    let v4 = reader.lookup_value("192.0.2.4".parse().unwrap()).unwrap();
    assert_eq!(v6.get("name"), Some(&libmaxminddb_rs::ValueRef::Utf8("v6")));
    assert_eq!(v4.get("name"), Some(&libmaxminddb_rs::ValueRef::Utf8("v4")));
    assert_eq!(
        reader
            .lookup_borrowed_map("2001:db8::1".parse().unwrap(), |value: Name<'_>| value.name)
            .unwrap(),
        Some("v6")
    );
    assert_eq!(
        reader
            .lookup_borrowed_map("192.0.2.4".parse().unwrap(), |value: Name<'_>| value.name)
            .unwrap(),
        Some("v4")
    );
}

#[test]
fn metadata_roundtrip() {
    let m = metadata(4);
    let bytes = Writer::with_metadata(m.clone()).finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    assert_eq!(reader.metadata().database_type, m.database_type);
    assert_eq!(
        reader.metadata().description.get("en"),
        Some(&"roundtrip".to_string())
    );
    assert_eq!(reader.metadata().binary_format_major_version, 2);
}

#[test]
fn corruption_is_rejected() {
    assert!(Reader::from_bytes(b"not-mmdb").is_err());
    let mut bytes = Writer::with_metadata(metadata(4)).finish().unwrap();
    bytes[0] = 1;
    // Empty writer has one tree node; damage may remain structurally valid, so damage the separator.
    bytes[6] = 1;
    assert!(Reader::from_bytes(&bytes).is_err());
}

#[cfg(feature = "derive")]
#[test]
fn custom_record_carries_network_and_excludes_it_from_payload() {
    use libmaxminddb_rs::{IpNetwork, MmdbEncode, MmdbRecord};

    #[derive(MmdbEncode, MmdbRecord)]
    struct Entry<'a> {
        #[mmdb(network)]
        network: IpNetwork,
        country: &'a str,
        category: &'a str,
    }

    let mut writer = Writer::with_metadata(metadata(4));
    let entry = Entry {
        network: "203.0.113.0/24".parse().unwrap(),
        country: "FR",
        category: "example",
    };
    writer.insert_entry(&entry).unwrap();
    let bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    let value = reader
        .lookup_value("203.0.113.7".parse::<IpAddr>().unwrap())
        .unwrap();
    assert_eq!(
        value.get("country"),
        Some(&libmaxminddb_rs::ValueRef::Utf8("FR"))
    );
    assert_eq!(
        value.get("category"),
        Some(&libmaxminddb_rs::ValueRef::Utf8("example"))
    );
    assert!(value.get("network").is_none());
}

#[cfg(feature = "derive")]
#[test]
fn nested_custom_structs_and_arrays_roundtrip() {
    use libmaxminddb_rs::MmdbEncode;

    #[derive(MmdbEncode)]
    struct LocationOut<'a> {
        city: &'a str,
    }

    #[derive(MmdbEncode)]
    struct RecordOut<'a> {
        country: &'a str,
        location: LocationOut<'a>,
        tags: Vec<&'a str>,
    }

    #[derive(Debug, MmdbDecode)]
    struct LocationIn<'a> {
        city: &'a str,
    }

    #[derive(Debug, MmdbDecode)]
    struct RecordIn<'a> {
        country: &'a str,
        location: LocationIn<'a>,
        tags: Vec<&'a str>,
    }

    let mut writer = Writer::with_metadata(metadata(4));
    writer
        .insert_encoded(
            "198.51.100.0/24".parse().unwrap(),
            &RecordOut {
                country: "FR",
                location: LocationOut { city: "Paris" },
                tags: vec!["proxy", "abuse"],
            },
        )
        .unwrap();
    let bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    let value: RecordIn<'_> = reader
        .lookup_borrowed("198.51.100.8".parse::<IpAddr>().unwrap())
        .unwrap();
    assert_eq!(value.country, "FR");
    assert_eq!(value.location.city, "Paris");
    assert_eq!(value.tags, vec!["proxy", "abuse"]);
}

#[test]
fn owned_file_and_mmap_sources_work() {
    use std::io::Write;

    let mut writer = Writer::with_metadata(metadata(4));
    writer
        .insert(
            "203.0.113.0/24".parse().unwrap(),
            &serde_json::json!({"name": "source-test"}),
        )
        .unwrap();
    let bytes = writer.finish().unwrap();

    let owned = Reader::from_vec(bytes.clone()).unwrap();
    assert_eq!(
        owned
            .lookup_value("203.0.113.9".parse::<IpAddr>().unwrap())
            .unwrap()
            .get("name"),
        Some(&libmaxminddb_rs::ValueRef::Utf8("source-test"))
    );

    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&bytes).unwrap();
    file.flush().unwrap();

    let file_reader = Reader::open(file.path()).unwrap();
    assert!(
        file_reader
            .lookup_value("203.0.113.9".parse::<IpAddr>().unwrap())
            .is_ok()
    );

    // SAFETY: the temporary file remains alive and is not modified/truncated while mapped.
    let mmap_reader = unsafe { Reader::open_mmap(file.path()).unwrap() };
    assert!(
        mmap_reader
            .lookup_value("203.0.113.9".parse::<IpAddr>().unwrap())
            .is_ok()
    );
}

#[test]
fn reserved_tree_pointer_range_is_rejected_during_lookup() {
    let bytes = Writer::with_metadata(metadata(4)).finish().unwrap();
    let mut corrupted = bytes;
    // node_count is 1 for an empty writer. Pointer value 2 lies in the reserved
    // node_count+1..node_count+15 range and must never be treated as data.
    corrupted[0..3].copy_from_slice(&[0, 0, 2]);
    let reader = Reader::from_bytes(&corrupted).unwrap();
    let error = reader
        .lookup_value("1.2.3.4".parse::<IpAddr>().unwrap())
        .unwrap_err();
    assert!(matches!(error, libmaxminddb_rs::Error::InvalidOffset(_)));
    let mapped = reader.lookup_borrowed_map("1.2.3.4".parse().unwrap(), |_: Borrowed<'_>| ());
    assert!(matches!(
        mapped,
        Err(libmaxminddb_rs::Error::InvalidOffset(_))
    ));
}

#[cfg(feature = "derive")]
#[test]
fn optional_encoded_fields_are_omitted_when_none() {
    use libmaxminddb_rs::MmdbEncode;

    #[derive(MmdbEncode)]
    struct OptionalRecord<'a> {
        country: &'a str,
        city: Option<&'a str>,
    }

    let mut writer = Writer::with_metadata(metadata(4));
    writer
        .insert_encoded(
            "192.0.2.0/24".parse().unwrap(),
            &OptionalRecord {
                country: "FR",
                city: None,
            },
        )
        .unwrap();
    let bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    let value = reader
        .lookup_value("192.0.2.10".parse::<IpAddr>().unwrap())
        .unwrap();
    assert_eq!(
        value.get("country"),
        Some(&libmaxminddb_rs::ValueRef::Utf8("FR"))
    );
    assert!(value.get("city").is_none());
}

#[test]
fn lookup_many_matches_sequential_including_parallel_path() {
    let mut writer = Writer::with_metadata(metadata(4));
    for (network, name) in [("10.0.0.0/24", "zero"), ("10.0.2.0/24", "two")] {
        writer
            .insert(
                network.parse().unwrap(),
                &serde_json::json!({ "name": name }),
            )
            .unwrap();
    }
    let bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();

    // Keep this above Reader::lookup_many's parallel threshold so the test covers
    // both the sequential and scoped-worker implementations.
    let ips: Vec<IpAddr> = (0..5_000)
        .map(|i| format!("10.0.{}.{}", i % 4, (i / 4) % 256).parse().unwrap())
        .collect();

    for batch in [&ips[..4], &ips[..]] {
        let results = reader.lookup_many(batch);
        assert_eq!(results.len(), batch.len());
        for (ip, result) in batch.iter().zip(&results) {
            match (reader.lookup_value(*ip), result) {
                (Ok(expected), Ok(actual)) => assert_eq!(&expected, actual),
                (Err(_), Err(_)) => {}
                _ => panic!("batch and sequential disagree for {ip}"),
            }
        }
    }
}
