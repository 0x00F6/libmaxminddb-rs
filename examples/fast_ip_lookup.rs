//! Fast IPv4 and IPv6 lookups when only a small result is needed.
//!
//! Run with `cargo run --release --example fast_ip_lookup`.

use std::error::Error;
use std::net::IpAddr;

use libmaxminddb_rs::{MetadataBuilder, MmdbDecode, MmdbEncode, Reader, Writer};

#[derive(MmdbEncode, MmdbDecode)]
struct NetworkRecord {
    asn: u32,
}

fn lookup_asns(reader: &Reader<'_>, addresses: &[IpAddr]) -> libmaxminddb_rs::Result<(usize, u64)> {
    let mut found = 0;
    let mut asn_sum = 0_u64;

    for &ip in addresses {
        // A miss returns None without decoding a record. On a hit, only the
        // small ASN leaves the callback rather than the decoded record.
        if let Some(asn) = reader.lookup_borrowed_map(ip, |record: NetworkRecord| record.asn)? {
            found += 1;
            asn_sum += u64::from(asn);
        }
    }

    Ok((found, asn_sum))
}

fn main() -> Result<(), Box<dyn Error>> {
    // A self-contained fixture. In an application, open the MMDB once and
    // reuse its Reader for both address families.
    let metadata = MetadataBuilder::new()
        .database_type("Fast-IP-Lookup-Example")
        .ip_version(6)
        .build()?;
    let mut writer = Writer::with_metadata(metadata);
    writer.insert_encoded("198.51.100.0/24".parse()?, &NetworkRecord { asn: 64513 })?;
    writer.insert_encoded("2001:db8:1::/48".parse()?, &NetworkRecord { asn: 64512 })?;
    let bytes = writer.finish()?;
    let reader = Reader::from_bytes(&bytes)?;

    // The fast tree is already prepared when Reader::from_bytes returns.
    // Parse addresses outside the lookup loop as well.
    let ipv4: [IpAddr; 3] = [
        "198.51.100.1".parse()?,
        "203.0.113.1".parse()?,
        "198.51.100.2".parse()?,
    ];
    let ipv6: [IpAddr; 3] = [
        "2001:db8:1::1".parse()?,
        "2001:db8:2::1".parse()?,
        "2001:db8:1::2".parse()?,
    ];

    let (found_v4, asn_sum_v4) = lookup_asns(&reader, &ipv4)?;
    let (found_v6, asn_sum_v6) = lookup_asns(&reader, &ipv6)?;
    println!("IPv4: found={found_v4}, asn_sum={asn_sum_v4}");
    println!("IPv6: found={found_v6}, asn_sum={asn_sum_v6}");
    Ok(())
}
