#![cfg(all(feature = "reader", feature = "writer"))]

use std::sync::{Arc, Barrier};

use libmaxminddb_rs::{MetadataBuilder, Reader, ReloadableReader, Value, ValueRef, Writer};

fn reader(version: u32) -> Reader<'static> {
    let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(6).build().unwrap());
    for network in ["10.0.0.0/8", "2001:db8::/32"] {
        writer
            .insert_value(network.parse().unwrap(), Value::Uint32(version))
            .unwrap();
    }
    Reader::from_vec(writer.finish().unwrap()).unwrap()
}

#[test]
fn reader_editor_and_container_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Reader<'static>>();
    assert_send_sync::<libmaxminddb_rs::Editor<'static>>();
    assert_send_sync::<ReloadableReader<'static>>();
}

#[test]
fn borrowed_value_survives_publication_during_query() {
    let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build().unwrap());
    writer
        .insert_value(
            "10.0.0.0/8".parse().unwrap(),
            Value::Utf8("original".into()),
        )
        .unwrap();
    let database = ReloadableReader::new(Reader::from_vec(writer.finish().unwrap()).unwrap());
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let pinned = database.load();
            let value = pinned.lookup_value("10.1.2.3".parse().unwrap()).unwrap();
            barrier.wait();
            // The writer publishes while this guard and borrowed string live.
            barrier.wait();
            assert_eq!(value, ValueRef::Utf8("original"));
        });
        barrier.wait();
        drop(database.replace(reader(2)));
        barrier.wait();
    });
    assert_eq!(version(&database.load(), "10.1.2.3"), 2);
}

fn version(reader: &Reader<'_>, ip: &str) -> u32 {
    match reader.lookup_value(ip.parse().unwrap()).unwrap() {
        ValueRef::Uint32(value) => value,
        value => panic!("unexpected value: {value:?}"),
    }
}

#[test]
fn snapshots_share_source_and_survive_replacement() {
    let initial = Arc::new(reader(1));
    let database = ReloadableReader::new(Arc::clone(&initial));
    let old = database.snapshot();
    let guard = database.load();
    let editor = database.editor();
    assert!(Arc::ptr_eq(&old, &initial));
    assert!(Arc::ptr_eq(editor.source_reader(), &initial));
    assert_eq!(old.as_bytes().as_ptr(), initial.as_bytes().as_ptr());
    let weak = Arc::downgrade(&initial);
    drop(initial);
    drop(database.replace(reader(2)));
    assert_eq!(version(&old, "10.1.2.3"), 1);
    assert_eq!(version(&guard, "2001:db8::1"), 1);
    assert_eq!(version(&database.load(), "10.1.2.3"), 2);
    drop(old);
    drop(guard);
    assert!(weak.upgrade().is_some());
    drop(editor);
    assert!(weak.upgrade().is_none());
}

#[test]
fn invalid_replacement_and_failed_reload_preserve_generation() {
    let database = ReloadableReader::new(reader(1));
    let old = database.snapshot();
    assert!(database.replace_from_vec(vec![0; 32]).is_err());
    let directory = tempfile::tempdir().unwrap();
    assert!(
        database
            .reload(directory.path().join("missing.mmdb"))
            .is_err()
    );
    assert!(Arc::ptr_eq(&old, &database.snapshot()));
    let path = directory.path().join("replacement.mmdb");
    std::fs::write(&path, reader(2).as_bytes()).unwrap();
    drop(database.reload(path).unwrap());
    assert_eq!(version(&database.load(), "2001:db8::1"), 2);
}

#[test]
fn stale_editor_cannot_overwrite_newer_edits() {
    let database = ReloadableReader::new(reader(1));
    let mut first = database.editor();
    let mut stale = database.editor();
    first
        .update_value("10.0.0.0/8".parse().unwrap(), Value::Uint32(2))
        .unwrap();
    stale
        .update_value("10.0.0.0/8".parse().unwrap(), Value::Uint32(3))
        .unwrap();
    assert!(database.commit(first).unwrap());
    assert!(!database.commit(stale).unwrap());
    assert_eq!(version(&database.load(), "10.1.2.3"), 2);
    let unrelated = ReloadableReader::new(reader(4)).editor();
    assert!(!database.commit(unrelated).unwrap());
}

#[test]
fn concurrent_publishers_have_exactly_one_winner() {
    let database = ReloadableReader::new(reader(1));
    let expected = database.snapshot();
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (2..=3)
            .map(|id| {
                let database = &database;
                let expected = &expected;
                let barrier = &barrier;
                scope.spawn(move || {
                    let replacement = reader(id);
                    barrier.wait();
                    database.compare_and_replace(expected, replacement)
                })
            })
            .collect();
        let winners = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>();
        assert_eq!(winners, 1);
    });
}

#[test]
fn concurrent_lookups_never_mix_generations() {
    let database = ReloadableReader::new(reader(1));
    let first = database.snapshot();
    let second = Arc::new(reader(2));
    let barrier = Barrier::new(5);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let database = &database;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..10_000 {
                    let pinned = database.load();
                    let ipv4 = version(&pinned, "10.1.2.3");
                    assert!(matches!(ipv4, 1 | 2));
                    assert_eq!(ipv4, version(&pinned, "2001:db8::1"));
                }
            });
        }
        barrier.wait();
        for i in 0..10_000 {
            drop(database.replace(Arc::clone(if i % 2 == 0 { &second } else { &first })));
        }
    });
}

#[test]
fn borrowed_sources_work_with_scoped_threads() {
    let bytes = reader(1).as_bytes().to_vec();
    let database = ReloadableReader::new(Reader::from_bytes(&bytes).unwrap());
    std::thread::scope(|scope| {
        scope.spawn(|| assert_eq!(version(&database.load(), "10.1.2.3"), 1));
        scope.spawn(|| {
            drop(database.replace(Reader::from_bytes(&bytes).unwrap()));
        });
    });
    assert_eq!(database.snapshot().as_bytes().as_ptr(), bytes.as_ptr());
}

#[test]
fn editor_commits_while_readers_hold_consistent_snapshots() {
    let database = ReloadableReader::new(reader(0));
    let barrier = Barrier::new(5);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let database = &database;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..10_000 {
                    let pinned = database.load();
                    let ipv4 = version(&pinned, "10.1.2.3");
                    assert!(ipv4 <= 20);
                    assert_eq!(ipv4, version(&pinned, "2001:db8::1"));
                }
            });
        }
        barrier.wait();
        for generation in 1..=20 {
            let mut editor = libmaxminddb_rs::Editor::from_reader(database.snapshot());
            for network in ["10.0.0.0/8", "2001:db8::/32"] {
                editor
                    .update_value(network.parse().unwrap(), Value::Uint32(generation))
                    .unwrap();
            }
            assert!(database.commit(editor).unwrap());
        }
    });
    assert_eq!(version(&database.load(), "10.1.2.3"), 20);
}
