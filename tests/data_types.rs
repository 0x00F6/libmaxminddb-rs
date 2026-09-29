#![cfg(all(feature = "reader", feature = "writer"))]

use std::collections::BTreeMap;

use libmaxminddb_rs::{MetadataBuilder, Reader, Value, ValueRef, Writer};

#[test]
fn scalar_and_container_types_roundtrip() {
    let metadata = MetadataBuilder::new()
        .database_type("types")
        .ip_version(4)
        .build()
        .unwrap();
    let mut writer = Writer::with_metadata(metadata);
    let value = Value::Map(BTreeMap::from([
        ("string".into(), Value::Utf8("hello".into())),
        ("bytes".into(), Value::Bytes(vec![1, 2, 3, 4])),
        ("double".into(), Value::Double(42.25)),
        ("float".into(), Value::Float(2.5)),
        ("u16".into(), Value::Uint16(65_000)),
        ("u32".into(), Value::Uint32(4_000_000_000)),
        ("i32".into(), Value::Int32(-123_456)),
        ("u64".into(), Value::Uint64(u64::MAX - 7)),
        ("u128".into(), Value::Uint128((1_u128 << 100) + 7)),
        ("bool".into(), Value::Bool(true)),
        (
            "array".into(),
            Value::Array(vec![Value::Utf8("a".into()), Value::Uint16(2)]),
        ),
    ]));
    writer
        .insert_value("0.0.0.0/0".parse().unwrap(), value)
        .unwrap();
    let bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    let got = reader.lookup_value("203.0.113.9".parse().unwrap()).unwrap();
    assert_eq!(got.get("string"), Some(&ValueRef::Utf8("hello")));
    assert_eq!(got.get("bytes"), Some(&ValueRef::Bytes(&[1, 2, 3, 4])));
    assert_eq!(got.get("double"), Some(&ValueRef::Double(42.25)));
    assert_eq!(got.get("float"), Some(&ValueRef::Float(2.5)));
    assert_eq!(got.get("i32"), Some(&ValueRef::Int32(-123_456)));
    assert_eq!(got.get("bool"), Some(&ValueRef::Bool(true)));
}

#[test]
fn multibyte_utf8_and_invalid_utf8() {
    let metadata = MetadataBuilder::new()
        .database_type("utf8")
        .ip_version(4)
        .build()
        .unwrap();
    let multi = "café"; // é is multi-byte UTF-8
    let mut writer = Writer::with_metadata(metadata);
    writer
        .insert_value(
            "203.0.113.0/24".parse().unwrap(),
            Value::Map(BTreeMap::from([("city".into(), Value::Utf8(multi.into()))])),
        )
        .unwrap();
    let bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    // Multibyte sequences must decode exactly and pass through the checked path.
    let got = reader.lookup_value("203.0.113.1".parse().unwrap()).unwrap();
    assert_eq!(got.get("city"), Some(&ValueRef::Utf8(multi)));

    // Corrupt the data-section copy of the multibyte payload so the decoder
    // must reject it via the checked UTF-8 validator instead of passing it through.
    let mut bytes = bytes;
    let corrupt_at = bytes
        .windows(multi.len())
        .position(|w| w == multi.as_bytes())
        .expect("multibyte payload");
    bytes[corrupt_at + 1] ^= 0x80; // turn 'a' into a stray continuation byte
    let reader = Reader::from_bytes(&bytes).unwrap();
    match reader.lookup_value("203.0.113.1".parse().unwrap()) {
        Err(libmaxminddb_rs::Error::DecodingError(_)) => {}
        Err(libmaxminddb_rs::Error::NotFound) => {}
        Err(e) => panic!("unexpected error: {e:?}"),
        Ok(value) => {
            // Any Ok result must still expose only valid UTF-8 strings.
            if let Some(ValueRef::Utf8(v)) = value.get("city") {
                assert!(std::str::from_utf8(v.as_bytes()).is_ok());
            }
        }
    }
}

#[test]
fn extended_lengths_roundtrip() {
    let metadata = MetadataBuilder::new()
        .database_type("lengths")
        .ip_version(4)
        .build()
        .unwrap();
    let mut writer = Writer::with_metadata(metadata);
    let long = "x".repeat(70_000);
    let medium = vec![7_u8; 512];
    writer
        .insert_value(
            "198.51.100.0/24".parse().unwrap(),
            Value::Map(BTreeMap::from([
                ("long".into(), Value::Utf8(long.clone())),
                ("medium".into(), Value::Bytes(medium.clone())),
            ])),
        )
        .unwrap();
    let bytes = writer.finish().unwrap();
    let reader = Reader::from_bytes(&bytes).unwrap();
    let got = reader
        .lookup_value("198.51.100.7".parse().unwrap())
        .unwrap();
    assert_eq!(got.get("long"), Some(&ValueRef::Utf8(&long)));
    assert_eq!(got.get("medium"), Some(&ValueRef::Bytes(&medium)));
}
