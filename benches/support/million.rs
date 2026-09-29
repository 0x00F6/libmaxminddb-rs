//! Deterministic million-route fixtures, shared by the native bench and its tests.
use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

use libmaxminddb_rs::{Error, IpNetwork, MetadataBuilder, MmdbDecode, MmdbEncode, Reader, Writer};
use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub const ENTRIES: usize = 1_000_000;
pub const SEED: u64 = 0x5eed_2026_0928_cafe;
pub const LABEL: &str = "million-route-benchmark";
const PROTOCOL: &str = "native-million-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    pub const ALL: [Self; 2] = [Self::V4, Self::V6];

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "ipv4" => Ok(Self::V4),
            "ipv6" => Ok(Self::V6),
            _ => Err(format!("unknown family: {s}").into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::V4 => "ipv4",
            Self::V6 => "ipv6",
        }
    }

    pub fn version(self) -> u16 {
        match self {
            Self::V4 => 4,
            Self::V6 => 6,
        }
    }

    pub fn prefix(self) -> u8 {
        match self {
            Self::V4 => 24,
            Self::V6 => 64,
        }
    }

    fn address_from_word(self, word: u128) -> IpAddr {
        match self {
            Self::V4 => Ipv4Addr::from(word as u32).into(),
            Self::V6 => Ipv6Addr::from(word).into(),
        }
    }

    fn route(self, key: u64) -> IpAddr {
        self.address_from_word(match self {
            Self::V4 => (key as u128) << 8,
            Self::V6 => (key as u128) << 64,
        })
    }

    fn host(self, key: u64, rng: &mut Rng) -> IpAddr {
        self.address_from_word(match self {
            Self::V4 => ((key as u128) << 8) | ((rng.next() % 254 + 1) as u128),
            Self::V6 => ((key as u128) << 64) | (rng.next() | 1) as u128,
        })
    }
}

#[derive(Debug, PartialEq, MmdbDecode, MmdbEncode)]
pub struct Record<'a> {
    pub id: u32,
    pub label: &'a str,
}

/// SplitMix64; no dependency on the platform RNG or randomized collection order.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

