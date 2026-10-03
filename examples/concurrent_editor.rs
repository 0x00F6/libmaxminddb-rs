//! Run with `cargo run --example concurrent_editor`.

use std::sync::{Arc, Barrier};

use libmaxminddb_rs::{Editor, MetadataBuilder, Reader, ReloadableReader, Value, ValueRef, Writer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    writer.insert_value("10.0.0.0/8".parse()?, Value::Uint32(0))?;
    let initial = Arc::new(Reader::from_vec(writer.finish()?)?);
    let database = ReloadableReader::new(Arc::clone(&initial));
    let barrier = Barrier::new(5);
    let ip = "10.1.2.3".parse()?;
    let network = "10.0.0.0/8".parse()?;

    std::thread::scope(|scope| {
        let mut readers = Vec::new();
        for _ in 0..4 {
            let database = &database;
            let barrier = &barrier;
            readers.push(scope.spawn(move || {
                barrier.wait();
                for _ in 0..10_000 {
                    // Keep the guard alive for the whole query/batch, including
                    // any values borrowing strings or bytes from its reader.
                    let pinned = database.load();
                    let first = pinned.lookup_value(ip).unwrap();
                    assert_eq!(first, pinned.lookup_value(ip).unwrap());
                    assert!(matches!(first, ValueRef::Uint32(0..=20)));
                }
            }));
        }
        barrier.wait();
        for generation in 1..=20 {
            // Share the current reader and PreparedTree, without copying bytes.
            let source = database.snapshot();
            let mut editor = Editor::from_reader(source);
            editor.update_value(network, Value::Uint32(generation))?;
            // Rebuild outside publication, then CAS against the source Arc.
            // Multiple writers should retry from a fresh snapshot on conflict.
            assert!(database.commit(editor)?);
        }
        for worker in readers {
            worker.join().expect("reader thread panicked");
        }
        Ok::<(), libmaxminddb_rs::Error>(())
    })?;

    assert_eq!(initial.lookup_value(ip)?, ValueRef::Uint32(0));
    assert_eq!(database.load().lookup_value(ip)?, ValueRef::Uint32(20));
    println!("Concurrent reads and edits completed; original snapshot is unchanged.");
    Ok(())
}
