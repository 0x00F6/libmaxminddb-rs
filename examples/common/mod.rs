//! Shared fixture for focused lookup examples.

use std::error::Error;
use std::path::Path;

use anyhow::Context;
use futures_util::StreamExt;
use libmaxminddb_rs::{MetadataBuilder, MmdbEncode, Writer};
use tokio::{fs, io::AsyncWriteExt};

/// Borrowed GeoLite2 Country record shape shared by the GeoLite example.
#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
#[allow(dead_code)]
pub struct GeoLite2Country<'a> {
    pub continent: Option<Continent<'a>>,
    pub country: Option<Country<'a>>,
    pub location: Option<Location<'a>>,
    pub registered_country: Option<Country<'a>>,
}

/// Borrowed GeoLite2 City record shape, reusing the shared geographic types.
#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
#[allow(dead_code)]
pub struct GeoLite2City<'a> {
    pub continent: Option<Continent<'a>>,
    pub country: Option<Country<'a>>,
    pub city: Option<City<'a>>,
    pub location: Option<Location<'a>>,
    pub postal: Option<Postal<'a>>,
    pub registered_country: Option<Country<'a>>,
    pub subdivisions: Option<Vec<Subdivision<'a>>>,
    pub traits: Option<Traits>,
}

/// Borrowed GeoLite2 ASN record shape.
#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
#[allow(dead_code)]
pub struct GeoLite2Asn<'a> {
    pub autonomous_system_number: u64,
    pub autonomous_system_organization: &'a str,
}

#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
#[allow(dead_code)]
pub struct Continent<'a> {
    pub code: Option<&'a str>,
    pub geoname_id: Option<u64>,
    pub names: Option<Names<'a>>,
}

#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
#[allow(dead_code)]
pub struct Country<'a> {
    pub geoname_id: Option<u64>,
    pub iso_code: Option<&'a str>,
    pub names: Option<Names<'a>>,
}

#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
#[allow(dead_code)]
pub struct City<'a> {
    pub geoname_id: Option<u64>,
    pub names: Option<Names<'a>>,
}

#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
#[allow(dead_code)]
pub struct Names<'a> {
    pub de: Option<&'a str>,
    pub en: Option<&'a str>,
    pub es: Option<&'a str>,
    pub fr: Option<&'a str>,
    pub ja: Option<&'a str>,
    #[serde(rename = "pt-BR")]
    pub pt_br: Option<&'a str>,
    pub ru: Option<&'a str>,
    #[serde(rename = "zh-CN")]
    pub zh_cn: Option<&'a str>,
}

#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
#[allow(dead_code)]
pub struct Location<'a> {
    pub accuracy_radius: Option<u64>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub time_zone: Option<&'a str>,
}

#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
#[allow(dead_code)]
pub struct Postal<'a> {
    pub code: Option<&'a str>,
}

#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
#[allow(dead_code)]
pub struct Subdivision<'a> {
    pub geoname_id: Option<u64>,
    pub iso_code: Option<&'a str>,
    pub names: Option<Names<'a>>,
}

#[derive(Debug, libmaxminddb_rs::MmdbDecode)]
#[allow(dead_code)]
pub struct Traits {
    pub is_anonymous_proxy: Option<bool>,
    pub is_satellite_provider: Option<bool>,
    pub is_anycast: Option<bool>,
}

#[derive(MmdbEncode)]
#[allow(dead_code)] // Only sample_bytes uses this record in the focused lookup examples.
struct SampleRecord<'a> {
    asn: u32,
    country: &'a str,
}

#[allow(dead_code)] // The GeoLite example imports only download_file from this module.
pub fn sample_bytes() -> Result<Vec<u8>, Box<dyn Error>> {
    let metadata = MetadataBuilder::new()
        .database_type("Lookup-Examples")
        .ip_version(6)
        .build()?;
    let mut writer = Writer::with_metadata(metadata);
    writer.insert_encoded(
        "198.51.100.0/24".parse()?,
        &SampleRecord {
            asn: 64513,
            country: "US",
        },
    )?;
    writer.insert_encoded(
        "2001:db8:1::/48".parse()?,
        &SampleRecord {
            asn: 64512,
            country: "FR",
        },
    )?;
    Ok(writer.finish()?)
}

/// Downloads a file in chunks and atomically moves it into place when complete.
///
/// This creates the destination directory as needed and avoids leaving a partial
/// database at the final path if the transfer is interrupted.
#[allow(dead_code)] // Shared by the GeoLite example; other examples only need sample_bytes.
pub async fn download_file(url: &str, path: impl AsRef<Path>) -> anyhow::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating database directory {}", parent.display()))?;
    }

    let temporary_path = path.with_extension("mmdb.download");
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("requesting GeoLite2 database from {url}"))?
        .error_for_status()
        .context("GeoLite2 database download returned an error status")?;
    let mut stream = response.bytes_stream();
    let mut file = fs::File::create(&temporary_path)
        .await
        .with_context(|| format!("creating temporary file {}", temporary_path.display()))?;

    while let Some(chunk) = stream.next().await {
        file.write_all(&chunk?).await?;
    }
    file.flush().await?;
    drop(file);

    fs::rename(&temporary_path, path)
        .await
        .with_context(|| format!("moving database into {}", path.display()))?;
    Ok(())
}
