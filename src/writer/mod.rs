//! MaxMind DB writer.

use std::collections::HashMap;
use std::hash::{BuildHasher, BuildHasherDefault, Hasher};
use std::path::Path;

use crate::encoder::encode_value;
use crate::{Error, IpNetwork, Metadata, MmdbEncode, MmdbRecord, Result, Value};

const METADATA_MARKER: &[u8] = b"\xab\xcd\xefMaxMind.com";

/// Non-cryptographic hasher for the writer's internal maps.
///
/// Interning and dedup only need a fast, well-distributed hash: collisions are
/// always resolved by comparing the actual bytes or values, so the hash never
/// determines serialization order or correctness.
#[derive(Default)]
struct FxHasher {
    hash: u64,
}

type FxHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

const FX_SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add(&mut self, value: u64) {
        self.hash = (self.hash.rotate_left(5) ^ value).wrapping_mul(FX_SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, remainder) = bytes.as_chunks::<8>();
        for chunk in chunks {
            self.add(u64::from_ne_bytes(*chunk));
        }
        if !remainder.is_empty() {
            let mut tail = [0u8; 8];
            tail[..remainder.len()].copy_from_slice(remainder);
            self.add(u64::from_ne_bytes(tail));
        }
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u16(&mut self, value: u16) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }
}

/// Strategy used when inserting data into an already-populated network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergeStrategy {
    /// Replace the old value completely.
    #[default]
    Replace,
    /// Append arrays; replace non-arrays.
    Append,
    /// Append only array elements not already present; replace non-arrays.
    AppendUnique,
    /// Merge maps recursively, append arrays, and replace scalar conflicts.
    DeepMerge,
}

const NO_NODE: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq)]
struct TrieNode {
    children: [u32; 2],
    value: Option<std::sync::Arc<Value>>,
    index: Option<u32>,
}

impl Default for TrieNode {
    #[inline]
    fn default() -> Self {
        Self {
            children: [NO_NODE, NO_NODE],
            value: None,
            index: None,
        }
    }
}

impl TrieNode {
    #[inline]
    fn has_children(&self) -> bool {
        self.children[0] != NO_NODE || self.children[1] != NO_NODE
    }
}

/// In-memory MaxMind DB builder with arena-allocated trie nodes.
///
/// The writer uses an arena allocation strategy: all `TrieNode` instances are
/// stored contiguously in a `Vec<TrieNode>`, and children are referenced by index
/// instead of `Box` pointers. This reduces allocation overhead and improves cache
/// locality during tree construction and traversal.
///
/// Node indices use `u32`, limiting the trie to 4 billion nodes (far beyond any
/// practical MMDB database).
#[derive(Debug)]
pub struct Writer {
    /// Arena of all trie nodes, allocated contiguously for better cache locality.
    /// The root node is always at index 0.
    nodes: Vec<TrieNode>,
    /// Index of the root node (always 0).
    root: u32,
    metadata: Metadata,
    merge_strategy: MergeStrategy,
}

