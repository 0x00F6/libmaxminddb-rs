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
    tags: Vec<&'a str>,
}

#[derive(MmdbEncode)]
struct Patch<'a> {
    score: u32,
    tags: Vec<&'a str>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let network = "198.51.100.0/24".parse()?;
    let ip = "198.51.100.7".parse()?;

    let metadata = MetadataBuilder::new()
        .ip_version(4)
        .database_type("editor-memory-example")
        .build()?;
    let mut writer = Writer::with_metadata(metadata);
    writer.insert_encoded(
        network,
        &Record {
            country: "FR",
            score: 1,
            tags: vec!["initial"],
        },
    )?;

    // Move the owned Reader directly into the container: no extra source Arc.
    let database = ReloadableReader::new(Reader::from_vec(writer.finish()?)?);

    {
        let guard = database.load();
        let record: Record<'_> = guard.lookup_borrowed(ip)?;
        assert_eq!(record.score, 1);
        println!("[before] {record:?}");
    } // The borrowed record and its guard are dropped here.

    let mut editor = Editor::from_reader(database.snapshot());
    // Weak observes destruction without keeping the Reader alive.
    let old_lifetime = Arc::downgrade(editor.source_reader());
    let patch = Patch {
        score: 42,
        tags: vec!["updated"],
    };
    editor.update_value(network, &patch, MergeStrategy::DeepMerge)?;

    println!("[edit] Changes are staged; the source is still unchanged.");
    println!("[commit] Rebuilding and atomically publishing the new database.");
    // With only one writer, no publication conflict is expected.
    assert!(database.commit(editor)?, "Unexpected publication conflict");

    // Commit consumed the editor. No old guard or snapshot is retained here.
    assert!(
        old_lifetime.upgrade().is_none(),
        "The old Reader is unexpectedly still alive"
    );
    println!("[memory] Old Reader destroyed; its owned buffers were released.");
    // Release the small allocation retained by the Weak observer as well.
    drop(old_lifetime);

    {
        let guard = database.load();
        let record: Record<'_> = guard.lookup_borrowed(ip)?;
        assert_eq!(record.country, "FR");
        assert_eq!(record.score, 42);
        assert_eq!(record.tags, vec!["initial", "updated"]);
        println!("[after] {record:?}");
    }

    println!("[done] Only the new database remains in the container.");
    Ok(())
}
