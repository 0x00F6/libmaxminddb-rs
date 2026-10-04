//! Run with `cargo run --example editor_merge`.

use std::sync::Arc;

use libmaxminddb_rs::{
    Editor, MergeStrategy, MetadataBuilder, MmdbDecode, MmdbEncode, Reader, ReloadableReader,
    Writer,
};

#[derive(Debug, MmdbEncode, MmdbDecode)]
struct Record<'a> {
    country: &'a str,
    score: u32,
}

#[derive(MmdbEncode)]
struct Patch {
    score: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let network = "198.51.100.0/24".parse()?;
    let ip = "198.51.100.7".parse()?;
    let metadata = MetadataBuilder::new().ip_version(4).build()?;
    let mut writer = Writer::with_metadata(metadata);
    writer.insert_encoded(
        network,
        &Record {
            country: "FR",
            score: 1,
        },
    )?;

    // Transfer ownership without retaining an extra Arc to the old database.
    let database = ReloadableReader::new(Reader::from_vec(writer.finish()?)?);
    {
        let guard = database.load();
        let record: Record<'_> = guard.lookup_borrowed(ip)?;
        println!("Before update: {record:?}");
    } // Borrowed fields and their guard are dropped before publication.

    let mut editor = Editor::from_reader(database.snapshot());
    // Weak observes destruction without keeping the old Reader alive.
    let old_lifetime = Arc::downgrade(editor.source_reader());
    editor.update_value(network, &Patch { score: 42 }, MergeStrategy::DeepMerge)?;

    // Rebuild, then publish atomically. A stale editor would return false.
    assert!(database.commit(editor)?, "Unexpected publication conflict");
    assert!(old_lifetime.upgrade().is_none());
    drop(old_lifetime);
    println!("Committed: the old Reader and its owned buffers were released.");

    let guard = database.load();
    let record: Record<'_> = guard.lookup_borrowed(ip)?;
    assert_eq!((record.country, record.score), ("FR", 42));
    println!("After DeepMerge: {record:?}");
    Ok(())
}
