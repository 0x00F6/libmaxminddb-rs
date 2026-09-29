//! Check whether an address has a matching network without decoding data.
//!
//! Run with `cargo run --example lookup_exists`.

mod common;

use std::error::Error;
use std::net::IpAddr;

use libmaxminddb_rs::{Error as MmdbError, MmdbDecode, Reader};

#[derive(MmdbDecode)]
struct SampleRecord<'a> {
    asn: u32,
    country: &'a str,
}

fn main() -> Result<(), Box<dyn Error>> {
    let bytes = common::sample_bytes()?;
    let reader = Reader::from_bytes(&bytes)?;
    let present: IpAddr = "198.51.100.1".parse()?;
    let absent: IpAddr = "203.0.113.1".parse()?;

    // This is useful when only membership matters; it skips record decoding.
    println!("{present}: {}", reader.lookup_exists(present));
    println!("{absent}: {}", reader.lookup_exists(absent));

    // A borrowed lookup on an uncovered IP returns Error::NotFound.
    // It does not decode a record when the tree traversal misses.
    match reader.lookup_borrowed::<SampleRecord<'_>>(absent) {
        Err(MmdbError::NotFound) => println!("{absent}: no matching record"),
        Ok(record) => {
            return Err(format!(
                "unexpected record for {absent}: AS{}, {}",
                record.asn, record.country
            )
            .into());
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
