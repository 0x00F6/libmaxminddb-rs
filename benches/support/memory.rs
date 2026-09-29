//! Linux process RSS, not allocator traffic or the size of the MMDB file.
use std::fs;
use std::hint::black_box;
use std::mem::size_of;
use std::net::IpAddr;

use libmaxminddb_rs::{Error, Reader};
use serde::Serialize;

use super::million::{Fixture, LABEL, Record, Result};

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub rss_bytes: u64,
    pub peak_rss_bytes: u64,
    pub anonymous_rss_bytes: u64,
    pub file_rss_bytes: u64,
}

impl Snapshot {
    pub fn read() -> Result<Self> {
        if !cfg!(target_os = "linux") {
            return Err(
                "RSS scenarios require Linux /proc/self/status; no measurement available".into(),
            );
        }
        Self::parse(&fs::read_to_string("/proc/self/status")?)
    }

    pub fn parse(status: &str) -> Result<Self> {
        fn kib(status: &str, key: &str) -> Result<u64> {
            let line = status
                .lines()
                .find(|line| line.starts_with(key))
                .ok_or_else(|| format!("missing {key} in /proc/self/status"))?;
            let mut fields = line.split_whitespace().skip(1);
            let value = fields.next().ok_or("missing RSS value")?.parse::<u64>()?;
            if fields.next() != Some("kB") {
                return Err("unexpected RSS unit".into());
            }
            value.checked_mul(1024).ok_or_else(|| "RSS overflow".into())
        }
        Ok(Self {
            rss_bytes: kib(status, "VmRSS:")?,
            peak_rss_bytes: kib(status, "VmHWM:")?,
            anonymous_rss_bytes: kib(status, "RssAnon:")?,
            file_rss_bytes: kib(status, "RssFile:")?,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct Measurement {
    pub family: String,
    pub opening: String,
    pub pid: u32,
    pub entries: usize,
    pub lookups: usize,
    pub database_bytes: u64,
    pub auxiliary_capacity_bytes: usize,
    pub before_auxiliary: Snapshot,
    pub before_open: Snapshot,
    pub after_open: Snapshot,
    pub after_lookups: Snapshot,
}

/// This entry point never calls fixture preparation. Its caller execs a fresh process.
pub fn measure(fixture: &Fixture, mode: &str) -> Result<Measurement> {
    if !fixture.cached() {
        return Err("memory worker requires a prepared fixture".into());
    }
    let before_auxiliary = Snapshot::read()?;
    let hits = fixture.queries("hits")?;
    let misses = fixture.queries("misses")?;
    let auxiliary_capacity_bytes = (hits.capacity() + misses.capacity()) * size_of::<IpAddr>();
    let before_open = Snapshot::read()?;
    let reader = match mode {
        "owned" => Reader::open(fixture.database())?,
        "mmap" => {
            // SAFETY: fixtures are finished before workers start. This harness never
            // modifies/truncates published files (updates rename a new inode), and
            // owns the Reader until all borrowed records have been consumed.
            unsafe { Reader::open_mmap(fixture.database())? }
        }
        _ => return Err(format!("unknown opening mode: {mode}").into()),
    };
    let after_open = Snapshot::read()?;
    // Every route and every verified miss is touched once, without retaining results.
    // Query buffers were loaded before `before_open`; their storage is reported separately.
    for (id, (&hit, &miss)) in hits.iter().zip(&misses).enumerate() {
        let record: Record<'_> = reader.lookup_borrowed(black_box(hit))?;
        if record.id != id as u32 || record.label != LABEL {
            return Err("invalid memory-workload hit".into());
        }
        black_box(record);
        match reader.lookup_borrowed::<Record<'_>>(black_box(miss)) {
            Err(Error::NotFound) => {
                black_box(false);
            }
            Ok(_) => return Err("invalid memory-workload miss".into()),
            Err(error) => return Err(error.into()),
        }
    }
    let after_lookups = Snapshot::read()?;
    // Keep reader and auxiliary buffers alive through the final sample.
    black_box((&reader, &hits, &misses));
    Ok(Measurement {
        family: fixture.family.name().into(),
        opening: mode.into(),
        pid: std::process::id(),
        entries: fixture.count,
        lookups: hits.len() + misses.len(),
        database_bytes: fixture.manifest()?.database_bytes,
        auxiliary_capacity_bytes,
        before_auxiliary,
        before_open,
        after_open,
        after_lookups,
    })
}
