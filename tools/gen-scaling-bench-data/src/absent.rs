//! Deterministic host routes and independently sampled, verified misses.
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::net::Ipv6Addr;
use std::path::Path;

use libmaxminddb_rs::{Error, IpNetwork, MetadataBuilder, Reader, Writer};

const SEED: u64 = 0x5EED_2024_CAFE_BABE;

fn is_absent(reader: &Reader<'_>, ip: Ipv6Addr) -> Result<bool, Error> {
    match reader.lookup_value(ip.into()) {
        Err(Error::NotFound) => Ok(true),
        Ok(_) => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn generate(dir: &Path, count: usize) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(dir)?;
    let metadata = MetadataBuilder::new()
        .ip_version(6)
        .database_type("GeoIP2-City")
        .build_epoch(1_700_000_000)
        .build()?;
    let mut writer = Writer::with_metadata(metadata);
    let record = super::shared_record_arc();
    let mut rng = super::SplitMix64::new(SEED);
    let mut entries = BTreeSet::new();
    while entries.len() < count {
        entries.insert(Ipv6Addr::from(rng.next_ipv6()));
    }
    // A sorted insertion order makes byte-for-byte reproduction explicit.
    for &ip in &entries {
        writer.insert_batch_shared([IpNetwork::new(ip.into(), 128)?], &record)?;
    }
    println!("Serializing {} distinct IPv6 /128 entries", entries.len());
    let bytes = writer.finish()?;
    fs::write(dir.join("ipv6.mmdb"), &bytes)?;
    let reader = Reader::from_bytes(&bytes)?;
    for &ip in &entries {
        let (_, prefix) = reader.lookup_value_with_prefix(ip.into())?;
        if prefix != 128 {
            return Err("generated host route has the wrong prefix".into());
        }
    }
    // A separate fixed stream avoids resampling the database's own addresses.
    let mut rng = super::SplitMix64::new(SEED ^ 0xA85E_17ED_D15C_A11E);
    let mut output = BufWriter::new(File::create(dir.join("ipv6-absent.txt"))?);
    let mut misses = BTreeSet::new();
    while misses.len() < count {
        let ip = Ipv6Addr::from(rng.next_ipv6());
        // Query the serialized tree: this also rejects coverage by any prefix.
        if is_absent(&reader, ip)? && misses.insert(ip) {
            writeln!(output, "{ip}")?;
        }
    }
    output.flush()?;
    fs::write(
        dir.join("fixture.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "protocol": "ipv6-absent-v1", "seed": format!("0x{SEED:016X}"),
            "entries": entries.len(), "prefix_length": 128,
            "verified_present": entries.len(), "verified_absent": misses.len(),
            "workload_seed": format!("0x{:016X}", SEED ^ 0xA85E_17ED_D15C_A11E),
        }))?,
    )?;
    println!(
        "Verified {} host routes and {} absent IPv6 queries",
        entries.len(),
        misses.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_is_reproducible_and_all_queries_are_absent() {
        let dir = std::env::temp_dir().join(format!("mmdb-absent-test-{}", std::process::id()));
        generate(&dir, 64).unwrap();
        let bytes = fs::read(dir.join("ipv6.mmdb")).unwrap();
        let workload = fs::read_to_string(dir.join("ipv6-absent.txt")).unwrap();
        let reader = Reader::from_bytes(&bytes).unwrap();
        let addresses: BTreeSet<Ipv6Addr> = workload.lines().map(|s| s.parse().unwrap()).collect();
        assert_eq!(addresses.len(), 64);
        for ip in addresses {
            assert!(is_absent(&reader, ip).unwrap());
        }
        generate(&dir, 64).unwrap();
        assert_eq!(bytes, fs::read(dir.join("ipv6.mmdb")).unwrap());
        assert_eq!(
            workload,
            fs::read_to_string(dir.join("ipv6-absent.txt")).unwrap()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn absence_rejects_addresses_covered_by_a_prefix() {
        let metadata = MetadataBuilder::new().ip_version(6).build().unwrap();
        let mut writer = Writer::with_metadata(metadata);
        writer
            .insert_batch_shared(
                ["2001:db8::/32".parse().unwrap()],
                &super::super::shared_record_arc(),
            )
            .unwrap();
        let bytes = writer.finish().unwrap();
        let reader = Reader::from_bytes(&bytes).unwrap();
        assert!(!is_absent(&reader, "2001:db8:1234::1".parse().unwrap()).unwrap());
        assert!(is_absent(&reader, "2001:db9::1".parse().unwrap()).unwrap());
    }
}