impl Writer {
    /// Creates a writer with explicit metadata.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MetadataBuilder, Writer};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    /// let bytes = writer.finish()?;
    /// assert!(!bytes.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn with_metadata(metadata: Metadata) -> Self {
        Self::with_metadata_and_capacity(metadata, 1024)
    }

    /// Creates a writer with explicit metadata and pre-allocated node capacity.
    /// Capacity reserves trie nodes; it does not limit the number of inserts.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MetadataBuilder, Writer};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let metadata = MetadataBuilder::new().ip_version(4).build()?;
    /// let writer = Writer::with_metadata_and_capacity(metadata, 1024);
    /// assert!(!writer.finish()?.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn with_metadata_and_capacity(metadata: Metadata, capacity: usize) -> Self {
        let mut nodes = Vec::with_capacity(capacity.max(1));
        nodes.push(TrieNode::default());
        Self {
            nodes,
            root: 0,
            metadata,
            merge_strategy: MergeStrategy::Replace,
        }
    }

    /// Configures duplicate-network merge behavior.
    /// See [`MergeStrategy`] for conflict behavior. The setting affects later
    /// inserts into the same network.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MergeStrategy, MetadataBuilder, Writer};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let writer = Writer::with_metadata(MetadataBuilder::new().build()?)
    ///     .merge_strategy(MergeStrategy::DeepMerge);
    /// assert!(!writer.finish()?.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn merge_strategy(mut self, strategy: MergeStrategy) -> Self {
        self.merge_strategy = strategy;
        self
    }

    /// Inserts a serde-serializable value for an IP network.
    /// Returns an encoding error for values the MMDB format cannot represent.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MetadataBuilder, Writer};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    /// writer.insert("198.51.100.0/24".parse()?, &serde_json::json!({"asn": 64512}))?;
    /// assert!(!writer.finish()?.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn insert<T: serde::Serialize + ?Sized>(
        &mut self,
        network: IpNetwork,
        value: &T,
    ) -> Result<()> {
        self.insert_value(network, Value::from_serialize(value)?)
    }

    /// Inserts a value produced by `#[derive(MmdbEncode)]`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "derive")]
    /// # mod example {
    /// use libmaxminddb_rs::{MetadataBuilder, MmdbEncode, Writer};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// #[derive(MmdbEncode)]
    /// struct Record<'a> { category: &'a str }
    /// let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    /// writer.insert_encoded("198.51.100.0/24".parse()?, &Record { category: "example" })?;
    /// assert!(!writer.finish()?.is_empty());
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    pub fn insert_encoded<T: MmdbEncode + ?Sized>(
        &mut self,
        network: IpNetwork,
        value: &T,
    ) -> Result<()> {
        self.insert_value(network, value.encode()?)
    }

    /// Inserts a custom record carrying its own `#[mmdb(network)]` field.
    ///
    /// This is the one-object insertion API for types deriving both `MmdbEncode` and
    /// `MmdbRecord`. The network field is not written into the record data.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "derive")]
    /// # mod example {
    /// use libmaxminddb_rs::{IpNetwork, MetadataBuilder, MmdbEncode, MmdbRecord, Writer};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// #[derive(MmdbEncode, MmdbRecord)]
    /// struct Record<'a> {
    ///     #[mmdb(network)] network: IpNetwork,
    ///     category: &'a str,
    /// }
    /// let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    /// writer.insert_entry(&Record {
    ///     network: "198.51.100.0/24".parse()?, category: "example",
    /// })?;
    /// assert!(!writer.finish()?.is_empty());
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    pub fn insert_entry<T: MmdbRecord + ?Sized>(&mut self, entry: &T) -> Result<()> {
        self.insert_value(entry.network(), entry.encode()?)
    }

    /// Inserts an already-typed MMDB value.
    /// Returns an error when the network family does not match metadata.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MetadataBuilder, Value, Writer};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    /// writer.insert_value("198.51.100.0/24".parse()?, Value::Utf8("example".into()))?;
    /// assert!(!writer.finish()?.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn insert_value(&mut self, network: IpNetwork, value: Value) -> Result<()> {
        let node_index = descend_network(
            &mut self.nodes,
            self.root,
            network,
            self.metadata.ip_version,
        )?;
        let strategy = self.merge_strategy;
        let node = &mut self.nodes[node_index as usize];
        node.value = Some(if strategy == MergeStrategy::Replace {
            std::sync::Arc::new(value)
        } else {
            match node.value.take() {
                Some(old) => std::sync::Arc::new(merge_values((*old).clone(), value, strategy)),
                None => std::sync::Arc::new(value),
            }
        });
        Ok(())
    }

    /// Inserts a shared MMDB value without copying the underlying structure.
    ///
    /// Alias for insert_value_shared.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MetadataBuilder, Value, Writer};
    /// use std::sync::Arc;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    /// let value = Arc::new(Value::Uint32(64512));
    /// writer.insert_value_arc("198.51.100.0/24".parse()?, value)?;
    /// assert!(!writer.finish()?.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn insert_value_arc(
        &mut self,
        network: IpNetwork,
        value: std::sync::Arc<Value>,
    ) -> Result<()> {
        self.insert_value_shared(network, value)
    }

    /// Inserts a shared MMDB value without copying the underlying structure.
    /// The writer retains the `Arc` on a first insert; merging may clone the
    /// underlying value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MetadataBuilder, Value, Writer};
    /// use std::sync::Arc;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    /// let value = Arc::new(Value::Uint32(64512));
    /// writer.insert_value_shared("198.51.100.0/24".parse()?, value)?;
    /// assert!(!writer.finish()?.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn insert_value_shared(
        &mut self,
        network: IpNetwork,
        value: std::sync::Arc<Value>,
    ) -> Result<()> {
        let node_index = descend_network(
            &mut self.nodes,
            self.root,
            network,
            self.metadata.ip_version,
        )?;
        let strategy = self.merge_strategy;
        let node = &mut self.nodes[node_index as usize];
        node.value = Some(if strategy == MergeStrategy::Replace {
            value
        } else {
            match node.value.take() {
                Some(old) => {
                    std::sync::Arc::new(merge_values((*old).clone(), (*value).clone(), strategy))
                }
                None => value,
            }
        });
        Ok(())
    }

    /// Inserts a batch of IP networks sharing the same value.
    /// A network-family mismatch aborts the batch with an error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MetadataBuilder, Value, Writer};
    /// use std::sync::Arc;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    /// let networks = ["198.51.100.0/24".parse()?, "203.0.113.0/24".parse()?];
    /// writer.insert_batch_shared(networks, &Arc::new(Value::Uint32(64512)))?;
    /// assert!(!writer.finish()?.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn insert_batch_shared<I>(
        &mut self,
        networks: I,
        value: &std::sync::Arc<Value>,
    ) -> Result<()>
    where
        I: IntoIterator<Item = IpNetwork>,
    {
        let iter = networks.into_iter();
        let (lower, _) = iter.size_hint();
        if lower > 0 {
            self.nodes.reserve(lower.saturating_mul(4).min(2_000_000));
        }
        for net in iter {
            self.insert_value_shared(net, std::sync::Arc::clone(value))?;
        }
        Ok(())
    }

    /// Inserts a batch of network/value pairs.
    /// The batch stops at the first invalid entry.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MetadataBuilder, Value, Writer};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    /// writer.insert_batch([("198.51.100.0/24".parse()?, Value::Bool(true))])?;
    /// assert!(!writer.finish()?.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn insert_batch<I>(&mut self, entries: I) -> Result<()>
    where
        I: IntoIterator<Item = (IpNetwork, Value)>,
    {
        for (net, val) in entries {
            self.insert_value(net, val)?;
        }
        Ok(())
    }

    /// Finalizes the database entirely in memory.
    /// Returns encoding errors for oversized trees or unrepresentable values.
    /// The returned vector owns the complete MMDB file.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MetadataBuilder, Writer};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    /// writer.insert("198.51.100.0/24".parse()?, &serde_json::json!({"asn": 64512}))?;
    /// let bytes = writer.finish()?;
    /// assert!(!bytes.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn finish(mut self) -> Result<Vec<u8>> {
        // Assign indices to nodes that will be in the search tree (nodes with children + root)
        let mut next = 0_u64;
        assign_indices(&mut self.nodes, self.root, &mut next);
        let node_count = next;

        if node_count > u64::from(u32::MAX) {
            return Err(Error::EncodingError(
                "MMDB node_count exceeds uint32".into(),
            ));
        }

        let mut pool = DataPool::default();
        let mut records: Vec<[u32; 2]> = vec![[0, 0]; node_count as usize];
        fill_records(
            &self.nodes,
            self.root,
            None,
            &mut None,
            &mut records,
            node_count,
            &mut pool,
        )?;
        // All pointers and data offsets are now resolved. Release the trie
        // before allocating the final MMDB buffer to avoid overlapping peaks.
        drop(std::mem::take(&mut self.nodes));

        let max_pointer = node_count
            .checked_add(16)
            .and_then(|v| v.checked_add(pool.data.len() as u64))
            .ok_or_else(|| Error::EncodingError("MMDB pointer overflow".into()))?;
        let record_size = if max_pointer < (1_u64 << 24) {
            24
        } else if max_pointer < (1_u64 << 28) {
            28
        } else if max_pointer <= u64::from(u32::MAX) {
            32
        } else {
            return Err(Error::EncodingError(
                "MMDB data section exceeds 32-bit pointer space".into(),
            ));
        };

        self.metadata.node_count = node_count;
        self.metadata.record_size = record_size;

        let mut out = Vec::with_capacity(
            records.len() * (record_size as usize / 4) + 16 + pool.data.len() + 256,
        );
        for [left, right] in records {
            encode_node(left as u64, right as u64, record_size, &mut out)?;
        }
        out.extend_from_slice(&[0_u8; 16]);
        out.extend_from_slice(&pool.data);
        out.extend_from_slice(METADATA_MARKER);
        encode_value(&self.metadata.to_value(), &mut out)?;
        Ok(out)
    }

    /// Finalizes and writes a database to disk.
    /// Propagates serialization and file I/O errors.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MetadataBuilder, Writer};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut writer = Writer::with_metadata(MetadataBuilder::new().ip_version(4).build()?);
    /// writer.insert("198.51.100.0/24".parse()?, &serde_json::json!({"asn": 64512}))?;
    /// let file = tempfile::NamedTempFile::new()?;
    /// writer.write_to_file(file.path())?;
    /// assert!(std::fs::metadata(file.path())?.len() > 0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn write_to_file(self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, self.finish()?)?;
        Ok(())
    }
}

