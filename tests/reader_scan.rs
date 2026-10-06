#![cfg(feature = "reader")]

use std::collections::BTreeSet;

use libmaxminddb_rs::{Error, IpNetwork, MmdbDecode, Reader, ValueRef};

const DATABASE: &[u8] = include_bytes!("fixtures/doc.mmdb");

struct Category<'a>(&'a str);

impl<'a> MmdbDecode<'a> for Category<'a> {
    fn decode(value: &ValueRef<'a>) -> libmaxminddb_rs::Result<Self> {
        match value.get("category") {
            Some(ValueRef::Utf8(category)) => Ok(Self(category)),
            _ => Err(Error::DecodingError("expected a category string".into())),
        }
    }
}

#[test]
fn reader_only_scan_retains_borrowed_strings_and_matches_lookup() {
    let reader = Reader::from_bytes(DATABASE).unwrap();
    let mut categories = BTreeSet::new();
    let mut networks = Vec::new();
    reader
        .visit_records(|network, value| {
            assert_eq!(reader.lookup_value(network.addr())?, value);
            if let Some(ValueRef::Utf8(category)) = value.get("category") {
                categories.insert(*category);
                let source = reader.as_bytes();
                let offset = category.as_ptr() as usize - source.as_ptr() as usize;
                assert_eq!(
                    &source[offset..offset + category.len()],
                    category.as_bytes()
                );
            }
            networks.push(network);
            Ok(())
        })
        .unwrap();
    assert!(categories.contains("compat"));
    assert!(!networks.is_empty());
    assert!(networks.windows(2).all(|n| n[0].addr() < n[1].addr()));
}

#[test]
fn reader_only_typed_scan_uses_manual_decode_without_derive() {
    let reader = Reader::from_bytes(DATABASE).unwrap();
    let mut records: Vec<(IpNetwork, &str)> = Vec::new();
    reader
        .visit_borrowed_records(|network, category: Category<'_>| {
            records.push((network, category.0));
            Ok(())
        })
        .unwrap();
    assert!(records.iter().any(|(_, category)| *category == "compat"));
}

#[test]
fn callback_error_stops_both_scans_immediately() {
    let reader = Reader::from_bytes(DATABASE).unwrap();
    let mut calls = 0;
    let result = reader.visit_records(|_, _| {
        calls += 1;
        Err(Error::InvalidDatabase("caller stopped the scan"))
    });
    assert!(matches!(
        result,
        Err(Error::InvalidDatabase("caller stopped the scan"))
    ));
    assert_eq!(calls, 1);
    calls = 0;
    let result = reader.visit_borrowed_records(|_, _: Category<'_>| {
        calls += 1;
        Err(Error::InvalidDatabase("caller stopped the scan"))
    });
    assert!(matches!(
        result,
        Err(Error::InvalidDatabase("caller stopped the scan"))
    ));
    assert_eq!(calls, 1);
}

#[test]
fn retained_strings_keep_their_source_generation_after_publication() {
    use libmaxminddb_rs::ReloadableReader;
    use std::sync::Arc;

    let original = Arc::new(Reader::from_bytes(DATABASE).unwrap());
    let database = ReloadableReader::new(Arc::clone(&original));
    let snapshot = database.snapshot();
    let mut categories = BTreeSet::new();
    snapshot
        .visit_borrowed_records(|_, category: Category<'_>| {
            categories.insert(category.0);
            Ok(())
        })
        .unwrap();
    database.replace(Arc::new(Reader::from_vec(DATABASE.to_vec()).unwrap()));
    drop(original);
    assert!(categories.contains("compat"));
    assert!(snapshot.lookup_exists("203.0.113.7".parse().unwrap()));
}
