use std::hint::black_box;
use std::net::Ipv4Addr;
use std::sync::{Arc, OnceLock};

use criterion::{Criterion, criterion_group, criterion_main};
use libmaxminddb_rs::{
    Editor, IpNetwork, MergeStrategy, MetadataBuilder, Reader, ReloadableReader, Value, Writer,
};

const ROUTES: u32 = 1_000_000;
const EDITS: u32 = 1_000;

fn million_ip_database() -> &'static [u8] {
    static DB: OnceLock<Vec<u8>> = OnceLock::new();
    DB.get_or_init(|| {
        let metadata = MetadataBuilder::new()
            .database_type("editor-million-v1")
            .ip_version(4)
            .build()
            .unwrap();
        let shared = std::sync::Arc::new(Value::Uint32(1));
        let mut writer = Writer::with_metadata_and_capacity(metadata, 2_100_000);
        for i in 0..ROUTES {
            let address = Ipv4Addr::from(0x0a00_0000_u32.wrapping_add(i));
            let network = IpNetwork::new(address.into(), 32).unwrap();
            writer
                .insert_value_shared(network, std::sync::Arc::clone(&shared))
                .unwrap();
        }
        writer.finish().unwrap()
    })
}

fn bench_editor(c: &mut Criterion) {
    let source = million_ip_database();

    let reader = Arc::new(Reader::from_bytes(source).unwrap());
    let database = ReloadableReader::new(Arc::clone(&reader));
    let ip = Ipv4Addr::from(0x0a00_0001).into();
    assert_eq!(
        reader.lookup_value(ip).unwrap(),
        libmaxminddb_rs::ValueRef::Uint32(1)
    );
    c.bench_function("rcu/million_direct_lookup", |b| {
        b.iter(|| black_box(reader.lookup_value(black_box(ip)).unwrap()))
    });
    c.bench_function("rcu/million_guard_lookup", |b| {
        b.iter(|| {
            let guard = database.load();
            black_box(guard.lookup_value(black_box(ip)).unwrap());
        })
    });
    c.bench_function("rcu/million_snapshot_lookup", |b| {
        b.iter(|| {
            let snapshot = database.snapshot();
            black_box(snapshot.lookup_value(black_box(ip)).unwrap());
        })
    });
    c.bench_function("rcu/million_shared_editor_open", |b| {
        b.iter(|| black_box(database.editor()))
    });
    c.bench_function("rcu/million_publish_prepared", |b| {
        b.iter(|| black_box(database.replace(Arc::clone(&reader))))
    });

    c.bench_function("editor/million_ipv4_update_1000_rebuild", |b| {
        b.iter(|| {
            let mut editor = Editor::from_reader(Arc::clone(black_box(&reader)));
            for i in 0..EDITS {
                let address = Ipv4Addr::from(0x0a00_0000_u32 + i * 997);
                let network = IpNetwork::new(address.into(), 32).unwrap();
                editor.update_value(network, Value::Uint32(2), MergeStrategy::Replace).unwrap();
            }
            black_box(editor.finish().unwrap())
        })
    });

    c.bench_function("editor/million_ipv4_remove_1000_rebuild", |b| {
        b.iter(|| {
            let mut editor = Editor::from_reader(Arc::clone(black_box(&reader)));
            for i in 0..EDITS {
                let address = Ipv4Addr::from(0x0a00_0000_u32 + i * 997);
                let network = IpNetwork::new(address.into(), 32).unwrap();
                editor.remove(network).unwrap();
            }
            black_box(editor.finish().unwrap())
        })
    });
}

fn configured_criterion() -> Criterion {
    Criterion::default()
        .warm_up_time(std::time::Duration::from_secs(1))
        .measurement_time(std::time::Duration::from_secs(10))
        .sample_size(10)
}

criterion_group! {
    name = editor_benches;
    config = configured_criterion();
    targets = bench_editor
}
criterion_main!(editor_benches);
