use anyhow::Result;
mod common;
use common::{GeoLite2City, download_file};
use libmaxminddb_rs::Reader;
use std::net::IpAddr;
use std::path::PathBuf;
use tokio::fs;

#[tokio::main]
async fn main() -> Result<()> {
    // Store the GeoLite2 City database under this package's target directory.
    let database_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("database")
        .join("GeoLite2-City.mmdb");

    // The shared downloader creates target/database and streams the file to disk.
    // Keep an existing database so later runs do not download it again.
    if !fs::try_exists(&database_file).await? {
        download_file(
            "https://github.com/P3TERX/GeoLite.mmdb/releases/download/2026.09.28/GeoLite2-City.mmdb",
            &database_file,
        )
        .await?;
    }

    // Open the MMDB reader, parse the address once, and borrow record strings.
    let reader = Reader::open(&database_file)?;
    let ip: IpAddr = "8.8.8.8".parse()?;
    let record: GeoLite2City<'_> = reader.lookup_borrowed(ip)?;

    println!("{record:#?}");
    Ok(())
}
