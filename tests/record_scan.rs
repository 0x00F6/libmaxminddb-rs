#![cfg(all(feature = "reader", feature = "writer"))]

use std::collections::{BTreeMap, BTreeSet};

use libmaxminddb_rs::{Error, MetadataBuilder, Reader, Value, ValueRef, Writer};

fn writer(ip_version: u16) -> Writer {
    Writer::with_metadata(
        MetadataBuilder::new()
            .database_type("scan-fixture")
            .ip_version(ip_version)
            .build()
            .unwrap(),
    )
}

#[test]
fn empty_ipv4_and_ipv6_databases_have_no_records() {
    for version in [4, 6] {
        let bytes = writer(version).finish().unwrap();
        let reader = Reader::from_bytes(&bytes).unwrap();
        let mut calls = 0;
        reader
            .visit_records(|_, _| {
                calls += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(calls, 0);
    }
}

#[test]
fn ranges_preserve_longest_prefix_values_and_skip_deleted_holes() {
    for version in [4, 6] {
        let mut writer = writer(version);
        for (network, value) in [
            ("10.0.0.0/8", "broad"),
            ("10.1.0.0/16", "narrow"),
            ("192.0.2.0/31", "non-byte-aligned"),
            ("255.255.255.255/32", "last"),
        ] {
            writer
                .insert_value(network.parse().unwrap(), Value::Utf8(value.into()))
                .unwrap();
        }
        writer.remove("10.2.0.0/16".parse().unwrap()).unwrap();
        if version == 6 {
            writer
                .insert_value("2001:db8::/127".parse().unwrap(), Value::Utf8("v6".into()))
                .unwrap();
            writer
                .insert_value(
                    "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff/128"
                        .parse()
                        .unwrap(),
                    Value::Utf8("last-v6".into()),
                )
                .unwrap();
        }
        let bytes = writer.finish().unwrap();
        let reader = Reader::from_bytes(&bytes).unwrap();
        let hole: libmaxminddb_rs::IpNetwork = "10.2.0.0/16".parse().unwrap();
        let mut rows = Vec::new();
        reader
            .visit_records(|network, value| {
                assert!(!network.contains(&hole.addr()));
                assert_eq!(reader.lookup_value(network.addr())?, value);
                assert_eq!(reader.lookup_value(network.broadcast())?, value);
                rows.push((network, value));
                Ok(())
            })
            .unwrap();
        for name in ["broad", "narrow", "non-byte-aligned", "last"] {
            assert!(rows.iter().any(|(_, value)| *value == ValueRef::Utf8(name)));
        }
        assert!(rows.iter().any(|(n, _)| n.to_string() == "192.0.2.0/31"));
        if version == 6 {
            assert!(rows.iter().any(|(n, _)| n.to_string() == "2001:db8::/127"));
            assert!(rows.iter().any(|(n, _)| n.prefix_len() == 128));
        }
    }
}

#[test]
fn shared_payloads_remain_separate_ranges_and_unique_fields_keep_raw_values() {
    let mut writer = writer(4);
    let value = Value::Map(BTreeMap::from([
        (
            "files".into(),
            Value::Array(vec![
                Value::Utf8("source-a.ipset".into()),
                Value::Utf8("source-b.netset".into()),
                Value::Utf8("source-a.ipset".into()),
            ]),
        ),
        (
            "categories".into(),
            Value::Array(vec![
                Value::Utf8("malware".into()),
                Value::Utf8("custom-category".into()),
                Value::Utf8("malware".into()),
            ]),
        ),
    ]));
    writer
        .insert_value("192.0.2.0/24".parse().unwrap(), value.clone())
        .unwrap();
    writer
        .insert_value("198.51.100.0/24".parse().unwrap(), value)
        .unwrap();
    let bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    let mut files = BTreeSet::new();
    let mut categories = BTreeSet::new();
    let mut ranges = 0;
    reader
        .visit_records(|_, value| {
            ranges += 1;
            for (field, set) in [("files", &mut files), ("categories", &mut categories)] {
                let Some(ValueRef::Array(values)) = value.get(field) else {
                    panic!("missing array")
                };
                for value in values {
                    let ValueRef::Utf8(text) = value else {
                        panic!("not a string")
                    };
                    set.insert(*text);
                }
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(ranges, 2);
    assert_eq!(files, BTreeSet::from(["source-a.ipset", "source-b.netset"]));
    assert_eq!(categories, BTreeSet::from(["custom-category", "malware"]));
}

#[test]
fn zero_prefix_covers_all_ipv4_addresses_without_enumerating_ips() {
    let mut writer = writer(4);
    writer
        .insert_value("0.0.0.0/0".parse().unwrap(), Value::Utf8("all".into()))
        .unwrap();
    let bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    let mut ranges = Vec::new();
    reader
        .visit_records(|network, value| {
            ranges.push(network);
            assert_eq!(value, ValueRef::Utf8("all"));
            Ok(())
        })
        .unwrap();
    // The serialized tree may split an insertion into two /1 leaves.
    assert_eq!(
        ranges
            .iter()
            .map(|n| 1_u64 << (32 - n.prefix_len()))
            .sum::<u64>(),
        1_u64 << 32
    );
}

#[test]
fn malformed_payload_and_reserved_tree_pointer_are_not_hidden() {
    let mut writer = writer(4);
    writer
        .insert_value("0.0.0.0/1".parse().unwrap(), Value::Utf8("value".into()))
        .unwrap();
    let mut bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    let count = reader.metadata().node_count as usize;
    assert_eq!(count, 1);
    let data = count * 6 + 16;
    bytes[data] = 0;
    let reader = Reader::from_bytes(&bytes).unwrap();
    assert!(reader.visit_records(|_, _| Ok(())).is_err());
    bytes[..3].copy_from_slice(&((count + 1) as u32).to_be_bytes()[1..]);
    let reader = Reader::from_bytes(&bytes).unwrap();
    assert!(matches!(
        reader.visit_records(|_, _| Ok(())),
        Err(Error::InvalidOffset(_))
    ));
}

#[cfg(feature = "derive")]
#[test]
fn typed_and_generic_scans_agree_and_borrow_bytes_for_all_source_backends() {
    #[derive(libmaxminddb_rs::MmdbDecode)]
    struct Record<'a> {
        file: &'a str,
        bytes: &'a [u8],
        categories: Vec<&'a str>,
    }
    let mut writer = writer(6);
    let value = Value::Map(BTreeMap::from([
        ("file".into(), Value::Utf8("source.ipset".into())),
        ("bytes".into(), Value::Bytes(vec![0, 1, 255])),
        (
            "categories".into(),
            Value::Array(vec![Value::Utf8("custom".into())]),
        ),
    ]));
    for network in ["192.0.2.0/31", "2001:db8::/127"] {
        writer
            .insert_value(network.parse().unwrap(), value.clone())
            .unwrap();
    }
    let bytes = writer.finish().unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), &bytes).unwrap();
    // SAFETY: the temporary file remains alive and unmodified until all
    // mapped readers and borrowed outputs have been dropped.
    let mmap = unsafe { Reader::open_mmap(file.path()) }.unwrap();
    for reader in [
        Reader::from_bytes(&bytes).unwrap(),
        Reader::from_vec(bytes.clone()).unwrap(),
        Reader::open(file.path()).unwrap(),
        mmap,
    ] {
        let mut generic = Vec::new();
        reader
            .visit_records(|network, value| {
                generic.push((network, value));
                Ok(())
            })
            .unwrap();
        let mut typed = Vec::new();
        reader
            .visit_borrowed_records(|network, record: Record<'_>| {
                let source = reader.as_bytes();
                for borrowed in [
                    record.file.as_bytes(),
                    record.bytes,
                    record.categories[0].as_bytes(),
                ] {
                    let offset = borrowed.as_ptr() as usize - source.as_ptr() as usize;
                    assert_eq!(&source[offset..offset + borrowed.len()], borrowed);
                }
                typed.push((network, record));
                Ok(())
            })
            .unwrap();
        assert_eq!(typed.len(), generic.len());
        for ((network, record), (other_network, value)) in typed.iter().zip(&generic) {
            assert_eq!(network, other_network);
            assert_eq!(value.get("file"), Some(&ValueRef::Utf8(record.file)));
            assert_eq!(value.get("bytes"), Some(&ValueRef::Bytes(record.bytes)));
            assert_eq!(record.categories, ["custom"]);
        }
    }
}

#[cfg(feature = "derive")]
#[test]
fn typed_schema_errors_are_propagated_without_visiting_bad_records() {
    #[derive(libmaxminddb_rs::MmdbDecode)]
    struct Record<'a> {
        file: &'a str,
    }
    let mut writer = writer(4);
    writer
        .insert(
            "192.0.2.0/24".parse().unwrap(),
            &serde_json::json!({"file": "valid"}),
        )
        .unwrap();
    writer
        .insert(
            "198.51.100.0/24".parse().unwrap(),
            &serde_json::json!({"file": 7}),
        )
        .unwrap();
    let bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    let mut calls = 0;
    let result = reader.visit_borrowed_records(|_, record: Record<'_>| {
        assert_eq!(record.file, "valid");
        calls += 1;
        Ok(())
    });
    assert!(matches!(result, Err(Error::DecodingError(_))));
    assert_eq!(calls, 1);
}
