use libmaxminddb_rs::{MetadataBuilder, Value, Writer};
use std::collections::BTreeMap;
use std::env;
use std::path::Path;

fn m(pairs: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Map(BTreeMap::from_iter(
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)),
    ))
}

fn names_map(en: &str) -> Value {
    m([("en", Value::Utf8(en.to_string()))])
}

fn record() -> Value {
    m([
        (
            "continent",
            m([
                ("code", Value::Utf8("EU".into())),
                ("geoname_id", Value::Uint32(6255148)),
                ("names", names_map("Europe")),
            ]),
        ),
        (
            "country",
            m([
                ("geoname_id", Value::Uint32(2635167)),
                ("iso_code", Value::Utf8("GB".into())),
                ("names", names_map("United Kingdom")),
            ]),
        ),
        (
            "subdivisions",
            Value::Array(vec![m([
                ("geoname_id", Value::Uint32(6269131)),
                ("iso_code", Value::Utf8("ENG".into())),
                ("names", names_map("England")),
            ])]),
        ),
        (
            "city",
            m([
                ("geoname_id", Value::Uint32(2643743)),
                ("names", names_map("London")),
            ]),
        ),
        (
            "location",
            m([
                ("accuracy_radius", Value::Uint16(100)),
                ("latitude", Value::Double(51.5142)),
                ("longitude", Value::Double(-0.0931)),
                ("time_zone", Value::Utf8("Europe/London".into())),
            ]),
        ),
        (
            "registered_country",
            m([
                ("geoname_id", Value::Uint32(6252001)),
                ("iso_code", Value::Utf8("US".into())),
                ("names", names_map("United States")),
            ]),
        ),
    ])
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = env::args_os()
        .nth(1)
        .map(|p| Path::new(&p).to_path_buf())
        .unwrap_or_else(|| "target/bench-data/GeoIP2-City-Bench.mmdb".into());

    let metadata = MetadataBuilder::new()
        .database_type("GeoIP2-City")
        .ip_version(6)
        .description("en", "deterministic benchmark dataset")
        .build_epoch(1)
        .build()?;
    let mut writer = Writer::with_metadata(metadata);

    let record = record();

    for i in 0_u32..4096 {
        let a = (i >> 8) as u8;
        let b = i as u8;
        let network = format!("{a}.{b}.0.0/16").parse()?;
        writer.insert_value(network, record.clone())?;
    }

    // Dedicated stable IPv4 hot address used by every implementation.
    writer.insert_value("81.2.69.0/24".parse()?, record.clone())?;

    // Build a non-trivial IPv6 search tree as well. A single /32 would make every
    // IPv6 lookup follow the same first 32 bits and would hide cache/locality effects.
    for i in 0_u32..4096 {
        let network = format!("2001:db8:{i:x}::/48").parse()?;
        writer.insert_value(network, record.clone())?;
    }

    let bytes = writer.finish()?;

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, &bytes)?;

    let file_size = std::fs::metadata(&out)?.len();
    eprintln!("✅ Wrote {file_size} bytes to {}", out.display());
    Ok(())
}
