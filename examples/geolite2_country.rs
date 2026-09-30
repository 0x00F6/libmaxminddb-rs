use anyhow::Result;
mod common;
use common::{GeoLite2Country, download_file};
use libmaxminddb_rs::Reader;
use std::net::IpAddr;
use std::path::PathBuf;
use tokio::fs;

const GEOLITE_REPO_URL: &str =
    "https://github.com/P3TERX/GeoLite.mmdb/releases/latest/download/GeoLite2-Country.mmdb";

#[tokio::main]
async fn main() -> Result<()> {
    // Keep the downloaded MMDB in this example package's target directory.
    let database_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("database")
        .join("GeoLite2-Country.mmdb");

    // download_file creates target/database automatically when it is missing.
    // Reuse an existing database file to avoid downloading it on every run.
    if !fs::try_exists(&database_file).await? {
        download_file(GEOLITE_REPO_URL, &database_file).await?;
    }

    // Open the downloaded MaxMind DB with the library reader.
    let reader = Reader::open(&database_file)?;

    // Parse the query address once before looking it up.
    let ip: IpAddr = "8.8.8.8".parse()?;

    // Borrowed strings remain tied to the reader's MMDB bytes where possible.
    let record: GeoLite2Country<'_> = reader.lookup_borrowed(ip)?;
    println!("{record:#?}");

    Ok(())
}
