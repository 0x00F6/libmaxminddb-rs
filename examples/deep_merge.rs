//! Deep merge of two custom records inserted for the same network.
//!
//! `MergeStrategy::DeepMerge` recursively merges MMDB maps, concatenates arrays,
//! and replaces conflicting scalar values with the latest insertion. It acts on
//! serialized MMDB values, not directly on Rust structs. `Writer::insert` uses
//! serde, allowing both map types and a unit enum in this example.
//!
//! Run with `cargo run --example deep_merge`.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::net::IpAddr;

use libmaxminddb_rs::{MergeStrategy, MetadataBuilder, Reader, Writer};
use serde::Serialize;
use serde_json::{Value as JsonValue, json};

#[derive(Debug, Serialize)]
enum PolicyMode {
    Observe,
    Enforce,
}

#[derive(Debug, Serialize)]
struct Coordinates {
    #[serde(skip_serializing_if = "Option::is_none")]
    latitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    longitude: Option<f64>,
}

#[derive(Debug, Serialize)]
struct Location<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    city: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    coordinates: Option<Coordinates>,
}

#[derive(Debug, Serialize)]
struct Provider<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reliability: Option<u32>,
    tags: Vec<&'a str>,
}

#[derive(Debug, Serialize)]
struct NetworkIntelligence<'a> {
    source: &'a str,
    owner: String,
    asn: u32,
    observations: u64,
    risk_delta: i32,
    confidence: f64,
    is_proxy: bool,
    mode: PolicyMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_only: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_only: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    optional_note: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<Location<'a>>,
    tags: Vec<&'a str>,
    risk_by_feed: BTreeMap<String, i32>,
    labels: HashMap<String, String>,
    providers: BTreeMap<String, Provider<'a>>,
}