#[derive(Default)]
struct DataPool {
    data: Vec<u8>,
    // Hashes index collision chains; the actual encoded bytes live only in data.
    offsets: FxHashMap<u64, usize>,
    entries: Vec<PoolEntry>,
    arc_offsets: FxHashMap<usize, usize>,
}

struct PoolEntry {
    offset: usize,
    len: usize,
    next: Option<usize>,
}

impl DataPool {
    fn intern(&mut self, value: &Value) -> Result<usize> {
        let start = self.data.len();
        if let Err(error) = encode_value(value, &mut self.data) {
            self.data.truncate(start);
            return Err(error);
        }
        let hash = self.offsets.hasher().hash_one(&self.data[start..]);
        Ok(self.intern_encoded(start, hash))
    }

    fn intern_arc(&mut self, arc: &std::sync::Arc<Value>) -> Result<usize> {
        let ptr = std::sync::Arc::as_ptr(arc) as usize;
        if let Some(&offset) = self.arc_offsets.get(&ptr) {
            return Ok(offset);
        }
        let offset = self.intern(arc.as_ref())?;
        self.arc_offsets.insert(ptr, offset);
        Ok(offset)
    }

    fn intern_encoded(&mut self, start: usize, hash: u64) -> usize {
        let head = self.offsets.entry(hash).or_insert(self.entries.len());
        let mut candidate = Some(*head);
        while let Some(index) = candidate {
            let Some(entry) = self.entries.get(index) else {
                break;
            };
            if self.data[entry.offset..entry.offset + entry.len] == self.data[start..] {
                let offset = entry.offset;
                // Reuse capacity for the next encoding, including duplicate values.
                self.data.truncate(start);
                return offset;
            }
            candidate = entry.next;
        }
        let next = (*head < self.entries.len()).then_some(*head);
        *head = self.entries.len();
        self.entries.push(PoolEntry {
            offset: start,
            len: self.data.len() - start,
            next,
        });
        start
    }
}

/// Assigns sequential indices to all nodes that have children (and root).
/// This is used to serialize the trie into the MMDB search tree format.
/// Recursively assigns sequential indices to nodes with children.
/// Called once during finish(); marked cold to keep out of icache.
#[cold]
fn assign_indices(nodes: &mut [TrieNode], node_index: u32, next: &mut u64) {
    if node_index == 0 || nodes[node_index as usize].has_children() {
        nodes[node_index as usize].index = Some(*next as u32);
        *next += 1;
        let c0 = nodes[node_index as usize].children[0];
        let c1 = nodes[node_index as usize].children[1];
        if c0 != NO_NODE {
            assign_indices(nodes, c0, next);
        }
        if c1 != NO_NODE {
            assign_indices(nodes, c1, next);
        }
    }
}

