//! # Compatibility Database Generator Example for libmaxminddb-rs
//!
//! This utility example builds a standard MaxMind DB file used to verify
//! cross-compatibility with other MMDB readers (such as the official C `libmaxminddb`,
//! Python `maxminddb`, and other language implementations).
//!
//! You can optionally provide an output path as the first CLI argument:
//! ```bash
//! cargo run --example generate_compat_db -- target/compat.mmdb
//! ```

use std::error::Error;
use std::path::PathBuf;

use libmaxminddb_rs::{MetadataBuilder, Writer};

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== libmaxminddb-rs Cross-Reader Compatibility Fixture Generator ===\n");

    let out_path: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/compat.mmdb".into())
        .into();

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Configure metadata compatible with all standard MMDB v2 readers
    let metadata = MetadataBuilder::new()
        .database_type("libmaxminddb-rs-compat")
        .ip_version(4)
        .language("en")
        .description("en", "Cross-reader compatibility fixture")
        .build()?;

    let mut writer = Writer::with_metadata(metadata);

    // Insert structured JSON record:
    writer.insert(
        "203.0.113.0/24".parse()?,
        &serde_json::json!({
            "country": {
                "iso_code": "FR",
                "names": {
                    "en": "France",
                    "fr": "France"
                }
            },
            "category": "compat",
            "accuracy_radius": 50
        }),
    )?;

    println!("Writing compatibility database to: {}", out_path.display());
    writer.write_to_file(&out_path)?;

    let size = std::fs::metadata(&out_path)?.len();
    println!(
        "Successfully generated compatibility MMDB (size: {} bytes).",
        size
    );

    Ok(())
}