pub fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut rng = Rng(seed);
    for i in (1..items.len()).rev() {
        let j = (rng.next() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub protocol: String,
    pub family: String,
    pub entries: usize,
    pub prefix: u8,
    pub seed: String,
    pub queries_per_kind: usize,
    pub database_bytes: u64,
}

pub struct Fixture {
    pub dir: PathBuf,
    pub family: Family,
    pub count: usize,
}

impl Fixture {
    pub fn new(root: &Path, family: Family, count: usize) -> Self {
        Self {
            dir: root.join(format!("{}-{count}", family.name())),
            family,
            count,
        }
    }

    pub fn database(&self) -> PathBuf {
        self.dir.join("database.mmdb")
    }

    pub fn manifest(&self) -> Result<Manifest> {
        Ok(serde_json::from_reader(File::open(
            self.dir.join("manifest.json"),
        )?)?)
    }

    pub fn cached(&self) -> bool {
        let Ok(m) = self.manifest() else { return false };
        m.protocol == PROTOCOL
            && m.family == self.family.name()
            && m.entries == self.count
            && m.queries_per_kind == self.count
            && m.prefix == self.family.prefix()
            && m.seed == format!("{SEED:#018x}")
            && fs::metadata(self.database()).is_ok_and(|f| f.len() == m.database_bytes)
            && ["hits", "misses"].iter().all(|kind| {
                fs::metadata(self.dir.join(format!("{kind}.bin")))
                    .is_ok_and(|f| f.len() == self.count as u64 * 16)
            })
    }

    /// Called only by the preparation subprocess, never by an RSS worker.
    pub fn prepare(&self) -> Result<()> {
        if self.cached() {
            return Ok(());
        }
        if self.count == 0 || self.count > ENTRIES {
            return Err("fixture count must be in 1..=1,000,000".into());
        }
        fs::create_dir_all(&self.dir)?;
        eprintln!(
            "Preparing {}: {} distinct /{} routes",
            self.family.name(),
            self.count,
            self.family.prefix()
        );
        let mut rng = Rng(SEED);
        let mut keys = BTreeSet::new();
        while keys.len() < self.count {
            let n = rng.next();
            keys.insert(match self.family {
                Family::V4 => n & 0xff_ffff,
                Family::V6 => n,
            });
        }
        let metadata = MetadataBuilder::new()
            .ip_version(self.family.version())
            .database_type(PROTOCOL)
            .build_epoch(1_700_000_000)
            .build()?;
        let mut writer = Writer::with_metadata(metadata);
        let mut hits = Vec::with_capacity(self.count);
        let mut hosts = Rng(SEED ^ 0x4849_5453);
        for (id, key) in keys.into_iter().enumerate() {
            writer.insert_encoded(
                IpNetwork::new(self.family.route(key), self.family.prefix())?,
                &Record {
                    id: id as u32,
                    label: LABEL,
                },
            )?;
            hits.push(self.family.host(key, &mut hosts));
        }
        // Unique IDs prevent adjacent records being coalesced into a larger CIDR.
        let bytes = writer.finish()?;
        let database_bytes = bytes.len() as u64;
        atomic_write(&self.database(), &bytes)?;
        let reader = Reader::from_vec(bytes)?;
        let misses = generate_misses(&reader, self.family, self.count)?;
        validate_queries(&reader, self.family, &hits, &misses)?;
        self.write_queries("hits", &hits)?;
        self.write_queries("misses", &misses)?;
        let manifest = Manifest {
            protocol: PROTOCOL.into(),
            family: self.family.name().into(),
            entries: self.count,
            prefix: self.family.prefix(),
            seed: format!("{SEED:#018x}"),
            queries_per_kind: self.count,
            database_bytes,
        };
        atomic_write(
            &self.dir.join("manifest.json"),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        eprintln!(
            "Verified {} hits and {} CIDR-safe misses ({} MiB)",
            self.count,
            self.count,
            database_bytes as f64 / 1048576.0
        );
        Ok(())
    }

    fn write_queries(&self, kind: &str, ips: &[IpAddr]) -> Result<()> {
        let path = self.dir.join(format!("{kind}.bin"));
        let mut temp = tempfile::NamedTempFile::new_in(&self.dir)?;
        let mut out = BufWriter::new(temp.as_file_mut());
        for ip in ips {
            let word = match ip {
                IpAddr::V4(ip) => u32::from(*ip) as u128,
                IpAddr::V6(ip) => u128::from(*ip),
            };
            out.write_all(&word.to_be_bytes())?;
        }
        out.flush()?;
        drop(out);
        temp.persist(path)?;
        Ok(())
    }

    pub fn queries(&self, kind: &str) -> Result<Vec<IpAddr>> {
        let file = File::open(self.dir.join(format!("{kind}.bin")))?;
        if file.metadata()?.len() != self.count as u64 * 16 {
            return Err("invalid workload length".into());
        }
        let mut input = BufReader::new(file);
        let mut ips = Vec::with_capacity(self.count);
        for _ in 0..self.count {
            let mut bytes = [0; 16];
            input.read_exact(&mut bytes)?;
            let word = u128::from_be_bytes(bytes);
            if self.family == Family::V4 && word > u32::MAX as u128 {
                return Err("invalid IPv4 query".into());
            }
            ips.push(self.family.address_from_word(word));
        }
        Ok(ips)
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    // Replacement uses rename, never truncation of an inode mapped by another run.
    // A uniquely created file also avoids collisions between concurrent preparations.
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().ok_or("missing fixture directory")?)?;
    temp.write_all(bytes)?;
    temp.persist(path)?;
    Ok(())
}

pub fn generate_misses(reader: &Reader, family: Family, count: usize) -> Result<Vec<IpAddr>> {
    let mut rng = Rng(SEED ^ 0x4d49_5353);
    let mut seen = HashSet::with_capacity(count);
    let mut misses = Vec::with_capacity(count);
    while misses.len() < count {
        let word = match family {
            Family::V4 => rng.next() as u32 as u128,
            Family::V6 => ((rng.next() as u128) << 64) | rng.next() as u128,
        };
        let ip = family.address_from_word(word);
        // Search the serialized tree: exact-address set membership is insufficient.
        match reader.lookup_value(ip) {
            Err(Error::NotFound) => {
                if seen.insert(ip) {
                    misses.push(ip);
                }
            }
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(misses)
}

pub fn validate_queries(
    reader: &Reader,
    family: Family,
    hits: &[IpAddr],
    misses: &[IpAddr],
) -> Result<()> {
    if hits.len() != misses.len() || !hits.windows(2).all(|w| w[0] < w[1]) {
        return Err("hits must be distinct, sorted, and have the same count as misses".into());
    }
    for (id, &ip) in hits.iter().enumerate() {
        let record: Record<'_> = reader.lookup_borrowed(ip)?;
        if record.id != id as u32
            || record.label != LABEL
            || reader.lookup_value_with_prefix(ip)?.1 != family.prefix()
        {
            return Err(format!("invalid hit {ip} / id {id}").into());
        }
    }
    let mut seen = HashSet::with_capacity(misses.len());
    for &ip in misses {
        if !seen.insert(ip) {
            return Err(format!("duplicate miss {ip}").into());
        }
        match reader.lookup_borrowed::<Record<'_>>(ip) {
            Err(Error::NotFound) => {}
            Ok(_) => return Err(format!("miss {ip} matches a serialized CIDR").into()),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