/// Fills the records array by traversing the trie and resolving targets.
/// Each node with children gets an entry in the records array at its assigned index.
/// Called once during finish(); marked cold to keep out of icache.
#[cold]
fn fill_records(
    nodes: &[TrieNode],
    node_index: u32,
    inherited: Option<&std::sync::Arc<Value>>,
    inherited_offset: &mut Option<usize>,
    records: &mut [[u32; 2]],
    node_count: u64,
    pool: &mut DataPool,
) -> Result<()> {
    let node = &nodes[node_index as usize];
    let index =
        node.index
            .ok_or_else(|| Error::EncodingError("indexed node expected".into()))? as usize;
    let mut own_offset = None;
    let (effective, cached_offset) = match node.value.as_ref() {
        Some(value) => (Some(value), &mut own_offset),
        None => (inherited, inherited_offset),
    };
    let mut targets = [node_count as u32, node_count as u32];
    let c0 = node.children[0];
    let c1 = node.children[1];
    for (side, child_idx) in [(0, c0), (1, c1)] {
        targets[side] = if child_idx != NO_NODE {
            let child = &nodes[child_idx as usize];
            if child.has_children() {
                fill_records(
                    nodes,
                    child_idx,
                    effective,
                    cached_offset,
                    records,
                    node_count,
                    pool,
                )?;
                child
                    .index
                    .ok_or_else(|| Error::EncodingError("child index missing".into()))?
            } else {
                // Leaf node with a value
                match child.value.as_ref() {
                    Some(value) => {
                        let offset = pool.intern_arc(value)?;
                        resolve_data_pointer(offset, node_count)?
                    }
                    None => match effective {
                        Some(value) => {
                            let offset = match *cached_offset {
                                Some(offset) => offset,
                                None => {
                                    let offset = pool.intern_arc(value)?;
                                    *cached_offset = Some(offset);
                                    offset
                                }
                            };
                            resolve_data_pointer(offset, node_count)?
                        }
                        None => node_count as u32,
                    },
                }
            }
        } else {
            match effective {
                Some(value) => {
                    let offset = match *cached_offset {
                        Some(offset) => offset,
                        None => {
                            let offset = pool.intern_arc(value)?;
                            *cached_offset = Some(offset);
                            offset
                        }
                    };
                    resolve_data_pointer(offset, node_count)?
                }
                None => node_count as u32,
            }
        };
    }
    records[index] = targets;
    Ok(())
}

#[inline(always)]
fn resolve_data_pointer(offset: usize, node_count: u64) -> Result<u32> {
    let ptr = node_count
        .checked_add(16)
        .and_then(|v| v.checked_add(offset as u64))
        .ok_or_else(|| Error::EncodingError("MMDB data pointer overflow".into()))?;
    if ptr > u64::from(u32::MAX) {
        return Err(Error::EncodingError(
            "MMDB data section exceeds 32-bit pointer space".into(),
        ));
    }
    Ok(ptr as u32)
}

fn encode_node(left: u64, right: u64, record_size: u16, out: &mut Vec<u8>) -> Result<()> {
    match record_size {
        24 => {
            if left >= 1 << 24 || right >= 1 << 24 {
                return Err(Error::EncodingError("24-bit tree pointer overflow".into()));
            }
            let bytes = [
                (left >> 16) as u8,
                (left >> 8) as u8,
                left as u8,
                (right >> 16) as u8,
                (right >> 8) as u8,
                right as u8,
            ];
            out.extend_from_slice(&bytes);
        }
        28 => {
            if left >= 1 << 28 || right >= 1 << 28 {
                return Err(Error::EncodingError("28-bit tree pointer overflow".into()));
            }
            let bytes = [
                (left >> 16) as u8,
                (left >> 8) as u8,
                left as u8,
                (((left >> 24) & 0x0f) << 4 | ((right >> 24) & 0x0f)) as u8,
                (right >> 16) as u8,
                (right >> 8) as u8,
                right as u8,
            ];
            out.extend_from_slice(&bytes);
        }
        32 => {
            out.extend_from_slice(&(left as u32).to_be_bytes());
            out.extend_from_slice(&(right as u32).to_be_bytes());
        }
        _ => {
            return Err(Error::EncodingError(
                "writer supports 24/28/32-bit records".into(),
            ));
        }
    }
    Ok(())
}

