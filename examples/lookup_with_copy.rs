//! Lookups that produce owned data or explicitly copy a borrowed result.
//!
//! `lookup` always returns an owned serde value. The borrowed lookup methods
//! copy payloads only when the requested type owns them (for example `String`).
//! `lookup_value`, `lookup_value_with_prefix`, and `lookup_many` allocate
//! map/array or batch containers, but their strings remain borrowed until
//! `to_owned_value()` is called.
//!
//! Run with `cargo run --example lookup_with_copy`.

mod common;

use std::error::Error;
use std::net::IpAddr;

use libmaxminddb_rs::{MmdbDecode, Reader};
use serde::Deserialize;

#[derive(Debug, PartialEq, MmdbDecode, Deserialize)]
struct OwnedRecord {
    asn: u32,
    country: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let bytes = common::sample_bytes()?;
    let reader = Reader::from_bytes(&bytes)?;
    let ip: IpAddr = "2001:db8:1::1".parse()?;

    // lookup converts through serde; all strings in the result are owned.
    let serde_record: OwnedRecord = reader.lookup(ip)?;
    println!("serde lookup: {serde_record:?}");

    // A derived borrowed lookup also copies when its target has String fields.
    let derived_record: OwnedRecord = reader.lookup_borrowed(ip)?;
    assert_eq!(derived_record, serde_record);
    println!("derived lookup with owned fields: {derived_record:?}");

    // The optional variant has the same owned decoding on a hit.
    let optional_record: Option<OwnedRecord> = reader.lookup_borrowed_opt(ip);
    assert_eq!(optional_record.as_ref(), Some(&serde_record));

    // Mapping an owned record can return just the copied country String.
    let country: Option<String> =
        reader.lookup_borrowed_map(ip, |record: OwnedRecord| record.country)?;
    assert_eq!(country.as_deref(), Some("FR"));
    println!("mapped country: {country:?}");

    // lookup_value borrows the payload; this explicit conversion copies it.
    let borrowed_value = reader.lookup_value(ip)?;
    let owned_value = borrowed_value.to_owned_value();

    // The prefixed lookup can be converted to an owned value in the same way.
    let (borrowed_with_prefix, prefix) = reader.lookup_value_with_prefix(ip)?;
    let owned_with_prefix = borrowed_with_prefix.to_owned_value();

    // lookup_many allocates its result Vec; convert each borrowed hit only if
    // the results must outlive the reader or its MMDB buffer.
    let addresses = [ip, "198.51.100.1".parse()?];
    let owned_batch: Vec<_> = reader
        .lookup_many(&addresses)
        .into_iter()
        .map(|result| result.map(|value| value.to_owned_value()))
        .collect();

    // Owned values remain usable after the reader and MMDB bytes are dropped.
    drop(reader);
    drop(bytes);
    println!("owned generic value: {owned_value:?}");
    println!("owned value for /{prefix}: {owned_with_prefix:?}");
    for (address, result) in addresses.iter().zip(owned_batch) {
        println!("{address}: {result:?}");
    }

    Ok(())
}
