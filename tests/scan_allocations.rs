#![cfg(all(feature = "reader", feature = "derive"))]

use libmaxminddb_rs::{MmdbDecode, Reader};

#[path = "../benches/support/allocations.rs"]
mod allocations;

#[derive(MmdbDecode)]
struct Record<'a> {
    category: &'a str,
}

#[test]
fn complete_borrowed_scalar_scan_has_no_heap_allocations() {
    let reader = Reader::from_bytes(include_bytes!("fixtures/doc.mmdb")).unwrap();
    let mut calls = 0;
    let (result, stats) = allocations::measure(|| {
        reader.visit_borrowed_records(|_, record: Record<'_>| {
            assert_eq!(record.category, "compat");
            calls += 1;
            Ok(())
        })
    });
    result.unwrap();
    assert!(calls > 0);
    assert_eq!(stats.allocations, 0);
    assert_eq!(stats.requested_bytes, 0);
}