/// Descends the trie for a given network, creating nodes in the arena as needed.
/// Returns the index of the final node for the network prefix.
#[inline(never)]
fn descend_network(
    nodes: &mut Vec<TrieNode>,
    root: u32,
    network: IpNetwork,
    db_ip_version: u16,
) -> Result<u32> {
    match (db_ip_version, network) {
        (4, IpNetwork::V4(net)) => {
            let ip = u32::from(net.network());
            Ok(descend_prefix_u32(
                nodes,
                root,
                ip,
                usize::from(net.prefix_len()),
            ))
        }
        (4, IpNetwork::V6(_)) => Err(Error::InvalidIpVersion(6)),
        (6, IpNetwork::V6(net)) => {
            let ip = u128::from(net.network());
            Ok(descend_prefix_u128(
                nodes,
                root,
                ip,
                usize::from(net.prefix_len()),
            ))
        }
        (6, IpNetwork::V4(net)) => {
            // Descend 96 bits to reach the IPv4 subtree in an IPv6 database
            let mut node_index = root;
            for _ in 0..96 {
                let bit = 0; // Always follow left child (0) for IPv4 in IPv6
                let child = nodes[node_index as usize].children[bit];
                node_index = if child != NO_NODE {
                    child
                } else {
                    let new_idx = nodes.len() as u32;
                    nodes.push(TrieNode::default());
                    nodes[node_index as usize].children[bit] = new_idx;
                    new_idx
                };
            }
            let ip = u32::from(net.network());
            Ok(descend_prefix_u32(
                nodes,
                node_index,
                ip,
                usize::from(net.prefix_len()),
            ))
        }
        (version, _) => Err(Error::InvalidIpVersion(version)),
    }
}

#[inline(always)]
fn descend_prefix_u32(
    nodes: &mut Vec<TrieNode>,
    mut node_index: u32,
    ip: u32,
    prefix: usize,
) -> u32 {
    for shift in (32 - prefix..32).rev() {
        let bit = ((ip >> shift) & 1) as usize;
        let child = nodes[node_index as usize].children[bit];
        node_index = if child != NO_NODE {
            child
        } else {
            let new_idx = nodes.len() as u32;
            nodes.push(TrieNode::default());
            nodes[node_index as usize].children[bit] = new_idx;
            new_idx
        };
    }
    node_index
}

#[inline(always)]
fn descend_prefix_u128(
    nodes: &mut Vec<TrieNode>,
    mut node_index: u32,
    ip: u128,
    prefix: usize,
) -> u32 {
    for shift in (128 - prefix..128).rev() {
        let bit = ((ip >> shift) & 1) as usize;
        let child = nodes[node_index as usize].children[bit];
        node_index = if child != NO_NODE {
            child
        } else {
            let new_idx = nodes.len() as u32;
            nodes.push(TrieNode::default());
            nodes[node_index as usize].children[bit] = new_idx;
            new_idx
        };
    }
    node_index
}

/// Appends every value of `right` that is not already present in `left`.
///
/// Small inputs use a direct scan; larger inputs index `left` by hash so the
/// merge stays near-linear. Hash hits are always confirmed with `Value` equality,
/// preserving float and variant semantics exactly.
fn append_unique(left: &mut Vec<Value>, right: Vec<Value>) {
    const SCAN_LIMIT: usize = 64;
    // For very small inputs, a direct scan is faster than building a hash map
    if left.len().saturating_mul(right.len()) <= SCAN_LIMIT {
        for value in right {
            if !left.contains(&value) {
                left.push(value);
            }
        }
        return;
    }

    // For larger inputs, build a hash index for O(1) lookups
    // Reserve capacity for both left and right to avoid rehashing
    let mut seen: FxHashMap<u64, Vec<usize>> = FxHashMap::default();
    seen.reserve(left.len() + right.len());

    // Index all existing values in left
    for (index, value) in left.iter().enumerate() {
        seen.entry(hash_value(value)).or_default().push(index);
    }

    // Check each right value against the index
    for value in right {
        let hash = hash_value(&value);
        // Check if any value with this hash in left actually equals our value
        let duplicate = seen
            .get(&hash)
            .is_some_and(|indices| indices.iter().any(|&index| left[index] == value));
        if duplicate {
            continue;
        }
        // Add to index and push to left
        seen.entry(hash).or_default().push(left.len());
        left.push(value);
    }
}

fn hash_value(value: &Value) -> u64 {
    let mut hasher = FxHasher::default();
    hash_value_into(value, &mut hasher);
    hasher.finish()
}

fn hash_value_into<H: Hasher>(value: &Value, state: &mut H) {
    match value {
        Value::Utf8(v) => {
            state.write_u8(0);
            state.write(v.as_bytes());
        }
        Value::Bytes(v) => {
            state.write_u8(1);
            state.write(v);
        }
        Value::Double(v) => {
            state.write_u8(2);
            state.write_u64(v.to_bits());
        }
        Value::Float(v) => {
            state.write_u8(3);
            state.write_u32(v.to_bits());
        }
        Value::Uint16(v) => {
            state.write_u8(4);
            state.write_u16(*v);
        }
        Value::Uint32(v) => {
            state.write_u8(5);
            state.write_u32(*v);
        }
        Value::Int32(v) => {
            state.write_u8(6);
            state.write_i32(*v);
        }
        Value::Uint64(v) => {
            state.write_u8(7);
            state.write_u64(*v);
        }
        Value::Uint128(v) => {
            state.write_u8(8);
            state.write_u128(*v);
        }
        Value::Bool(v) => {
            state.write_u8(9);
            state.write_u8(u8::from(*v));
        }
        Value::Array(values) => {
            state.write_u8(10);
            state.write_usize(values.len());
            for value in values {
                hash_value_into(value, state);
            }
        }
        Value::Map(entries) => {
            state.write_u8(11);
            state.write_usize(entries.len());
            for (key, value) in entries {
                state.write(key.as_bytes());
                hash_value_into(value, state);
            }
        }
    }
}