fn show(label: &str, value: &impl Serialize) -> Result<(), Box<dyn Error>> {
    let json: JsonValue = serde_json::to_value(value)?;
    println!("{label}:\n{}\n", serde_json::to_string_pretty(&json)?);
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let network = "203.0.113.0/24".parse()?;
    let ip: IpAddr = "203.0.113.42".parse()?;

    let base = NetworkIntelligence {
        source: "geo-feed",
        owner: "Example Networks".to_owned(),
        asn: 64500,
        observations: 100,
        risk_delta: -2,
        confidence: 0.75,
        is_proxy: false,
        mode: PolicyMode::Observe,
        base_only: Some("legacy-contract"),
        source_only: None,
        optional_note: Some("review pending"),
        location: Some(Location {
            country: Some("FR"),
            city: Some("Paris"),
            coordinates: Some(Coordinates {
                latitude: Some(48.8566),
                longitude: Some(2.3522),
            }),
        }),
        tags: vec!["residential", "shared"],
        risk_by_feed: BTreeMap::from([("geo".to_owned(), 1), ("fraud".to_owned(), 2)]),
        labels: HashMap::from([("region".to_owned(), "eu".to_owned())]),
        providers: BTreeMap::from([(
            "primary".to_owned(),
            Provider {
                name: Some("Paris ISP"),
                reliability: Some(90),
                tags: vec!["broadband"],
            },
        )]),
    };

    let source = NetworkIntelligence {
        source: "threat-feed",
        owner: "Security Partner".to_owned(),
        asn: 64501,
        observations: 125,
        risk_delta: 5,
        confidence: 0.95,
        is_proxy: true,
        mode: PolicyMode::Enforce,
        base_only: None,
        source_only: Some("new-contract".to_owned()),
        optional_note: Some("confirmed proxy"),
        location: Some(Location {
            country: None,
            city: Some("Lyon"),
            coordinates: Some(Coordinates {
                latitude: None,
                longitude: Some(4.8357),
            }),
        }),
        tags: vec!["vpn", "shared"],
        risk_by_feed: BTreeMap::from([("fraud".to_owned(), 9), ("bot".to_owned(), 4)]),
        labels: HashMap::from([
            ("region".to_owned(), "fr".to_owned()),
            ("priority".to_owned(), "high".to_owned()),
        ]),
        providers: BTreeMap::from([
            (
                "primary".to_owned(),
                Provider {
                    name: None,
                    reliability: Some(97),
                    tags: vec!["monitored"],
                },
            ),
            (
                "backup".to_owned(),
                Provider {
                    name: Some("Fallback ISP"),
                    reliability: Some(80),
                    tags: vec!["cellular"],
                },
            ),
        ]),
    };

    show("Initial/base record", &base)?;
    show("Source record inserted second", &source)?;

    let metadata = MetadataBuilder::new()
        .database_type("Enriched-Network-Intelligence")
        .ip_version(4)
        .build()?;
    let mut writer = Writer::with_metadata(metadata).merge_strategy(MergeStrategy::DeepMerge);
    writer.insert(network, &base)?;
    writer.insert(network, &source)?;

    let bytes = writer.finish()?;
    let reader = Reader::from_bytes(&bytes)?;
    let (value, prefix_len) = reader.lookup_value_with_prefix(ip)?;
    let merged = value.to_json();
    show(&format!("Merged result for {ip} (/{prefix_len})"), &merged)?;

    // Preserved from base: source `None` fields are omitted; they do not delete keys.
    assert_eq!(merged["base_only"], json!("legacy-contract"));
    assert_eq!(merged["location"]["country"], json!("FR"));
    assert_eq!(
        merged["location"]["coordinates"]["latitude"],
        json!(48.8566)
    );
    assert_eq!(merged["risk_by_feed"]["geo"], json!(1));
    assert_eq!(merged["providers"]["primary"]["name"], json!("Paris ISP"));

    // Added from source: optional fields, map keys, and a nested map entry.
    assert_eq!(merged["source_only"], json!("new-contract"));
    assert_eq!(merged["risk_by_feed"]["bot"], json!(4));
    assert_eq!(merged["labels"]["priority"], json!("high"));
    assert_eq!(merged["providers"]["backup"]["name"], json!("Fallback ISP"));

    // Replaced by deep merge: String/&str, numeric, bool, and unit enum values
    // are MMDB scalars. `Some` replaces an existing optional scalar.
    assert_eq!(merged["source"], json!("threat-feed"));
    assert_eq!(merged["owner"], json!("Security Partner"));
    assert_eq!(merged["asn"], json!(64501));
    assert_eq!(merged["observations"], json!(125));
    assert_eq!(merged["risk_delta"], json!(5));
    assert_eq!(merged["confidence"], json!(0.95));
    assert_eq!(merged["is_proxy"], json!(true));
    assert_eq!(merged["mode"], json!("Enforce"));
    assert_eq!(merged["optional_note"], json!("confirmed proxy"));

    // Recursively merged: struct fields and BTreeMap/HashMap entries all become
    // MMDB maps. Shared keys recurse; existing keys and new keys both survive.
    assert_eq!(merged["location"]["city"], json!("Lyon"));
    assert_eq!(
        merged["location"]["coordinates"]["longitude"],
        json!(4.8357)
    );
    assert_eq!(merged["risk_by_feed"]["fraud"], json!(9));
    assert_eq!(merged["labels"]["region"], json!("fr"));
    assert_eq!(merged["providers"]["primary"]["reliability"], json!(97));

    // Vec<T> becomes an MMDB array: deep merge concatenates it, preserving
    // order and duplicates. It does not replace, deduplicate, or merge elements.
    assert_eq!(
        merged["tags"],
        json!(["residential", "shared", "vpn", "shared"])
    );
    assert_eq!(
        merged["providers"]["primary"]["tags"],
        json!(["broadband", "monitored"])
    );

    println!("All deep-merge outcomes matched the expected values.");
    Ok(())
}
