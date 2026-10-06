//! Build, deep-merge and scan an in-memory synthetic MMDB with assertions.
//!
//! The reader borrows only the bytes produced by this example's writer.
//! Run without arguments; reader,writer,derive are required.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use anyhow::{Result, bail};
use libmaxminddb_rs::{MergeStrategy, MetadataBuilder, MmdbDecode, Reader, Writer};
use serde_json::json;

#[derive(MmdbDecode)]
struct Record<'a> {
    files: Vec<&'a str>,
    categories: Vec<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
struct Inventory<'a> {
    ranges: u64,
    files: BTreeSet<&'a str>,
    categories: BTreeSet<&'a str>,
}

fn inventory<'a>(reader: &'a Reader<'_>) -> libmaxminddb_rs::Result<Inventory<'a>> {
    let mut result = Inventory {
        ranges: 0,
        files: BTreeSet::new(),
        categories: BTreeSet::new(),
    };
    reader.visit_borrowed_records(|_network, record: Record<'_>| {
        // The sets retain borrowed strings. Arrays and BTreeSet nodes allocate,
        // but neither the MMDB strings nor its buffer are cloned.
        result.files.extend(record.files);
        result.categories.extend(record.categories);
        result.ranges += 1;
        Ok(())
    })?;
    Ok(result)
}

fn parallel_inventory<'a>(reader: &'a Reader<'_>) -> libmaxminddb_rs::Result<Inventory<'a>> {
    let result = Mutex::new(Inventory {
        ranges: 0,
        files: BTreeSet::new(),
        categories: BTreeSet::new(),
    });
    reader.visit_borrowed_records_parallel(|_network, record: Record<'_>| {
        // Fn + Sync can run on several workers. Decode outside the lock; keep
        // borrowed strings in the protected inventory. For heavier scans,
        // prefer worker-local aggregation to avoid one shared lock per record.
        let mut result = result
            .lock()
            .map_err(|_| libmaxminddb_rs::Error::InvalidDatabase("inventory lock poisoned"))?;
        result.files.extend(record.files);
        result.categories.extend(record.categories);
        result.ranges += 1;
        Ok(())
    })?;
    result
        .into_inner()
        .map_err(|_| libmaxminddb_rs::Error::InvalidDatabase("inventory lock poisoned"))
}

fn print_inventory(result: &Inventory<'_>) {
    println!("Network ranges: {}", result.ranges);
    println!("Unique files: {}", result.files.len());
    for file in &result.files {
        println!("  {file:?}");
    }
    println!("Unique categories: {}", result.categories.len());
    for category in &result.categories {
        println!("  {category:?}");
    }
}