fn merge_values(old: Value, new: Value, strategy: MergeStrategy) -> Value {
    match strategy {
        MergeStrategy::Replace => new,
        MergeStrategy::Append => match (old, new) {
            (Value::Array(mut left), Value::Array(right)) => {
                left.extend(right);
                Value::Array(left)
            }
            (_, new) => new,
        },
        MergeStrategy::AppendUnique => match (old, new) {
            (Value::Array(mut left), Value::Array(right)) => {
                append_unique(&mut left, right);
                Value::Array(left)
            }
            (_, new) => new,
        },
        MergeStrategy::DeepMerge => match (old, new) {
            (Value::Map(mut left), Value::Map(right)) => {
                for (key, value) in right {
                    match left.remove(&key) {
                        Some(previous) => {
                            left.insert(
                                key,
                                merge_values(previous, value, MergeStrategy::DeepMerge),
                            );
                        }
                        None => {
                            left.insert(key, value);
                        }
                    }
                }
                Value::Map(left)
            }
            (Value::Array(mut left), Value::Array(right)) => {
                left.extend(right);
                Value::Array(left)
            }
            (_, new) => new,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn replace_duplicate_reuses_the_new_shared_value() {
        let metadata = crate::MetadataBuilder::new().ip_version(4).build().unwrap();
        let mut writer = Writer::with_metadata(metadata);
        let network = "198.51.100.0/24".parse().unwrap();
        let old = std::sync::Arc::new(Value::Utf8("old".into()));
        let new = std::sync::Arc::new(Value::Utf8("new".into()));

        writer.insert_value_shared(network, old.clone()).unwrap();
        writer.insert_value_shared(network, new.clone()).unwrap();

        assert_eq!(std::sync::Arc::strong_count(&old), 1);
        assert!(
            writer
                .nodes
                .iter()
                .filter_map(|node| node.value.as_ref())
                .any(|stored| std::sync::Arc::ptr_eq(stored, &new))
        );
        assert!(!writer.finish().unwrap().is_empty());
    }

    #[test]
    fn inherited_cache_preserves_first_use_order_and_tree_records() {
        fn reference(
            nodes: &[TrieNode],
            node_index: u32,
            inherited: Option<&std::sync::Arc<Value>>,
            records: &mut [[u32; 2]],
            node_count: u64,
            pool: &mut DataPool,
        ) {
            let node = &nodes[node_index as usize];
            let effective = node.value.as_ref().or(inherited);
            let mut targets = [node_count as u32, node_count as u32];
            let child_indices = [(0, node.children[0]), (1, node.children[1])];
            for (side, child_idx) in child_indices {
                targets[side] = if child_idx != NO_NODE {
                    let child = &nodes[child_idx as usize];
                    if child.has_children() {
                        reference(nodes, child_idx, effective, records, node_count, pool);
                        child.index.unwrap()
                    } else {
                        match child.value.as_ref().or(effective) {
                            Some(value) => {
                                let offset = pool.intern_arc(value).unwrap();
                                (node_count + 16 + offset as u64) as u32
                            }
                            None => node_count as u32,
                        }
                    }
                } else {
                    match effective {
                        Some(value) => {
                            let offset = pool.intern_arc(value).unwrap();
                            (node_count + 16 + offset as u64) as u32
                        }
                        None => node_count as u32,
                    }
                };
            }
            records[node.index.unwrap() as usize] = targets;
        }
        let mut writer = Writer::with_metadata(crate::MetadataBuilder::new().build().unwrap());
        for (network, text) in [
            ("::/0", "root"),
            ("2001:db8::/32", "v6"),
            ("2001:db8:1::/48", "leaf"),
            ("10.0.0.0/8", "parent"),
            ("10.1.0.0/16", "child"),
            ("10.1.2.0/24", "leaf"),
            ("10.2.3.4/32", "leaf"),
            ("10.2.3.5/32", "parent"),
        ] {
            writer
                .insert_value(network.parse().unwrap(), Value::Utf8(text.into()))
                .unwrap();
        }
        let mut count = 0;
        assign_indices(&mut writer.nodes, writer.root, &mut count);
        let mut expected = vec![[0, 0]; count as usize];
        let mut actual = expected.clone();
        let mut old_pool = DataPool::default();
        let mut new_pool = DataPool::default();
        reference(
            &writer.nodes,
            writer.root,
            None,
            &mut expected,
            count,
            &mut old_pool,
        );
        fill_records(
            &writer.nodes,
            writer.root,
            None,
            &mut None,
            &mut actual,
            count,
            &mut new_pool,
        )
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(new_pool.data, old_pool.data);
    }

    #[test]
    fn pool_compares_bytes_even_when_hashes_collide() {
        let mut pool = DataPool::default();
        for (text, expected) in [("first", 0), ("second", 6), ("first", 0), ("second", 6)] {
            let start = pool.data.len();
            encode_value(&Value::Utf8(text.into()), &mut pool.data).unwrap();
            assert_eq!(pool.intern_encoded(start, 42), expected);
        }
        assert_eq!(pool.entries.len(), 2);
        assert_eq!(pool.data.len(), 13);
    }

    #[test]
    fn pool_reuses_duplicates_and_preserves_distinct_integer_types() {
        let mut pool = DataPool::default();
        let a = pool.intern(&Value::Uint16(42)).unwrap();
        let b = pool.intern(&Value::Uint32(42)).unwrap();
        assert_ne!(a, b);
        assert_eq!(pool.intern(&Value::Uint16(42)).unwrap(), a);
        assert_eq!(pool.entries.len(), 2);
    }

    #[test]
    fn packs_28_bit_nodes_per_mmdb_layout() {
        let mut out = Vec::new();
        encode_node(0x0123_4567, 0x0abc_def0, 28, &mut out).unwrap();
        assert_eq!(out, vec![0x23, 0x45, 0x67, 0x1a, 0xbc, 0xde, 0xf0]);
    }

    #[test]
    fn append_unique_matches_scan_for_small_and_large_inputs() {
        fn merge(left: Vec<Value>, right: Vec<Value>) -> Vec<Value> {
            let Value::Array(merged) = merge_values(
                Value::Array(left),
                Value::Array(right),
                MergeStrategy::AppendUnique,
            ) else {
                panic!()
            };
            merged
        }

        let small_left = vec![Value::Uint64(1), Value::Utf8("a".into())];
        let small_right = vec![Value::Uint64(1), Value::Uint64(2), Value::Utf8("a".into())];
        assert_eq!(
            merge(small_left.clone(), small_right.clone()),
            vec![Value::Uint64(1), Value::Utf8("a".into()), Value::Uint64(2)]
        );

        let large_left: Vec<Value> = (0..50).map(Value::Uint64).collect();
        let mut large_right: Vec<Value> = (25..75).map(Value::Uint64).collect();
        large_right.push(Value::Utf8("new".into()));
        let merged = merge(large_left, large_right);
        assert_eq!(merged.len(), 76);
        assert_eq!(merged[49], Value::Uint64(49));
        assert_eq!(merged[50], Value::Uint64(50));
        assert_eq!(merged[75], Value::Utf8("new".into()));
    }

    #[test]
    fn value_hash_distinguishes_variants_and_float_bits() {
        assert_ne!(hash_value(&Value::Uint16(1)), hash_value(&Value::Uint32(1)));
        assert_ne!(hash_value(&Value::Int32(-0)), hash_value(&Value::Uint64(0)));
        assert_ne!(
            hash_value(&Value::Double(0.0)),
            hash_value(&Value::Double(-0.0))
        );
        assert_eq!(
            hash_value(&Value::Double(0.0)),
            hash_value(&Value::Double(0.0))
        );
        // Every variant participates in structural array deduplication.
        for value in [
            Value::Bytes(vec![0, 255]),
            Value::Float(1.25),
            Value::Uint128(1 << 100),
            Value::Bool(true),
            Value::Array(vec![Value::Uint32(1)]),
            Value::Map(BTreeMap::from([("k".into(), Value::Uint16(1))])),
        ] {
            assert_eq!(hash_value(&value), hash_value(&value));
        }
    }

    #[test]
    fn inherited_value_fills_empty_leaf_and_missing_sibling_once() {
        use std::sync::Arc;

        let mut nodes = vec![TrieNode::default(), TrieNode::default()];
        nodes[0].index = Some(0);
        nodes[0].children[0] = 1;
        nodes[0].value = Some(Arc::new(Value::Utf8("parent".into())));
        let mut records = [[0, 0]];
        let mut pool = DataPool::default();
        fill_records(&nodes, 0, None, &mut None, &mut records, 1, &mut pool).unwrap();
        assert_eq!(records[0][0], records[0][1]);
        assert_eq!(pool.entries.len(), 1);

        nodes[0].value = None;
        fill_records(
            &nodes,
            0,
            None,
            &mut None,
            &mut records,
            1,
            &mut DataPool::default(),
        )
        .unwrap();
        assert_eq!(records[0], [1, 1]);
    }

    #[test]
    fn deep_merge_maps_and_arrays() {
        let old = Value::Map(BTreeMap::from([
            ("country".into(), Value::Utf8("FR".into())),
            (
                "categories".into(),
                Value::Array(vec![Value::Utf8("abuse".into())]),
            ),
        ]));
        let new = Value::Map(BTreeMap::from([
            ("city".into(), Value::Utf8("Paris".into())),
            (
                "categories".into(),
                Value::Array(vec![Value::Utf8("proxy".into())]),
            ),
        ]));
        let Value::Map(merged) = merge_values(old, new, MergeStrategy::DeepMerge) else {
            panic!()
        };
        assert_eq!(merged.get("country"), Some(&Value::Utf8("FR".into())));
        assert_eq!(merged.get("city"), Some(&Value::Utf8("Paris".into())));
        assert_eq!(
            merged.get("categories"),
            Some(&Value::Array(vec![
                Value::Utf8("abuse".into()),
                Value::Utf8("proxy".into())
            ]))
        );
    }

    #[cfg(feature = "reader")]
    #[test]
    fn insertion_apis_preserve_distinct_networks_and_shared_values() {
        use crate::{MmdbEncode, MmdbRecord, Reader};
        use std::{net::IpAddr, sync::Arc};

        struct Entry {
            network: IpNetwork,
            value: u32,
        }
        impl MmdbEncode for Entry {
            fn encode(&self) -> Result<Value> {
                Ok(Value::Uint32(self.value))
            }
        }
        impl MmdbRecord for Entry {
            fn network(&self) -> IpNetwork {
                self.network
            }
        }

        let metadata = crate::MetadataBuilder::new().ip_version(4).build().unwrap();
        let mut writer = Writer::with_metadata(metadata);
        writer
            .insert("198.51.100.0/24".parse().unwrap(), &"serde")
            .unwrap();
        writer
            .insert_encoded(
                "198.51.101.0/24".parse().unwrap(),
                &Entry {
                    network: "198.51.101.0/24".parse().unwrap(),
                    value: 1,
                },
            )
            .unwrap();
        writer
            .insert_entry(&Entry {
                network: "198.51.102.0/24".parse().unwrap(),
                value: 2,
            })
            .unwrap();
        writer
            .insert_batch([("198.51.103.0/24".parse().unwrap(), Value::Uint32(3))])
            .unwrap();
        writer
            .insert_value("198.51.103.0/24".parse().unwrap(), Value::Uint32(3))
            .unwrap();
        let shared = Arc::new(Value::Uint32(4));
        writer
            .insert_batch_shared(
                [
                    "198.51.104.0/24".parse().unwrap(),
                    "198.51.105.0/24".parse().unwrap(),
                ],
                &shared,
            )
            .unwrap();
        writer
            .insert_value_arc("198.51.106.0/24".parse().unwrap(), Arc::clone(&shared))
            .unwrap();
        writer
            .insert_value_shared("198.51.107.0/24".parse().unwrap(), Arc::clone(&shared))
            .unwrap();
        writer
            .insert_value_shared(
                "198.51.107.0/24".parse().unwrap(),
                Arc::new(Value::Uint32(5)),
            )
            .unwrap();

        let bytes = writer.finish().unwrap();
        let reader = Reader::from_bytes(&bytes).unwrap();
        for (last_octet, expected) in [
            (100, "serde"),
            (101, "1"),
            (102, "2"),
            (103, "3"),
            (104, "4"),
            (105, "4"),
            (106, "4"),
            (107, "5"),
        ] {
            let ip: IpAddr = format!("198.51.{last_octet}.1").parse().unwrap();
            let actual = reader.lookup_value(ip).unwrap().to_json();
            assert_eq!(actual.to_string().trim_matches('"'), expected);
        }
    }

    #[test]
    fn batches_stop_on_family_mismatch_and_file_write_propagates_io_errors() {
        use std::sync::Arc;

        let metadata = crate::MetadataBuilder::new().ip_version(4).build().unwrap();
        let mut writer = Writer::with_metadata(metadata.clone());
        let ipv4 = "198.51.100.0/24".parse().unwrap();
        let ipv6 = "2001:db8::/32".parse().unwrap();
        assert!(matches!(
            writer.insert_batch([(ipv4, Value::Bool(true)), (ipv6, Value::Bool(false)),]),
            Err(Error::InvalidIpVersion(6))
        ));
        assert!(matches!(
            writer.insert_batch_shared([ipv6], &Arc::new(Value::Bool(true))),
            Err(Error::InvalidIpVersion(6))
        ));
        writer
            .insert_batch_shared(std::iter::empty(), &Arc::new(Value::Bool(true)))
            .unwrap();
        let file = tempfile::tempdir().unwrap();
        let path = file.path().join("generated.mmdb");
        Writer::with_metadata(metadata.clone())
            .write_to_file(&path)
            .unwrap();
        assert!(!std::fs::read(&path).unwrap().is_empty());
        assert!(
            Writer::with_metadata(metadata)
                .write_to_file(file.path())
                .is_err()
        );
    }

    #[test]
    fn merge_strategies_and_node_encodings_reject_invalid_inputs() {
        let writer = Writer::with_metadata(crate::MetadataBuilder::new().build().unwrap())
            .merge_strategy(MergeStrategy::DeepMerge);
        assert_eq!(writer.merge_strategy, MergeStrategy::DeepMerge);
        let left = Value::Array(vec![Value::Uint16(1), Value::Uint16(2)]);
        let right = Value::Array(vec![Value::Uint16(2), Value::Uint16(3)]);
        assert_eq!(
            merge_values(left.clone(), right.clone(), MergeStrategy::Replace),
            right
        );
        assert_eq!(
            merge_values(left.clone(), right.clone(), MergeStrategy::Append),
            Value::Array(vec![
                Value::Uint16(1),
                Value::Uint16(2),
                Value::Uint16(2),
                Value::Uint16(3)
            ])
        );
        assert_eq!(
            merge_values(left, right, MergeStrategy::AppendUnique),
            Value::Array(vec![Value::Uint16(1), Value::Uint16(2), Value::Uint16(3)])
        );
        assert_eq!(
            merge_values(
                Value::Bool(false),
                Value::Bool(true),
                MergeStrategy::DeepMerge
            ),
            Value::Bool(true)
        );
        assert_eq!(
            merge_values(
                Value::Bool(false),
                Value::Utf8("x".into()),
                MergeStrategy::Append
            ),
            Value::Utf8("x".into())
        );
        assert_eq!(
            merge_values(
                Value::Bool(false),
                Value::Utf8("x".into()),
                MergeStrategy::AppendUnique
            ),
            Value::Utf8("x".into())
        );

        assert!(encode_node(1 << 24, 0, 24, &mut Vec::new()).is_err());
        assert!(encode_node(0, 1 << 28, 28, &mut Vec::new()).is_err());
        assert!(encode_node(0, 0, 20, &mut Vec::new()).is_err());
        let mut bytes = Vec::new();
        encode_node(0x1234_5678, 0x9abc_def0, 32, &mut bytes).unwrap();
        assert_eq!(bytes, [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0]);
        assert!(resolve_data_pointer(usize::MAX, 1).is_err());
        assert!(resolve_data_pointer(u32::MAX as usize, 1).is_err());
    }
}
