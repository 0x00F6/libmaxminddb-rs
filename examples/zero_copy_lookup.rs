//! # Zero-Copy Borrowed Lookup Example for libmaxminddb-rs
//!
//! This example groups the reader methods that borrow MMDB bytes.
//!
//! When decoding records from an MMDB database, string fields (`&'a str`) and binary
//! slices (`&'a [u8]`) borrow directly from the underlying database memory
//! (whether loaded in a `Vec<u8>` or mapped via `mmap`).
//!
//! Borrowed strings and byte slices do not allocate. Generic `ValueRef` maps
//! still allocate their container vectors. Typed borrowed decoding avoids them.
//!
//! Run this example with:
//! ```bash
//! cargo run --example zero_copy_lookup
//! ```

use std::error::Error;
use std::net::IpAddr;

use libmaxminddb_rs::{MetadataBuilder, MmdbDecode, MmdbEncode, Reader, ValueRef, Writer};

/// An IP intelligence record with zero-copy borrowed strings.
///
/// The lifetime `'a` is tied to the `Reader` and its memory buffer.
/// None of the fields below allocate on the heap when deserializing.
#[derive(Debug, PartialEq, MmdbEncode, MmdbDecode)]
struct IpIntelligence<'a> {
    asn: u32,
    org: &'a str,
    country_iso: &'a str,
    threat_category: Option<&'a str>,
    is_anonymous_proxy: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== libmaxminddb-rs Zero-Copy Lookup Example ===\n");

    // 1. Create a sample IP intelligence database
    let metadata = MetadataBuilder::new()
        .database_type("Zero-Copy-Intel-DB")
        .ip_version(4)
        .description(
            "en",
            "IP intelligence database demonstrating zero-copy decoding",
        )
        .build()?;

    let mut writer = Writer::with_metadata(metadata);

    // Insert network ranges
    writer.insert_encoded(
        "198.51.100.0/24".parse()?,
        &IpIntelligence {
            asn: 64512,
            org: "Documentation Reserved AS",
            country_iso: "US",
            threat_category: Some("datacenter"),
            is_anonymous_proxy: false,
        },
    )?;

    writer.insert_encoded(
        "203.0.113.128/25".parse()?,
        &IpIntelligence {
            asn: 64513,
            org: "Test Threat Network Inc.",
            country_iso: "NL",
            threat_category: Some("tor_exit_node"),
            is_anonymous_proxy: true,
        },
    )?;

    let database_bytes = writer.finish()?;
    println!(
        "1. Database prepared in memory ({} bytes).\n",
        database_bytes.len()
    );

    // 2. Open with Reader
    let reader = Reader::from_bytes(&database_bytes)?;

    // 3. Perform zero-copy borrowed lookup
    let target_ip: IpAddr = "203.0.113.199".parse()?;
    println!(
        "2. Performing zero-copy borrowed lookup for IP: {}",
        target_ip
    );

    // `lookup_borrowed` returns `IpIntelligence<'_>`.
    // Its string references (`org`, `country_iso`, `threat_category`) point
    // directly into `database_bytes` without copying.
    let intel: IpIntelligence<'_> = reader.lookup_borrowed(target_ip)?;

    println!("   ASN          : AS{}", intel.asn);
    println!("   Organization : {}", intel.org);
    println!("   Country      : {}", intel.country_iso);
    println!("   Threat       : {:?}", intel.threat_category);
    println!("   Proxy Flag   : {}", intel.is_anonymous_proxy);

    // `lookup_borrowed_opt` preserves borrowed fields and returns None on a miss.
    let missing_ip: IpAddr = "192.0.2.1".parse()?;
    let missing: Option<IpIntelligence<'_>> = reader.lookup_borrowed_opt(missing_ip);
    println!("   Optional lookup missed: {}", missing.is_none());

    // `lookup_value` returns the encoded record as a generic ValueRef map.
    // `get` borrows each field from that map without copying string payloads.
    // The map container itself is allocated by generic decoding.
    let generic_value = reader.lookup_value(target_ip)?;
    let Some(ValueRef::Uint32(asn)) = generic_value.get("asn") else {
        return Err("expected a u32 asn field".into());
    };
    let Some(ValueRef::Utf8(org)) = generic_value.get("org") else {
        return Err("expected a string org field".into());
    };
    let Some(ValueRef::Utf8(country_iso)) = generic_value.get("country_iso") else {
        return Err("expected a string country_iso field".into());
    };
    let threat_category = match generic_value.get("threat_category") {
        Some(ValueRef::Utf8(category)) => Some(*category),
        None => None, // An absent field represents the optional None value.
        _ => return Err("expected a string threat_category field".into()),
    };
    let Some(ValueRef::Bool(is_anonymous_proxy)) = generic_value.get("is_anonymous_proxy") else {
        return Err("expected a boolean is_anonymous_proxy field".into());
    };
    println!("   ValueRef ASN          : AS{asn}");
    println!("   ValueRef Organization : {org}");
    println!("   ValueRef Country      : {country_iso}");
    println!("   ValueRef Threat       : {threat_category:?}");
    println!("   ValueRef Proxy Flag   : {is_anonymous_proxy}");
    assert_eq!(*asn, intel.asn);
    assert_eq!(*org, intel.org);
    assert_eq!(*country_iso, intel.country_iso);
    assert_eq!(threat_category, intel.threat_category);
    assert_eq!(*is_anonymous_proxy, intel.is_anonymous_proxy);

    // `lookup_value_with_prefix` adds the exact matched prefix length.
    println!("\n3. Inspecting subnet prefix length:");
    let (value_ref, prefix_len) = reader.lookup_value_with_prefix(target_ip)?;
    println!("   Matched subnet prefix: /{}\n", prefix_len);

    // The ValueRef returned with a prefix exposes fields in the same way.
    if let Some(ValueRef::Utf8(country_iso)) = value_ref.get("country_iso") {
        println!("   Country from prefixed lookup: {country_iso}");
    }

    // 5. Inspect generic borrowed ValueRef dynamically
    if let ValueRef::Map(entries) = value_ref {
        println!("4. Dynamic traversal of borrowed map entries:");
        for (key, val) in entries {
            println!("   ↳ Key: {:<18} | Value: {:?}", key, val);
        }
    }

    println!("\nZero-copy lookup example completed successfully!");
    Ok(())
}
