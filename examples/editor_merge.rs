//! Run with `cargo run --example editor_merge`.

use std::sync::Arc;

use libmaxminddb_rs::{
    Editor, MergeStrategy, MetadataBuilder, MmdbEncode, Reader, ReloadableReader, ValueRef, Writer,
};

#[derive(MmdbEncode)]
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
    writer.insert_encoded(network, &Record { country: "FR", score: 1, tags: vec!["original"] })?;
    let original = Arc::new(Reader::from_vec(writer.finish()?)?);
    let database = ReloadableReader::new(Arc::clone(&original));
    println!("[before] {:?}", original.lookup_value(ip)?);

    let mut editor = Editor::from_reader(database.snapshot());
    let patch = Patch { score: 42, tags: vec!["updated"] };
    println!("[edit] Encoding a custom struct with MmdbEncode; no serde is required.");
    editor.update_value(network, &patch, MergeStrategy::DeepMerge)?;
    println!("[edit] DeepMerge keeps country, replaces score and appends tags.");
    assert!(database.commit(editor)?);

    let guard = database.load();
    let merged = guard.lookup_value(ip)?;
    assert_eq!(merged.get("country"), Some(&ValueRef::Utf8("FR")));
    assert_eq!(merged.get("score"), Some(&ValueRef::Uint32(42)));
    assert_eq!(merged.get("tags"), Some(&ValueRef::Array(vec![
        ValueRef::Utf8("original"), ValueRef::Utf8("updated"),
    ])));
    println!("[after] {merged:?}");
    println!("[snapshot] The original Reader is unchanged: {:?}", original.lookup_value(ip)?);
    Ok(())
}
