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

    println!("[setup] Created generation 0 for {network}; lookup address: {ip}.");
    println!("[setup] The container and original snapshot share one immutable Reader.");
    println!("[setup] Four readers will run alongside one writer.");
    println!("[setup] Console messages from different threads may interleave.");

    std::thread::scope(|scope| {
        let mut readers = Vec::new();
        for reader_id in 0..4 {
            let database = &database;
            let barrier = &barrier;
            readers.push(scope.spawn(move || {
                println!("[reader {reader_id}] Ready; waiting at the start barrier.");
                barrier.wait();
                for _ in 0..10_000 {
                    // Keep the guard alive for the whole query/batch, including
                    // any values borrowing strings or bytes from its reader.
                    let pinned = database.load();
                    let first = pinned.lookup_value(ip).unwrap();
                    assert_eq!(first, pinned.lookup_value(ip).unwrap());
                    assert!(matches!(first, ValueRef::Uint32(0..=20)));
                }
                println!("[reader {reader_id}] Finished 10,000 consistent guarded queries.");
            }));
        }
        barrier.wait();
        println!("[writer] Starting edits while reader threads perform their queries.");
        for generation in 1..=20 {
            // Share the current reader and PreparedTree, without copying bytes.
            let source = database.snapshot();
            let mut editor = Editor::from_reader(source);
            editor.update_value(network, Value::Uint32(generation))?;
            println!("[writer] Staged generation {generation} in a private editor overlay.");
            println!("[writer] Source bytes and the prepared tree remain unchanged.");
            // Rebuild outside publication, then CAS against the source Arc.
            // Multiple writers should retry from a fresh snapshot on conflict.
            assert!(database.commit(editor)?);
            println!("[writer] Rebuilt, prepared and atomically published {generation}.");
            println!("[writer] Existing guards keep their version; new loads see this one.");
        }
        println!("[writer] All 20 generations published; waiting for reader threads.");
        for worker in readers {
            worker.join().expect("reader thread panicked");
        }
        Ok::<(), libmaxminddb_rs::Error>(())
    })?;

    assert_eq!(initial.lookup_value(ip)?, ValueRef::Uint32(0));
    assert_eq!(database.load().lookup_value(ip)?, ValueRef::Uint32(20));
    println!("[verify] The original snapshot still returns generation 0.");
    println!("[verify] The active reader now returns generation 20.");
    println!("[done] All concurrent reads and edits completed successfully.");
    Ok(())
}
