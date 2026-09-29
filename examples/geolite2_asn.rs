use anyhow::Result;
mod common;
use common::{GeoLite2Asn, download_file};
use libmaxminddb_rs::Reader;
use std::net::IpAddr;
use std::path::PathBuf;
use tokio::fs;

#[tokio::main]
async fn main() -> Result<()> {
    // Use a database-specific filename so a previously downloaded Country or City
    // database cannot be mistaken for the ASN database.
    let database_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("database")
        .join("GeoLite2-ASN.mmdb");

    // Download once; the shared helper creates target/database and streams to disk.
    if !fs::try_exists(&database_file).await? {
        download_file(
            "https://github.com/P3TERX/GeoLite.mmdb/releases/download/2026.09.28/GeoLite2-ASN.mmdb",
            &database_file,
        )
        .await?;
    }

    let reader = Reader::open(&database_file)?;
    let ip: IpAddr = "8.8.8.8".parse()?;

    // The organization string borrows from the MMDB data for this lookup.
    let record: GeoLite2Asn<'_> = reader.lookup_borrowed(ip)?;
    println!("{record:#?}");
    Ok(())
}
