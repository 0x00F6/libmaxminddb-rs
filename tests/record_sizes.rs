#![cfg(all(feature = "reader", feature = "writer"))]

//! Verifies that the reader traverses and decodes every valid 24–64-bit search
//! trees identically. The writer only emits 24-bit trees for small databases,
//! so this test re-encodes the tree nodes of a small writer output into the
//! larger record layouts and patches the metadata `record_size` integer.

use std::net::IpAddr;

use libmaxminddb_rs::{MetadataBuilder, Reader, Value, Writer};

const MARKER: &[u8] = b"\xab\xcd\xefMaxMind.com";

fn metadata(ip_version: u16) -> libmaxminddb_rs::Metadata {
    MetadataBuilder::new()
        .database_type("record-sizes")
        .ip_version(ip_version)
        .build()
        .unwrap()
}

fn ipv4_db() -> Vec<u8> {
    let mut writer = Writer::with_metadata(metadata(4));
    writer
        .insert_value(
            "10.0.0.0/8".parse().unwrap(),
            Value::Map(Into::into({
                let mut m = std::collections::BTreeMap::new();
                m.insert("name".into(), Value::Utf8("broad".into()));
                m
            })),
        )
        .unwrap();
    writer
        .insert_value(
            "10.1.0.0/16".parse().unwrap(),
            Value::Map(Into::into({
                let mut m = std::collections::BTreeMap::new();
                m.insert("name".into(), Value::Utf8("narrow".into()));
                m
            })),
        )
        .unwrap();
    writer.finish().unwrap()
}

fn ipv6_db() -> Vec<u8> {
    let mut writer = Writer::with_metadata(metadata(6));
    writer
        .insert_value(
            "2001:db8::/32".parse().unwrap(),
            Value::Map(Into::into({
                let mut m = std::collections::BTreeMap::new();
                m.insert("name".into(), Value::Utf8("v6".into()));
                m
            })),
        )
        .unwrap();
    writer
        .insert_value(
            "192.0.2.0/24".parse().unwrap(),
            Value::Map(Into::into({
                let mut m = std::collections::BTreeMap::new();
                m.insert("name".into(), Value::Utf8("v4".into()));
                m
            })),
        )
        .unwrap();
    writer.finish().unwrap()
}

/// Re-encodes the search tree of a writer-produced 24-bit database into a
/// different record size, keeping the separator, data section, marker and
/// metadata intact. The metadata `record_size` Uint16 is patched in place
/// (min-width encoded: `0xA1` control byte plus payload) so the file stays
/// structurally valid.
fn reencode_tree(base: &[u8], record_size: u16) -> Vec<u8> {
    let base_reader = Reader::from_bytes(base).unwrap();
    assert_eq!(base_reader.metadata().record_size, 24);
    let node_count = base_reader.metadata().node_count as usize;
    let src_node_size = 6;
    let tree_end = node_count * src_node_size;
    let base_marker_pos = base
        .windows(MARKER.len())
        .rposition(|w| w == MARKER)
        .expect("metadata marker");
    let metadata_len = base.len() - base_marker_pos - MARKER.len();

    let mut out = Vec::with_capacity(base.len() + tree_end);
    for i in 0..node_count {
        let n = &base[i * src_node_size..(i + 1) * src_node_size];
        let left = (u64::from(n[0]) << 16) | (u64::from(n[1]) << 8) | u64::from(n[2]);
        let right = (u64::from(n[3]) << 16) | (u64::from(n[4]) << 8) | u64::from(n[5]);
        match record_size {
            28 => out.extend_from_slice(&[
                (left >> 16) as u8,
                (left >> 8) as u8,
                left as u8,
                (((left >> 24) & 0x0f) << 4 | ((right >> 24) & 0x0f)) as u8,
                (right >> 16) as u8,
                (right >> 8) as u8,
                right as u8,
            ]),
            32 => {
                out.extend_from_slice(&(left as u32).to_be_bytes());
                out.extend_from_slice(&(right as u32).to_be_bytes());
            }
            36..=64 if record_size.is_multiple_of(4) => {
                let bits = usize::from(record_size);
                let mut packed = vec![0_u8; bits / 4];
                for (side, value) in [left, right].into_iter().enumerate() {
                    for bit in 0..bits {
                        let stream_bit = side * bits + bit;
                        packed[stream_bit / 8] |=
                            (((value >> (bits - bit - 1)) & 1) as u8) << (7 - stream_bit % 8);
                    }
                }
                out.extend_from_slice(&packed);
            }
            other => panic!("unsupported test record size {other}"),
        }
    }
    out.extend_from_slice(&base[tree_end..]);

    let metadata_pos = out.len() - metadata_len;
    let old = [0xa1, 0x18];
    let new = [0xa1, record_size as u8];
    let rel = out[metadata_pos..]
        .windows(2)
        .position(|w| w == old)
        .expect("record_size field in metadata");
    out[metadata_pos + rel..metadata_pos + rel + 2].copy_from_slice(&new);
    out
}

fn name_of<'a>(
    reader: &'a Reader<'a>,
    ip: &str,
) -> Result<Option<&'a str>, libmaxminddb_rs::Error> {
    match reader.lookup_value(ip.parse::<IpAddr>().unwrap()) {
        Ok(value) => match value.get("name") {
            Some(libmaxminddb_rs::ValueRef::Utf8(v)) => Ok(Some(v)),
            Some(_) => panic!("name is not a string"),
            None => Ok(None),
        },
        Err(e) => Err(e),
    }
}

fn check_db(base: &[u8], expected: &[(&str, Option<&str>)], miss: &[&str]) {
    for record_size in [24_u16, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64] {
        let bytes = if record_size == 24 {
            base.to_vec()
        } else {
            reencode_tree(base, record_size)
        };
        let reader = Reader::from_bytes(&bytes).unwrap();
        assert_eq!(reader.metadata().record_size, record_size);
        let mut scanned = 0;
        reader
            .visit_records(|network, value| {
                assert_eq!(reader.lookup_value(network.addr())?, value);
                assert_eq!(reader.lookup_value(network.broadcast())?, value);
                scanned += 1;
                Ok(())
            })
            .unwrap();
        assert!(scanned > 0);
        for (ip, want) in expected {
            assert_eq!(
                &name_of(&reader, ip).unwrap(),
                want,
                "record_size={record_size} ip={ip}"
            );
        }
        for ip in miss {
            let result = name_of(&reader, ip);
            assert!(
                matches!(result, Err(libmaxminddb_rs::Error::NotFound)),
                "record_size={record_size} ip={ip} expected NotFound, got {result:?}"
            );
        }
    }
}

#[test]
fn ipv4_tree_all_record_sizes() {
    let base = ipv4_db();
    check_db(
        &base,
        &[
            ("10.1.2.3", Some("narrow")),
            ("10.1.255.255", Some("narrow")),
            ("10.9.2.3", Some("broad")),
            ("10.0.0.1", Some("broad")),
        ],
        &["8.8.8.8", "192.0.2.1"],
    );
}

#[test]
fn ipv6_tree_all_record_sizes() {
    let base = ipv6_db();
    check_db(
        &base,
        &[
            ("2001:db8::1", Some("v6")),
            ("2001:db8::ffff:ffff:ffff:ffff", Some("v6")),
            ("192.0.2.4", Some("v4")),
            ("192.0.2.254", Some("v4")),
        ],
        &[
            "2001:db9::1",
            "2607:f8b0::1",
            // An IPv4 address that maps to empty v4 subtree.
            "198.51.100.1",
        ],
    );
}
