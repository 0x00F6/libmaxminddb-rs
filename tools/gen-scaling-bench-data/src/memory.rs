use std::{fs, net::Ipv4Addr, path::Path};

use libmaxminddb_rs::{Error, IpNetwork, MetadataBuilder, Reader, Writer};
use serde_json::json;

#[path = "../../memory-protocol.rs"]
mod protocol;

pub fn generate(dir: &Path, entries: usize) -> Result<(), Box<dyn std::error::Error>> {
    if entries == 0 || entries > 5_000_000 {
        return Err("memory fixture size must be 1..=5,000,000".into());
    }
    fs::create_dir_all(dir)?;
    let manifest_path = dir.join("manifest.json");
    if let Ok(text) = fs::read_to_string(&manifest_path) {
        let manifest: serde_json::Value = serde_json::from_str(&text)?;
        if manifest["protocol"] == protocol::PROTOCOL
            && manifest["entries"] == entries
            && fs::metadata(dir.join("database.mmdb"))
                .is_ok_and(|m| Some(m.len()) == manifest["database_bytes"].as_u64())
            && fs::metadata(dir.join("memory-queries.bin"))
                .is_ok_and(|m| m.len() == (protocol::LOOKUPS * 4) as u64)
        {
            return Ok(());
        }
    }
    eprintln!("Preparing RSS fixture: {entries} distinct IPv4 /32 entries");
    let metadata = MetadataBuilder::new()
        .ip_version(4)
        .database_type("GeoIP2-City")
        .build_epoch(1_700_000_000)
        .build()?;
    let mut writer = Writer::with_metadata(metadata);
    let value = super::shared_record_arc();
    for index in 0..entries {
        writer.insert_value_arc(
            IpNetwork::new(Ipv4Addr::from(protocol::route(index as u32)).into(), 32)?,
            value.clone(),
        )?;
    }
    let bytes = writer.finish()?;
    let database_bytes = bytes.len();
    let reader = Reader::from_bytes(&bytes)?;
    // Validate the serialized tree, including absence of unintended CIDR coverage.
    for index in 0..entries {
        let ip = protocol::route(index as u32);
        if reader.lookup_value_with_prefix(Ipv4Addr::from(ip).into())?.1 != 32 {
            return Err("RSS fixture lost an exact /32 route".into());
        }
        if !matches!(
            reader.lookup_value(Ipv4Addr::from(ip | 1).into()),
            Err(Error::NotFound)
        ) {
            return Err("RSS fixture contains an expected miss".into());
        }
    }
    drop(reader);
    // Publish by rename: a previously mapped inode is never truncated.
    fn publish(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temporary = path.with_extension(format!("{nonce}.tmp"));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        drop(file);
        fs::rename(temporary, path)
    }
    publish(&dir.join("database.mmdb"), &bytes)?;
    publish(
        &dir.join("memory-queries.bin"),
        &protocol::workload(entries, protocol::LOOKUPS),
    )?;
    publish(
        &manifest_path,
        &serde_json::to_vec_pretty(&json!({
            "protocol": protocol::PROTOCOL, "entries": entries, "prefix": 32,
            "family": "ipv4", "seed": format!("{:#x}", protocol::SEED),
            "lookups": protocol::LOOKUPS, "warmup": protocol::WARMUP,
            "database_bytes": database_bytes, "hit_ratio": 0.5,
        }))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_routes_and_misses_survive_serialization_deterministically() {
        let dir = std::env::temp_dir().join(format!("mmdb-rss-fixture-{}", std::process::id()));
        let second = dir.join("independent");
        generate(&dir, 128).unwrap();
        generate(&second, 128).unwrap();
        for file in ["database.mmdb", "memory-queries.bin", "manifest.json"] {
            assert_eq!(
                fs::read(dir.join(file)).unwrap(),
                fs::read(second.join(file)).unwrap()
            );
        }
        let bytes = fs::read(dir.join("database.mmdb")).unwrap();
        let reader = Reader::from_bytes(&bytes).unwrap();
        let queries = protocol::workload(128, 1000);
        for (index, chunk) in queries.chunks_exact(4).enumerate() {
            let ip = Ipv4Addr::from(u32::from_be_bytes(chunk.try_into().unwrap()));
            match reader.lookup_value(ip.into()) {
                Ok(_) => assert_eq!(index % 2, 0),
                Err(Error::NotFound) => assert_eq!(index % 2, 1),
                Err(error) => panic!("Invalid fixture: {error}"),
            }
        }
        let routes: std::collections::HashSet<_> = (0..100_000).map(protocol::route).collect();
        assert_eq!(routes.len(), 100_000);
        assert!(routes.iter().all(|ip| ip & 1 == 0));
        fs::remove_dir_all(dir).unwrap();
    }
}
