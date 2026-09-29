//! Large-working-set benchmarks. Generation and RSS sampling run in separate execs.
#[path = "support/memory.rs"]
mod memory;
#[path = "support/million.rs"]
mod million;

use std::hint::black_box;
use std::io::Write;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use criterion::{Criterion, Throughput};
use libmaxminddb_rs::{Error, Reader};
use million::{ENTRIES, Family, Fixture, Record, Result, SEED, shuffle, validate_queries};

fn fixture_root() -> PathBuf {
    std::env::var_os("MMDB_MICRO_FIXTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/micro-benchmarks/fixtures")
        })
}

fn run_dir() -> PathBuf {
    std::env::var_os("MMDB_MICRO_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/micro-benchmarks/standalone")
        })
}

#[inline]
fn lookup(reader: &Reader, ip: IpAddr) -> bool {
    match reader.lookup_borrowed::<Record<'_>>(black_box(ip)) {
        Ok(record) => {
            black_box(record);
            true
        }
        Err(Error::NotFound) => false,
        Err(error) => panic!("unexpected lookup error: {error}"),
    }
}

fn bench_family(c: &mut Criterion, fixture: &Fixture) -> Result<()> {
    // Owned open, identical record schema and work for both families. Everything
    // below, except the b.iter closure, is outside the measured lookup interval.
    let reader = Reader::open(fixture.database())?;
    let hits = fixture.queries("hits")?;
    let misses = fixture.queries("misses")?;
    validate_queries(&reader, fixture.family, &hits, &misses)?;
    let mut random = hits.clone();
    shuffle(&mut random, SEED ^ 0x5241_4e44);
    let mut mixed = Vec::with_capacity(hits.len() + misses.len());
    mixed.extend_from_slice(&hits);
    mixed.extend_from_slice(&misses);
    shuffle(&mut mixed, SEED ^ 0x004d_4958_4544);

    let mut group = c.benchmark_group("reader_million");
    group.throughput(Throughput::Elements(1));
    for (name, queries) in [
        ("lookup_not_exists", misses.as_slice()),
        ("lookup_borrowed_hit_sequential", hits.as_slice()),
        ("lookup_borrowed_hit_random", random.as_slice()),
        ("lookup_borrowed_mixed_random", mixed.as_slice()),
    ] {
        // Warm every queried address, including lazy radix state, before Criterion.
        for &ip in queries {
            black_box(lookup(&reader, ip));
        }
        // Continue across samples: resetting would exercise only a small hot prefix.
        let mut index = 0;
        group.bench_function(format!("{name}_{}", fixture.family.name()), |b| {
            b.iter(|| {
                let ip = queries[index];
                index += 1;
                if index == queries.len() {
                    index = 0;
                }
                black_box(lookup(&reader, ip))
            });
        });
    }
    group.finish();
    Ok(())
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let root = fixture_root();
    if args.get(1).map(String::as_str) == Some("--prepare-child") {
        let family = Family::parse(args.get(2).ok_or("missing family")?)?;
        return Fixture::new(&root, family, ENTRIES).prepare();
    }
    if args.get(1).map(String::as_str) == Some("--memory-child") {
        let family = Family::parse(args.get(2).ok_or("missing family")?)?;
        let mode = args.get(3).ok_or("missing opening mode")?;
        let result = memory::measure(&Fixture::new(&root, family, ENTRIES), mode)?;
        println!("{}", serde_json::to_string(&result)?);
        return Ok(());
    }
    for family in Family::ALL {
        let status = Command::new(std::env::current_exe()?)
            .args(["--prepare-child", family.name()])
            .status()?;
        if !status.success() {
            return Err(format!("{} preparation failed: {status}", family.name()).into());
        }
    }
    let mut criterion = Criterion::default()
        .warm_up_time(Duration::from_millis(200))
        .measurement_time(Duration::from_secs(1))
        .sample_size(30)
        .configure_from_args();
    for family in Family::ALL {
        bench_family(&mut criterion, &Fixture::new(&root, family, ENTRIES))?;
    }
    criterion.final_summary();
    let mut memory_results = Vec::new();
    for family in Family::ALL {
        for mode in ["owned", "mmap"] {
            eprint!(
                "Measuring RSS in a fresh process: {} / {mode}",
                family.name()
            );
            std::io::stderr().flush()?;
            let result = Command::new(std::env::current_exe()?)
                .args(["--memory-child", family.name(), mode])
                .output()?;
            if !result.status.success() {
                eprintln!(" — failed");
                return Err(format!(
                    "RSS worker failed ({}): {}",
                    result.status,
                    String::from_utf8_lossy(&result.stderr)
                )
                .into());
            }
            let measurement: serde_json::Value = serde_json::from_slice(&result.stdout)?;
            let after_open = measurement["after_open"]["rss_bytes"]
                .as_u64()
                .ok_or("missing RSS after open")? as f64
                / 1_048_576.0;
            let peak = measurement["after_lookups"]["peak_rss_bytes"]
                .as_u64()
                .ok_or("missing peak RSS")? as f64
                / 1_048_576.0;
            eprintln!(" — after open: {after_open:.2} MiB; peak: {peak:.2} MiB");
            memory_results.push(measurement);
        }
    }
    std::fs::create_dir_all(run_dir())?;
    std::fs::write(
        run_dir().join("memory.json"),
        serde_json::to_vec_pretty(&memory_results)?,
    )?;
    println!("RSS results: {}", run_dir().join("memory.json").display());
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("million_reader: {error}");
        std::process::exit(1);
    }
}