fn run_demo() -> Result<()> {
    let metadata = MetadataBuilder::new()
        .database_type("Synthetic-FireHOL-Scan")
        .ip_version(6)
        .build_epoch(1_700_000_000)
        .build()?;
    let mut writer = Writer::with_metadata(metadata).merge_strategy(MergeStrategy::DeepMerge);
    let mut expected = BTreeMap::new();

    for (cidr, file, category) in [
        ("192.0.2.1/32", "malware.ipset", "malware"),
        ("198.51.100.42/32", "abuse.ipset", "abuse"),
        ("2001:db8::1/128", "malware.ipset", "malware"),
    ] {
        let network = cidr.parse()?;
        writer.insert(
            network,
            &json!({
                "files": ["base.ipset"],
                "categories": ["other"],
                "source": {"name": "base", "score": 1}
            }),
        )?;
        // DeepMerge applies to the same prefix: nested maps recurse, arrays
        // concatenate with duplicates intact, and the newer scalar wins.
        writer.insert(
            network,
            &json!({
                "files": [file],
                "categories": [category, "other"],
                "source": {"score": 9, "reviewed": true}
            }),
        )?;
        expected.insert(
            network,
            json!({
                "files": ["base.ipset", file],
                "categories": ["other", category, "other"],
                "source": {"name": "base", "score": 9, "reviewed": true}
            }),
        );
    }

    let bytes = writer.finish()?;
    let reader = Reader::from_bytes(&bytes)?;
    let mut seen = BTreeSet::new();

    reader.visit_records(|network, value| {
        assert!(seen.insert(network), "A stored network was visited twice");
        let actual = value.to_json();
        assert_eq!(expected.get(&network), Some(&actual));
        assert_eq!(reader.lookup_value(network.network())?.to_json(), actual);
        Ok(())
    })?;
    assert_eq!(seen, expected.keys().copied().collect());

    let result = inventory(&reader)?;
    assert_eq!(result.ranges, 3);
    assert_eq!(
        result.files,
        BTreeSet::from(["abuse.ipset", "base.ipset", "malware.ipset"])
    );
    assert_eq!(
        result.categories,
        BTreeSet::from(["abuse", "malware", "other"])
    );

    assert_eq!(parallel_inventory(&reader)?, result);
    // Force two workers even for this tiny fixture, to demonstrate the explicit
    // limit API. Each decoded record stays on the worker that produced it.
    let parallel_ranges = std::sync::atomic::AtomicU64::new(0);
    reader.visit_borrowed_records_parallel_with_workers(
        std::num::NonZeroUsize::new(2).unwrap(),
        |_network, _record: Record<'_>| {
            parallel_ranges.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        },
    )?;
    assert_eq!(parallel_ranges.into_inner(), result.ranges);
    println!("Synthetic DeepMerge database: sequential and parallel scan assertions passed.");
    print_inventory(&result);
    Ok(())
}

fn main() -> Result<()> {
    if std::env::args_os().nth(1).is_some() {
        bail!(
            "This example takes no arguments and scans only the in-memory database built by its writer"
        );
    }
    run_demo()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_deep_merge_demo_without_an_external_database() {
        run_demo().unwrap();
    }

    #[test]
    fn inventories_all_raw_fields_without_display_truncation_or_string_copies() {
        let metadata = MetadataBuilder::new().ip_version(6).build().unwrap();
        let mut writer = Writer::with_metadata(metadata);
        let files: Vec<_> = (0..65).map(|i| format!("source-{i}.ipset")).collect();
        let categories = ["malware", "abuse", "custom-category", "malware"];
        for network in ["192.0.2.0/24", "2001:db8::/32"] {
            writer
                .insert(
                    network.parse().unwrap(),
                    &serde_json::json!({"files": files, "categories": categories}),
                )
                .unwrap();
        }
        let bytes = writer.finish().unwrap();
        let reader = Reader::from_bytes(&bytes).unwrap();
        let result = inventory(&reader).unwrap();
        assert_eq!(parallel_inventory(&reader).unwrap(), result);
        assert_eq!(result.ranges, 2);
        assert_eq!(result.files.len(), 65);
        assert_eq!(
            result.categories,
            BTreeSet::from(["abuse", "custom-category", "malware"])
        );
        let source = reader.as_bytes();
        for text in result.files.iter().chain(&result.categories) {
            let offset = text.as_ptr() as usize - source.as_ptr() as usize;
            assert_eq!(&source[offset..offset + text.len()], text.as_bytes());
        }
    }

    #[test]
    fn rejects_wrong_schema_instead_of_reporting_an_empty_inventory() {
        let metadata = MetadataBuilder::new().ip_version(4).build().unwrap();
        let mut writer = Writer::with_metadata(metadata);
        writer
            .insert(
                "192.0.2.0/24".parse().unwrap(),
                &serde_json::json!({"files": 7, "categories": []}),
            )
            .unwrap();
        let bytes = writer.finish().unwrap();
        let reader = Reader::from_bytes(&bytes).unwrap();
        assert!(inventory(&reader).is_err());
        assert!(parallel_inventory(&reader).is_err());
    }
}
