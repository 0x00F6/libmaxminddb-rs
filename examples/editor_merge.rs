//! Run with `cargo run --example editor_merge`.

use std::sync::Arc;

use libmaxminddb_rs::{
    Editor, MergeStrategy, MetadataBuilder, MmdbDecode, MmdbEncode, Reader, ReloadableReader,
    Writer,
};

#[derive(Debug, PartialEq, MmdbDecode, MmdbEncode)]
struct Record<'a> {
    country: &'a str,
    score: u32,
    tags: Vec<&'a str>,
}

// No Serialize derive is needed: the editor uses MmdbEncode directly.
#[derive(MmdbEncode)]
struct Patch<'a> {
    score: u32,
    tags: Vec<&'a str>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let network = "198.51.100.0/24".parse()?;
    let ip = "198.51.100.7".parse()?;
    let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    writer.insert_encoded(
        network,
        &Record {
            country: "FR",
            score: 1,
            tags: vec!["original"],
        },
    )?;
    let original = Arc::new(Reader::from_vec(writer.finish()?)?);
    let database = ReloadableReader::new(Arc::clone(&original));
    let before: Record<'_> = original.lookup_borrowed(ip)?;
    println!("[before] Decoded custom Record: {before:?}");

    let mut editor = Editor::from_reader(database.snapshot());
    let patch = Patch {
        score: 42,
        tags: vec!["updated"],
    };
    println!("[edit] Encoding a custom struct with MmdbEncode; no serde is required.");
    editor.update_value(network, &patch, MergeStrategy::DeepMerge)?;
    println!("[edit] DeepMerge keeps country, replaces score and appends tags.");
    assert!(database.commit(editor)?);

    let guard = database.load();
    // The struct borrows its strings from this generation; keep guard alive.
    // Decoding the tags Vec allocates its container, while its strings borrow.
    let merged: Record<'_> = guard.lookup_borrowed(ip)?;
    assert_eq!(merged.country, "FR");
    assert_eq!(merged.score, 42);
    assert_eq!(merged.tags, vec!["original", "updated"]);
    println!("[after] {merged:?}");
    let unchanged: Record<'_> = original.lookup_borrowed(ip)?;
    assert_eq!(unchanged, before);
    println!("[snapshot] The original Reader still returns the same struct: {unchanged:?}");
    Ok(())
}
