//! MaxMind DB reader implementation.
//!
//! Key architectural design choices:
//! - Contiguous 64-byte-aligned native tree representation (`PreparedTree`).
//! - Lock-free, zero-atomic eager radix accelerator tables for root and IPv4 subtrees.
//! - Const-unrolled byte-stride multi-bit traversal without per-lookup heap allocations.
//! - Full zero-copy decoding with fast ASCII validation and direct memory-mapped access.

mod marker;
mod scan;
mod tree;

use memmap2::Mmap;
use serde::de::DeserializeOwned;
use std::net::IpAddr;
use std::path::Path;
use std::sync::OnceLock;

use crate::decoder::{Decoder, RawDecoder};
use crate::{Error, Metadata, MmdbDecode, Result, ValueRef};
use marker::{METADATA_MARKER, find_metadata_marker};
use tree::PreparedTree;

/// Computes the IPv4 start node for IPv6 databases.
/// This is the node at which IPv4 lookups should start when the database is IPv6.
/// Called once during reader construction; marked cold to keep out of icache.
#[cold]
fn compute_ipv4_start_node(bytes: &[u8], node_count: u64, record_size: u16) -> Result<Option<u64>> {
    let mut node = 0_u64;
    match record_size {
        28 => {
            for _ in 0..96 {
                if node >= node_count {
                    return Ok(None);
                }
                let offset = node as usize * 7;
                if offset + 7 > bytes.len() {
                    return Err(Error::UnexpectedEof);
                }
                // SAFETY: the full node window was checked against bytes.len() above.
                let p = unsafe { bytes.as_ptr().add(offset) };
                // SAFETY: the checked node window contains every byte loaded below.
                node = unsafe {
                    (u64::from(*p.add(3) >> 4) << 24)
                        | (u64::from(*p) << 16)
                        | (u64::from(*p.add(1)) << 8)
                        | u64::from(*p.add(2))
                };
                if node >= node_count {
                    return Ok(None);
                }
            }
            Ok(Some(node))
        }
        32 => {
            for _ in 0..96 {
                if node >= node_count {
                    return Ok(None);
                }
                let offset = node as usize * 8;
                if offset + 8 > bytes.len() {
                    return Err(Error::UnexpectedEof);
                }
                // SAFETY: the full node window was checked against bytes.len() above.
                let p = unsafe { bytes.as_ptr().add(offset) };
                // SAFETY: the checked 8-byte node contains this unaligned 4-byte load.
                node = u64::from(u32::from_be(unsafe {
                    core::ptr::read_unaligned(p.cast::<u32>())
                }));
                if node >= node_count {
                    return Ok(None);
                }
            }
            Ok(Some(node))
        }
        24 => {
            for _ in 0..96 {
                if node >= node_count {
                    return Ok(None);
                }
                let offset = node as usize * 6;
                if offset + 6 > bytes.len() {
                    return Err(Error::UnexpectedEof);
                }
                // SAFETY: the full node window was checked against bytes.len() above.
                let p = unsafe { bytes.as_ptr().add(offset) };
                // SAFETY: the checked node window contains every byte loaded below.
                node = unsafe {
                    (u64::from(*p) << 16) | (u64::from(*p.add(1)) << 8) | u64::from(*p.add(2))
                };
                if node >= node_count {
                    return Ok(None);
                }
            }
            Ok(Some(node))
        }
        _ => {
            // Fallback for non-specialized record sizes
            for _ in 0..96 {
                if node >= node_count {
                    return Ok(None);
                }
                node = match read_record_static(bytes, node as usize, record_size)? {
                    Some(next) if next < node_count => next,
                    _ => return Ok(None),
                };
            }
            Ok(Some(node))
        }
    }
}

/// Helper to read a record from the tree for compute_ipv4_start_node fallback.
#[cold]
fn read_record_static(bytes: &[u8], node: usize, record_size: u16) -> Result<Option<u64>> {
    let node_size = usize::from(record_size) / 4;
    let offset = node
        .checked_mul(node_size)
        .ok_or(Error::InvalidNode(node as u64))?;
    let slice = bytes
        .get(offset..offset + node_size)
        .ok_or(Error::UnexpectedEof)?;
    // For simplicity, just read the left child (side 0)
    // This matches the original compute_ipv4_start behavior
    Ok(Some(read_packed_record_static(
        slice,
        usize::from(record_size),
        0,
    )?))
}

/// Helper to read a packed record for compute_ipv4_start_node fallback.
#[cold]
fn read_packed_record_static(bytes: &[u8], bits: usize, side: usize) -> Result<u64> {
    let start = side * bits;
    let mut value = 0_u64;
    for bit_index in start..start + bits {
        let byte = *bytes.get(bit_index / 8).ok_or(Error::UnexpectedEof)?;
        let bit = (byte >> (7 - (bit_index % 8))) & 1;
        value = (value << 1) | u64::from(bit);
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// Source : zero-copy (Borrowed / Mmap) ou owned
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Source<'a> {
    Borrowed(&'a [u8]),
    Mmap(Mmap),
    Owned(Vec<u8>),
}

impl Source<'_> {
    #[inline(always)]
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Borrowed(v) => v,
            Self::Mmap(v) => v,
            Self::Owned(v) => v,
        }
    }
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// Parsed MaxMind DB reader.
///
/// Opening from `&[u8]` does not copy the database bytes. Opening prepares
/// native-endian children for every valid record size; common 24/28/32-bit
/// records also use a cache-aligned tree and bounded byte-stride or radix tables.
/// This increases open time and reader memory use, but the first lookup performs
/// no index construction. Generic `lookup_value` materializes map and
/// array containers; typed borrowed decoding avoids those containers.
#[derive(Debug)]
pub struct Reader<'a> {
    source: Source<'a>,
    metadata: Metadata,
    data_pointer_bias: usize,
    data_section_start: usize,
    metadata_marker: usize,
    ipv4_start_node: Option<u64>,
    /// Built at open, then shared immutably by all lookup threads.
    prepared_tree: PreparedTree,
}

impl Reader<'static> {
    /// Opens a MaxMind DB file into an owned byte buffer.
    ///
    /// Returns an I/O or format error if the file cannot be read or validated.
    /// Preparing the search-tree index is part of the open cost.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::Reader;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb");
    /// let reader = Reader::open(path)?;
    /// assert_eq!(reader.metadata().ip_version, 4);
    /// # Ok(())
    /// # }
    /// ```
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_vec(std::fs::read(path)?)
    }

    /// Opens a MaxMind DB file through a read-only memory mapping.
    /// The search-tree index is prepared before this method returns.
    ///
    /// # Safety
    /// The mapped file must not be modified or truncated for as long as the
    /// returned reader exists. Violating this operating-system mmap
    /// requirement can make subsequent memory accesses invalid.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::Reader;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb");
    /// // SAFETY: This checked-in fixture is not changed while the reader exists.
    /// let reader = unsafe { Reader::open_mmap(path)? };
    /// assert_eq!(reader.metadata().ip_version, 4);
    /// # Ok(())
    /// # }
    /// ```
    pub unsafe fn open_mmap(path: impl AsRef<Path>) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        // SAFETY: Forwarded to the caller by this function's explicit safety contract.
        let mmap = unsafe { Mmap::map(&file)? };
        Self::from_source(Source::Mmap(mmap))
    }

    /// Builds a reader that owns the provided bytes.
    ///
    /// Returns a format error when the byte vector is not a valid MMDB.
    /// The search-tree index is prepared before this method returns.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::Reader;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_vec(bytes.to_vec())?;
    /// assert_eq!(reader.metadata().ip_version, 4);
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_vec(data: Vec<u8>) -> Result<Self> {
        Self::from_source(Source::Owned(data))
    }
}

impl<'a> Reader<'a> {
    /// Opens a MaxMind DB directly from a borrowed byte slice without copying it.
    /// Returns a format error when the bytes are not a valid MMDB.
    /// The search-tree index is prepared before this method returns.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::Reader;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// assert_eq!(reader.metadata().ip_version, 4);
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn from_bytes(data: &'a [u8]) -> Result<Self> {
        Self::from_source(Source::Borrowed(data))
    }

    fn from_source(source: Source<'a>) -> Result<Self> {
        let bytes = source.bytes();
        let marker = find_metadata_marker(bytes)?;
        let metadata_start = marker + METADATA_MARKER.len();
        let decoder = Decoder::new(bytes, metadata_start, bytes.len());
        let (value, _) = decoder.decode_at(metadata_start)?;
        let metadata = Metadata::from_value(&value)?;

        if metadata.binary_format_major_version != 2 {
            return Err(Error::InvalidMetadata(
                "unsupported binary format major version",
            ));
        }
        if !matches!(metadata.ip_version, 4 | 6) {
            return Err(Error::InvalidIpVersion(metadata.ip_version));
        }
        if metadata.record_size < 24 || metadata.record_size % 4 != 0 || metadata.record_size > 64 {
            return Err(Error::InvalidMetadata(
                "record_size must be a multiple of 4 between 24 and 64",
            ));
        }

        let node_bytes = usize::from(metadata.record_size) / 4;
        let node_count_usize = usize::try_from(metadata.node_count)
            .map_err(|_| Error::InvalidMetadata("node count exceeds address space"))?;
        let search_tree_size = node_count_usize
            .checked_mul(node_bytes)
            .ok_or(Error::InvalidMetadata("search tree size overflow"))?;
        // MMDB data records use a pointer relative to node_count. Folding the
        // invariant part here turns the hot-path conversion into one checked add.
        let data_pointer_bias = search_tree_size
            .checked_sub(node_count_usize)
            .ok_or(Error::InvalidMetadata("invalid search tree geometry"))?;
        let data_section_start = search_tree_size
            .checked_add(16)
            .ok_or(Error::InvalidMetadata("data section offset overflow"))?;

        if data_section_start > marker || data_section_start > bytes.len() {
            return Err(Error::InvalidDatabase(
                "search tree overlaps metadata or exceeds file",
            ));
        }
        if bytes.get(search_tree_size..data_section_start) != Some(&[0_u8; 16][..]) {
            return Err(Error::InvalidDatabase(
                "missing 16-byte data section separator",
            ));
        }

        // record_size is validated to [24, 64] and a multiple of 4;
        // conversion to u8 is infallible, but keep the defensive check.
        let record_size_u8 = u8::try_from(metadata.record_size)
            .map_err(|_| Error::InvalidMetadata("record_size must fit in u8"))?;

        // Compute ipv4_start_node for IPv6 databases before building the prepared tree
        // so we can pass it directly to avoid rebuilding later.
        let ipv4_start_node = if metadata.ip_version == 6 {
            compute_ipv4_start_node(bytes, metadata.node_count, metadata.record_size)?
        } else {
            None
        };

        // Prepare the immutable native tree and its accelerators before the
        // Reader becomes visible. This moves the one-time allocation and tree
        // decoding out of the first lookup, including concurrent first lookups.
        // Every valid record width is prepared here, so lookups have one
        // traversal path and preparation errors surface during open.
        let prepared_tree = PreparedTree::build(
            &bytes[..data_section_start],
            record_size_u8,
            metadata.node_count,
            ipv4_start_node,
        )?;

        Ok(Self {
            source,
            metadata,
            data_pointer_bias,
            data_section_start,
            metadata_marker: marker,
            ipv4_start_node,
            prepared_tree,
        })
    }

    /// Visits every reachable network and its decoded value in address order.
    ///
    /// Only the `reader` feature is required. This walks stored CIDR ranges,
    /// not individual IP addresses, and skips no-data branches. IPv4 networks
    /// below `::/96` in an IPv6 database are returned as IPv4 networks; other
    /// IPv6 ranges keep their stored address family. These are the ranges
    /// exported by the tree, not necessarily the original writer insertions.
    ///
    /// Strings and bytes borrow this reader and may be retained after the
    /// callback. Generic maps and arrays allocate their container vectors.
    /// Equal payloads referenced by several ranges are visited for each range;
    /// callers collecting unique fields should deduplicate those fields.
    /// For schema-directed decoding, use [`Self::visit_borrowed_records`].
    ///
    /// # Errors
    ///
    /// The first traversal, decoding or callback error stops the scan. Earlier
    /// callbacks are not rolled back, so publish collected results only after
    /// success. Cycles and nodes beyond the address width are rejected. To bound
    /// expansion of shared subtrees, a scan allows at most
    /// `256 * (node_count + 1)` tree entries before returning
    /// [`Error::ResourceLimit`]. Existing value-decoding limits also apply.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{Reader, ValueRef};
    /// use std::collections::BTreeSet;
    ///
    /// # fn main() -> Result<(), libmaxminddb_rs::Error> {
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// let mut categories = BTreeSet::new();
    /// reader.visit_records(|_network, value| {
    ///     if let Some(ValueRef::Utf8(category)) = value.get("category") {
    ///         categories.insert(*category);
    ///     }
    ///     Ok(())
    /// })?;
    /// assert!(categories.contains("compat"));
    /// # Ok(())
    /// # }
    /// ```
    pub fn visit_records<'s>(
        &'s self,
        mut visitor: impl FnMut(crate::IpNetwork, ValueRef<'s>) -> Result<()>,
    ) -> Result<()> {
        let decoder = Decoder::new(
            self.source.bytes(),
            self.data_section_start,
            self.metadata_marker,
        );
        self.visit_record_offsets(|network, offset| {
            let (value, _) = decoder.decode_at(offset)?;
            visitor(network, value)
        })
    }

    /// Visits every reachable network using schema-directed borrowed decoding.
    ///
    /// This has the same ordering, address-family rules, traversal limits and
    /// error behavior as [`Self::visit_records`], but decodes directly into `T`
    /// through [`MmdbDecode`]. Borrowed strings and bytes are not copied.
    /// Derived records containing only borrowed scalars do not allocate generic
    /// map/array containers; owning fields such as `Vec` or `String` still
    /// allocate. A manual `MmdbDecode` implementation may use the generic
    /// decoder through the trait's default implementation.
    ///
    /// This method requires only `reader`. Deriving `MmdbDecode` is optional
    /// and additionally requires `derive`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "derive")]
    /// # fn main() -> Result<(), libmaxminddb_rs::Error> {
    /// use libmaxminddb_rs::{MmdbDecode, Reader};
    /// use std::collections::BTreeSet;
    ///
    /// #[derive(MmdbDecode)]
    /// struct Record<'a> { category: &'a str }
    ///
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// let mut categories = BTreeSet::new();
    /// reader.visit_borrowed_records(|_network, record: Record<'_>| {
    ///     categories.insert(record.category);
    ///     Ok(())
    /// })?;
    /// assert!(categories.contains("compat"));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "derive"))] fn main() {}
    /// ```
    pub fn visit_borrowed_records<'s, T>(
        &'s self,
        mut visitor: impl FnMut(crate::IpNetwork, T) -> Result<()>,
    ) -> Result<()>
    where
        T: MmdbDecode<'s>,
    {
        self.visit_record_offsets(|network, offset| {
            let mut decoder = RawDecoder::new(
                self.source.bytes(),
                self.data_section_start,
                self.metadata_marker,
                offset,
            );
            visitor(network, T::decode_raw(&mut decoder)?)
        })
    }

    /// Returns parsed database metadata.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::Reader;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// assert_eq!(reader.metadata().database_type, "libmaxminddb-rs-compat");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Returns the underlying database bytes.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::Reader;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// assert_eq!(reader.as_bytes(), bytes);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.source.bytes()
    }

    /// Looks up an IP and returns a borrowed generic value.
    /// Returns [`Error::NotFound`] on a miss; malformed data can also produce a decode error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::Reader;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// let value = reader.lookup_value("203.0.113.7".parse()?)?;
    /// assert!(value.get("country").is_some());
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn lookup_value(&self, ip: IpAddr) -> Result<ValueRef<'_>> {
        self.lookup_value_with_prefix(ip).map(|(v, _)| v)
    }

    /// Looks up an IP and returns the borrowed value and matched prefix length.
    /// Returns [`Error::NotFound`] on a miss. The prefix is the matched network's
    /// CIDR length, not the address width.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::Reader;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// let (_, prefix) = reader.lookup_value_with_prefix("203.0.113.7".parse()?)?;
    /// assert_eq!(prefix, 24);
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn lookup_value_with_prefix(&self, ip: IpAddr) -> Result<(ValueRef<'_>, u8)> {
        let (offset, prefix) = self.resolve_offset(ip)?;
        let decoder = Decoder::new(
            self.source.bytes(),
            self.data_section_start,
            self.metadata_marker,
        );
        let (value, _) = decoder.decode_at(offset)?;
        Ok((value, prefix))
    }

    /// Decodes an IP record into a user type generated with `#[derive(MmdbDecode)]`.
    ///
    /// Derived types are decoded in a single pass straight from the database
    /// bytes: strings borrow the buffer, unknown fields are skipped without
    /// being decoded, and no intermediate [`ValueRef`] tree is built, so the
    /// lookup performs no heap allocation beyond the `Vec`/`String` fields the
    /// target type itself owns.
    ///
    /// Returns [`Error::NotFound`] when the IP matches no network in the
    /// database. No record is decoded on this miss path.
    ///
    /// The traversal and miss handling stay on the inlined fast path; only a
    /// hit enters the (cold) decode machinery. This keeps the dominant miss
    /// case from materializing the large `Result<T, Error>` return value that
    /// a user-sized `T` (for example a `CityRecord`) would force onto the
    /// hot loop, and lets `lookup_borrowed` match tree-only traversal on the miss
    /// path.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MmdbDecode, Reader};
    /// # #[cfg(feature = "derive")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// #[derive(MmdbDecode)]
    /// struct Record<'a> { category: &'a str }
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// let record: Record<'_> = reader.lookup_borrowed("203.0.113.7".parse()?)?;
    /// assert_eq!(record.category, "compat");
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "derive"))] fn main() {}
    /// ```
    #[inline(always)]
    pub fn lookup_borrowed<'s, T>(&'s self, ip: IpAddr) -> Result<T>
    where
        T: MmdbDecode<'s>,
    {
        let (offset, _) = self.resolve_offset(ip)?;
        decode_borrowed_at::<T>(self, offset)
    }

    /// Like [`lookup_borrowed`](Self::lookup_borrowed) but returns `Option<T>`
    /// instead of `Result<T, Error>`.
    ///
    /// On a miss the function returns `None` without ever constructing the
    /// `Error` enum. This avoids the out-of-line `Error` drop-glue that the
    /// `Result<T, Error>` return type forces on every miss when the caller
    /// discards the error (via `.ok()`, `match`, etc.). Because `Error` has
    /// `String`-bearing variants, its drop-glue is a non-inlined function
    /// call; eliminating it shaves several nanoseconds from the miss path —
    /// the dominant shape for random/absent workloads.
    ///
    /// Decode failures (only reachable on a corrupt or malicious database)
    /// are also folded into `None`. Callers that must distinguish "not found"
    /// from "found but undecodable" should use [`lookup_borrowed`](Self::lookup_borrowed)
    /// instead.
    ///
    /// # Performance
    /// On the miss path this matches tree-only traversal — no `Error`
    /// is constructed or dropped. On the hit path it is identical to
    /// `lookup_borrowed`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MmdbDecode, Reader};
    /// # #[cfg(feature = "derive")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// #[derive(MmdbDecode)]
    /// struct Record<'a> { category: &'a str }
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// let record: Option<Record<'_>> = reader.lookup_borrowed_opt("192.0.2.1".parse()?);
    /// assert!(record.is_none());
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "derive"))] fn main() {}
    /// ```
    #[inline(always)]
    pub fn lookup_borrowed_opt<'s, T>(&'s self, ip: IpAddr) -> Option<T>
    where
        T: MmdbDecode<'s>,
    {
        let (offset, _) = self.resolve_offset_opt(ip)?;
        decode_borrowed_at_opt::<T>(self, offset)
    }

    /// Decodes a borrowed record and passes it to `on_hit`, returning `None`
    /// when no network matches the address.
    ///
    /// A caller that only needs a small result can return it from the callback
    /// without carrying a potentially large `T` through the miss path. Decode
    /// and invalid-pointer errors are preserved as [`Error`] values.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{MmdbDecode, Reader};
    /// # #[cfg(feature = "derive")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// #[derive(MmdbDecode)]
    /// struct Record<'a> { category: &'a str }
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// let category = reader.lookup_borrowed_map("203.0.113.7".parse()?, |r: Record<'_>| r.category)?;
    /// assert_eq!(category, Some("compat"));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "derive"))] fn main() {}
    /// ```
    #[inline(always)]
    pub fn lookup_borrowed_map<'s, T, R>(
        &'s self,
        ip: IpAddr,
        on_hit: impl FnOnce(T) -> R,
    ) -> Result<Option<R>>
    where
        T: MmdbDecode<'s>,
    {
        let traversed = match (self.metadata.ip_version, ip) {
            (4, IpAddr::V4(v4)) => self.traverse_ipv4(0, &v4.octets(), 0),
            (4, IpAddr::V6(_)) => None,
            (6, IpAddr::V6(v6)) => self.traverse_ipv6(0, &v6.octets(), 0),
            (6, IpAddr::V4(v4)) => self
                .ipv4_start_node
                .and_then(|start| self.traverse_ipv4(start, &v4.octets(), 0)),
            (v, _) => return Err(Error::InvalidIpVersion(v)),
        };
        let Some((record, _)) = traversed else {
            return Ok(None);
        };
        let offset = self.record_to_file_offset(record)?;
        decode_borrowed_map_at::<T, R>(self, offset, on_hit).map(Some)
    }

    /// Deserializes through serde into an owned type.
    /// Returns [`Error::NotFound`] on a miss and a conversion error if the
    /// stored value does not match the requested type. This path allocates.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::Reader;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// let value: serde_json::Value = reader.lookup("203.0.113.7".parse()?)?;
    /// assert_eq!(value["category"], "compat");
    /// # Ok(())
    /// # }
    /// ```
    pub fn lookup<T: DeserializeOwned>(&self, ip: IpAddr) -> Result<T> {
        let value = self.lookup_value(ip)?;
        Ok(serde_json::from_value(value.to_json())?)
    }

    /// Looks up many IPs and returns borrowed values in the same order as `ips`.
    ///
    /// The output vector and decoded map/array containers allocate. For
    /// large batches, work is split across threads with `std::thread::scope`.
    /// Each result independently reports a miss or decode error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::{Error, Reader};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// let ips = ["203.0.113.7".parse()?, "192.0.2.1".parse()?];
    /// let results = reader.lookup_many(&ips);
    /// assert!(results[0].is_ok());
    /// assert!(matches!(results[1], Err(Error::NotFound)));
    /// # Ok(())
    /// # }
    /// ```
    pub fn lookup_many(&self, ips: &[IpAddr]) -> Vec<Result<ValueRef<'_>>> {
        const PARALLEL_MIN: usize = 4_096;
        static WORKERS: OnceLock<usize> = OnceLock::new();
        let workers =
            *WORKERS.get_or_init(|| std::thread::available_parallelism().map_or(1, |c| c.get()));

        if workers <= 1 || ips.len() < PARALLEL_MIN {
            return ips.iter().map(|&ip| self.lookup_value(ip)).collect();
        }

        let chunk = ips.len().div_ceil(workers.min(ips.len()));
        let mut results: Vec<Result<ValueRef<'_>>> =
            (0..ips.len()).map(|_| Err(Error::NotFound)).collect();

        std::thread::scope(|scope| {
            for (slice, slots) in ips.chunks(chunk).zip(results.chunks_mut(chunk)) {
                scope.spawn(move || {
                    for (ip, slot) in slice.iter().zip(slots) {
                        *slot = self.lookup_value(*ip);
                    }
                });
            }
        });

        results
    }

    // -----------------------------------------------------------------------
    // Hot path
    // -----------------------------------------------------------------------

    /// Resolves a tree record to a validated file offset without decoding.
    ///
    /// # Performance
    /// This method is approximately **50-100x faster** than `lookup_value` for
    /// databases with complex data structures, as it skips all decoding overhead.
    ///
    /// # Performance Note
    /// This is the primary hot path for lookups. Marked `#[inline(always)]` after
    /// benchmarking showed significant improvements:
    /// - lookup_ipv4_hot: **10-20% faster**
    /// - lookup_ipv4_random: **8-13% faster**
    /// - city_lookup_ipv4: **6-9% faster**
    /// - open_from_bytes: **10-14% faster**
    /// - open_owned_file: **8-14% faster**
    /// - Most writer benchmarks: **3-7% faster**
    ///
    /// The only regression was deep_merge_100 (+1-5%), which is acceptable given
    /// the broad improvements across critical reader and writer paths.
    #[inline(always)]
    #[allow(clippy::unnecessary_lazy_evaluations)]
    fn resolve_offset(&self, ip: IpAddr) -> Result<(usize, u8)> {
        let (record, prefix) = match (self.metadata.ip_version, ip) {
            (4, IpAddr::V4(v4)) => self
                .traverse_ipv4(0, &v4.octets(), 0)
                .ok_or(Error::NotFound)?,
            (4, IpAddr::V6(_)) => return Err(Error::NotFound),
            (6, IpAddr::V6(v6)) => self
                .traverse_ipv6(0, &v6.octets(), 0)
                .ok_or(Error::NotFound)?,
            (6, IpAddr::V4(v4)) => {
                let start = self.ipv4_start_node.ok_or_else(|| Error::NotFound)?;
                self.traverse_ipv4(start, &v4.octets(), 0)
                    .ok_or(Error::NotFound)?
            }
            (v, _) => return Err(Error::InvalidIpVersion(v)),
        };
        let offset = self.record_to_file_offset(record)?;
        Ok((offset, prefix))
    }

    /// Internal `Option`-returning counterpart of `resolve_offset`.
    ///
    /// Never constructs an `Error`: the miss path stays a plain `None` so
    /// `lookup_borrowed_opt` (and any future `Option`-returning public API)
    /// avoids the out-of-line `Error` drop-glue entirely on the dominant
    /// random/absent workload.
    #[inline(always)]
    fn resolve_offset_opt(&self, ip: IpAddr) -> Option<(usize, u8)> {
        let (record, prefix) = match (self.metadata.ip_version, ip) {
            (4, IpAddr::V4(v4)) => self.traverse_ipv4(0, &v4.octets(), 0)?,
            (4, IpAddr::V6(_)) => return None,
            (6, IpAddr::V6(v6)) => self.traverse_ipv6(0, &v6.octets(), 0)?,
            (6, IpAddr::V4(v4)) => {
                let start = self.ipv4_start_node?;
                self.traverse_ipv4(start, &v4.octets(), 0)?
            }
            _ => return None,
        };
        let offset = self.record_to_file_offset_opt(record)?;
        Some((offset, prefix))
    }

    /// Checks if an IP address exists in the database without decoding or allocating.
    ///
    /// It traverses the tree and validates the resulting data offset.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use libmaxminddb_rs::Reader;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// assert!(reader.lookup_exists("203.0.113.7".parse()?));
    /// assert!(!reader.lookup_exists("192.0.2.1".parse()?));
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn lookup_exists(&self, ip: IpAddr) -> bool {
        self.resolve_offset(ip).is_ok()
    }

    #[inline(always)]
    fn traverse_ipv4(&self, node: u64, octets: &[u8; 4], prefix_base: u8) -> Option<(u64, u8)> {
        self.prepared_tree.traverse_ipv4(node, octets, prefix_base)
    }

    #[inline(always)]
    fn traverse_ipv6(&self, node: u64, octets: &[u8; 16], prefix_base: u8) -> Option<(u64, u8)> {
        self.prepared_tree.traverse_ipv6(node, octets, prefix_base)
    }

    // -----------------------------------------------------------------------
    // Record decoding (used by compute_ipv4_start, non-hot)
    // -----------------------------------------------------------------------

    #[allow(dead_code, clippy::unnecessary_lazy_evaluations)]
    #[cold]
    fn read_record(&self, node: u64, side: usize) -> Result<u64> {
        if node >= self.metadata.node_count || side > 1 {
            return Err(Error::InvalidNode(node));
        }
        let node_size = usize::from(self.metadata.record_size) / 4;
        let offset = usize::try_from(node)
            .ok()
            .and_then(|n| n.checked_mul(node_size))
            .ok_or_else(|| Error::InvalidNode(node))?;
        let bytes = self
            .source
            .bytes()
            .get(offset..offset + node_size)
            .ok_or_else(|| Error::UnexpectedEof)?;

        match self.metadata.record_size {
            24 => {
                let base = side * 3;
                Ok((u64::from(unsafe { *bytes.get_unchecked(base) }) << 16)
                    | (u64::from(unsafe { *bytes.get_unchecked(base + 1) }) << 8)
                    | u64::from(unsafe { *bytes.get_unchecked(base + 2) }))
            }
            28 => {
                if side == 0 {
                    Ok((u64::from(unsafe { *bytes.get_unchecked(3) } >> 4) << 24)
                        | (u64::from(unsafe { *bytes.get_unchecked(0) }) << 16)
                        | (u64::from(unsafe { *bytes.get_unchecked(1) }) << 8)
                        | u64::from(unsafe { *bytes.get_unchecked(2) }))
                } else {
                    Ok((u64::from(unsafe { *bytes.get_unchecked(3) } & 0x0f) << 24)
                        | (u64::from(unsafe { *bytes.get_unchecked(4) }) << 16)
                        | (u64::from(unsafe { *bytes.get_unchecked(5) }) << 8)
                        | u64::from(unsafe { *bytes.get_unchecked(6) }))
                }
            }
            32 => {
                let base = side * 4;
                Ok(u64::from(u32::from_be_bytes(
                    unsafe { bytes.get_unchecked(base..base + 4) }
                        .try_into()
                        .expect("length checked"),
                )))
            }
            bits => read_packed_record(bytes, usize::from(bits), side),
        }
    }

    #[inline]
    #[allow(clippy::unnecessary_lazy_evaluations)]
    fn record_to_file_offset(&self, record: u64) -> Result<usize> {
        let node_count = self.metadata.node_count;
        if record < node_count.saturating_add(16) {
            return Err(Error::InvalidOffset(record as usize));
        }
        let record = usize::try_from(record).map_err(|_| Error::InvalidOffset(usize::MAX))?;
        let offset = record
            .checked_add(self.data_pointer_bias)
            .ok_or(Error::InvalidOffset(record))?;

        if offset >= self.metadata_marker {
            return Err(Error::InvalidOffset(offset));
        }
        Ok(offset)
    }

    /// `Option`-returning counterpart of `record_to_file_offset`.
    /// Folds every invalid-offset case into `None` so the `Option`-returning
    /// lookup path never constructs or drops an `Error`.
    #[inline(always)]
    fn record_to_file_offset_opt(&self, record: u64) -> Option<usize> {
        let node_count = self.metadata.node_count;
        if record < node_count.saturating_add(16) {
            return None;
        }
        let record = usize::try_from(record).ok()?;
        let offset = record.checked_add(self.data_pointer_bias)?;
        if offset >= self.metadata_marker {
            return None;
        }
        Some(offset)
    }
}

// ---------------------------------------------------------------------------
// Free helpers for open-time record decoding and prepared-tree construction
// ---------------------------------------------------------------------------

/// Decodes a borrowed value from an already-resolved data-section offset.
///
/// Kept out of the inlined `lookup_borrowed` miss path: the caller already
/// paid the traversal, and only a hit reaches here. Building the big
/// `Result<T, Error>` return value inside this cold function keeps the hot
/// miss loop free of the large-`T` machinery.
#[cold]
#[inline(never)]
fn decode_borrowed_at<'s, T>(reader: &'s Reader<'_>, offset: usize) -> Result<T>
where
    T: MmdbDecode<'s>,
{
    let mut decoder = RawDecoder::new(
        reader.source.bytes(),
        reader.data_section_start,
        reader.metadata_marker,
        offset,
    );
    T::decode_raw(&mut decoder)
}

/// Decodes and consumes a hit before returning to the lookup's hot path, so
/// a large user record does not cross this function boundary.
#[cold]
#[inline(never)]
fn decode_borrowed_map_at<'s, T, R>(
    reader: &'s Reader<'_>,
    offset: usize,
    on_hit: impl FnOnce(T) -> R,
) -> Result<R>
where
    T: MmdbDecode<'s>,
{
    let mut decoder = RawDecoder::new(
        reader.source.bytes(),
        reader.data_section_start,
        reader.metadata_marker,
        offset,
    );
    let value = T::decode_raw(&mut decoder)?;
    Ok(on_hit(value))
}

/// `Option`-returning counterpart of `decode_borrowed_at`.
///
/// Used by `lookup_borrowed_opt` so the miss path never touches the `Error`
/// enum. Decode failures (unreachable on a validated tree) are folded into
/// `None`; the cold `Error` is constructed and dropped entirely inside this
/// function, never reaching the caller's hot loop.
#[cold]
#[inline(never)]
fn decode_borrowed_at_opt<'s, T>(reader: &'s Reader<'_>, offset: usize) -> Option<T>
where
    T: MmdbDecode<'s>,
{
    let mut decoder = RawDecoder::new(
        reader.source.bytes(),
        reader.data_section_start,
        reader.metadata_marker,
        offset,
    );
    T::decode_raw(&mut decoder).ok()
}

/// Loads an unaligned 64-bit big-endian word.
///
/// # Safety
/// - `base` must point to a valid tree buffer.
#[allow(clippy::unnecessary_lazy_evaluations)]
fn read_packed_record(bytes: &[u8], bits: usize, side: usize) -> Result<u64> {
    let start = side * bits;
    let mut value = 0_u64;
    for bit_index in start..start + bits {
        let byte = *bytes.get(bit_index / 8).ok_or(Error::UnexpectedEof)?;
        let bit = (byte >> (7 - (bit_index % 8))) & 1;
        value = (value << 1) | u64::from(bit);
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// Tests: prepared traversal, error branches, and metadata-validation failures.
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "writer"))]
mod reader_tests {
    use super::*;
    use crate::writer::Writer;
    use crate::{MetadataBuilder, Value};

    // Two fixed nodes written densely. `count` is the number of nodes in the
    // tree; indices below `count` are child pointers, `count` is "no data".
    fn raw_node(record_size: u8, left: u64, right: u64) -> Vec<u8> {
        match record_size {
            24 => {
                let mut n = Vec::with_capacity(6);
                n.extend_from_slice(&(left as u32).to_be_bytes()[1..=3]);
                n.extend_from_slice(&(right as u32).to_be_bytes()[1..=3]);
                n
            }
            28 => vec![
                (left >> 16) as u8,
                (left >> 8) as u8,
                left as u8,
                ((((left >> 24) & 0x0f) << 4) | ((right >> 24) & 0x0f)) as u8,
                (right >> 16) as u8,
                (right >> 8) as u8,
                right as u8,
            ],
            32 => {
                let mut n = Vec::with_capacity(8);
                n.extend_from_slice(&(left as u32).to_be_bytes());
                n.extend_from_slice(&(right as u32).to_be_bytes());
                n
            }
            // Wider records are packed MSB-first, with the right child
            // starting mid-byte for widths such as 36 or 44 bits.
            36..=64 if record_size.is_multiple_of(4) => {
                let bits = usize::from(record_size);
                let mut n = vec![0_u8; bits / 4];
                for (side, value) in [left, right].into_iter().enumerate() {
                    for bit in 0..bits {
                        let stream_bit = side * bits + bit;
                        n[stream_bit / 8] |=
                            (((value >> (bits - bit - 1)) & 1) as u8) << (7 - stream_bit % 8);
                    }
                }
                n
            }
            _ => panic!("unexpected record size {record_size}"),
        }
    }

    fn node_stream(record_size: u8, nodes: &[(u64, u64)]) -> Vec<u8> {
        let mut out = Vec::new();
        for &(l, r) in nodes {
            out.extend_from_slice(&raw_node(record_size, l, r));
        }
        out
    }

    // Builds a reader straight out of raw pieces, bypassing `from_source`
    // validation, so traversal and offset math can be tested directly.
    fn crafted_reader(
        record_size: u16,
        node_count: u64,
        tree: Vec<u8>,
        data: Vec<u8>,
        ip_version: u16,
        ipv4_start: Option<u64>,
    ) -> Reader<'static> {
        let mut file = tree;
        file.extend_from_slice(&[0_u8; 16]);
        file.extend_from_slice(&data);
        let search_tree_size = (node_count as usize) * (usize::from(record_size) / 4);
        let data_section_start = search_tree_size + 16;
        let prepared_tree = PreparedTree::build(&file, record_size as u8, node_count, ipv4_start)
            .expect("crafted tree must be preparable");
        Reader {
            source: Source::Owned(file),
            metadata: Metadata {
                node_count,
                record_size,
                ip_version,
                database_type: "test".into(),
                languages: vec!["en".into()],
                binary_format_major_version: 2,
                binary_format_minor_version: 0,
                build_epoch: 0,
                description: Default::default(),
            },
            data_pointer_bias: search_tree_size - (node_count as usize),
            data_section_start,
            metadata_marker: data_section_start + data.len(),
            ipv4_start_node: ipv4_start,
            prepared_tree,
        }
    }

    // A one-node tree whose single record resolves to the (only) data payload,
    // which is a utf8 string of length 2: 0x42 'a' 'b'.
    fn scalar_reader(record_size: u16) -> Reader<'static> {
        let node_count = 1;
        let data_pointer = node_count + 16;
        crafted_reader(
            record_size,
            node_count,
            node_stream(record_size as u8, &[(data_pointer, data_pointer)]),
            vec![0x42, b'a', b'b'],
            6,
            Some(0),
        )
    }

    struct ScanText<'a>(&'a str);

    impl<'a> MmdbDecode<'a> for ScanText<'a> {
        fn decode(value: &ValueRef<'a>) -> Result<Self> {
            match value {
                ValueRef::Utf8(text) => Ok(Self(text)),
                _ => Err(Error::InvalidDatabase("expected scan text")),
            }
        }
    }

    fn parallel_text_scan(reader: &Reader<'_>, calls: &std::sync::atomic::AtomicU64) -> Result<()> {
        reader.visit_borrowed_records_parallel_with_workers(
            std::num::NonZeroUsize::new(4).unwrap(),
            |_, text: ScanText<'_>| {
                assert_eq!(text.0, "ab");
                calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            },
        )
    }

    #[test]
    fn scan_rejects_cycles_before_expanding_repeated_branches() {
        for version in [4, 6] {
            let reader =
                crafted_reader(24, 1, node_stream(24, &[(0, 0)]), vec![], version, Some(0));
            let mut calls = 0;
            let result = reader.visit_records(|_, _| {
                calls += 1;
                Ok(())
            });
            assert!(matches!(
                result,
                Err(Error::InvalidDatabase("cyclic search tree"))
            ));
            assert_eq!(calls, 0);
            let calls = std::sync::atomic::AtomicU64::new(0);
            assert!(matches!(
                parallel_text_scan(&reader, &calls),
                Err(Error::InvalidDatabase("cyclic search tree"))
            ));
            assert_eq!(calls.into_inner(), 0);
        }
    }

    #[test]
    fn scan_rejects_nodes_beyond_the_address_width() {
        for (version, bits) in [(4, 32), (6, 128)] {
            let count = bits + 1;
            let nodes: Vec<_> = (0..count).map(|n| (n + 1, count)).collect();
            let reader =
                crafted_reader(24, count, node_stream(24, &nodes), vec![], version, Some(0));
            assert!(matches!(
                reader.visit_records(|_, _| Ok(())),
                Err(Error::InvalidDatabase("search tree exceeds address width"))
            ));
            assert!(matches!(
                parallel_text_scan(&reader, &std::sync::atomic::AtomicU64::new(0)),
                Err(Error::InvalidDatabase("search tree exceeds address width"))
            ));
        }
    }

    #[test]
    fn scan_allows_shared_subtrees_but_bounds_exponential_expansion() {
        for layers in [3, 20] {
            let pointer = layers + 16;
            let nodes: Vec<_> = (0..layers)
                .map(|n| {
                    let next = if n + 1 < layers { n + 1 } else { pointer };
                    (next, next)
                })
                .collect();
            let reader = crafted_reader(
                24,
                layers,
                node_stream(24, &nodes),
                vec![0x42, b'a', b'b'],
                4,
                None,
            );
            let mut calls = 0;
            let result = reader.visit_records(|_, value| {
                assert_eq!(value, ValueRef::Utf8("ab"));
                calls += 1;
                Ok(())
            });
            if layers == 3 {
                result.unwrap();
                assert_eq!(calls, 8);
            } else {
                assert!(matches!(
                    result,
                    Err(Error::ResourceLimit("MMDB scan tree-entry budget exceeded"))
                ));
                assert!(calls <= 256 * (layers + 1));
            }
            let calls = std::sync::atomic::AtomicU64::new(0);
            let result = parallel_text_scan(&reader, &calls);
            if layers == 3 {
                result.unwrap();
                assert_eq!(calls.into_inner(), 8);
            } else {
                assert!(matches!(
                    result,
                    Err(Error::ResourceLimit("MMDB scan tree-entry budget exceeded"))
                ));
                assert!(calls.into_inner() <= 256 * (layers + 1));
            }
        }
    }

    #[test]
    fn parallel_scan_checks_ancestors_beyond_the_frontier_and_reserved_pointers() {
        // A shared branching chain produces 32 frontier tasks before workers
        // encounter a back-edge to the root. Its cycle crosses the task boundary.
        let count = 7;
        let nodes: Vec<_> = (0..count)
            .map(|n| {
                let next = if n + 1 < count { n + 1 } else { 0 };
                (next, next)
            })
            .collect();
        let reader = crafted_reader(24, count, node_stream(24, &nodes), vec![], 4, None);
        assert!(matches!(
            parallel_text_scan(&reader, &std::sync::atomic::AtomicU64::new(0)),
            Err(Error::InvalidDatabase("cyclic search tree"))
        ));
        for size in [24_u16, 28, 32, 36, 64] {
            let reader = crafted_reader(
                size,
                1,
                node_stream(size as u8, &[(2, 1)]),
                vec![0x42, b'a', b'b'],
                4,
                None,
            );
            assert!(matches!(
                parallel_text_scan(&reader, &std::sync::atomic::AtomicU64::new(0)),
                Err(Error::InvalidOffset(_))
            ));
            let reader = scalar_reader(size);
            let calls = std::sync::atomic::AtomicU64::new(0);
            parallel_text_scan(&reader, &calls).unwrap();
            assert_eq!(calls.into_inner(), 2);
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn ipv4_subtree_start_handles_early_leaves_truncation_and_packed_fallback() {
        for size in [24_u8, 28, 32] {
            assert_eq!(
                compute_ipv4_start_node(&raw_node(size, 1, 1), 1, u16::from(size)).unwrap(),
                None
            );
            assert!(matches!(
                compute_ipv4_start_node(&[], 1, u16::from(size)),
                Err(Error::UnexpectedEof)
            ));
            assert_eq!(
                compute_ipv4_start_node(&[], 0, u16::from(size)).unwrap(),
                None
            );
        }
        assert_eq!(
            compute_ipv4_start_node(&raw_node(40, 1, 1), 1, 40).unwrap(),
            None
        );
        assert!(matches!(
            compute_ipv4_start_node(&[], 1, 40),
            Err(Error::UnexpectedEof)
        ));
        assert!(matches!(
            read_record_static(&[], 0, 40),
            Err(Error::UnexpectedEof)
        ));
        assert!(matches!(
            read_packed_record_static(&[], 40, 0),
            Err(Error::UnexpectedEof)
        ));

        let nodes: Vec<_> = (0..97_u64).map(|i| ((i + 1).min(96), 97)).collect();
        assert_eq!(
            compute_ipv4_start_node(&node_stream(40, &nodes), 97, 40).unwrap(),
            Some(96)
        );
    }

    #[test]
    fn optional_and_mapped_lookups_preserve_hit_miss_and_decode_error_semantics() {
        struct Text<'a>(&'a str);
        impl<'a> MmdbDecode<'a> for Text<'a> {
            fn decode(value: &ValueRef<'a>) -> Result<Self> {
                match value {
                    ValueRef::Utf8(v) => Ok(Self(v)),
                    _ => Err(Error::DecodingError("expected text".into())),
                }
            }
        }
        struct Number;
        impl<'a> MmdbDecode<'a> for Number {
            fn decode(_value: &ValueRef<'a>) -> Result<Self> {
                Err(Error::DecodingError("expected number".into()))
            }
        }

        let reader = scalar_reader(24);
        let address = ip("2001:db8::1");
        assert_eq!(
            reader.lookup_borrowed_opt::<Text<'_>>(address).unwrap().0,
            "ab"
        );
        assert_eq!(
            reader
                .lookup_borrowed_map(address, |record: Text<'_>| record.0)
                .unwrap(),
            Some("ab")
        );
        assert!(reader.lookup_borrowed_opt::<Number>(address).is_none());
        assert!(
            reader
                .lookup_borrowed_map(address, |_record: Number| ())
                .is_err()
        );
        assert!(reader.lookup_exists(address));
        assert!(reader.record_to_file_offset_opt(17).is_some());
        assert!(reader.record_to_file_offset_opt(1).is_none());
        assert!(
            reader
                .record_to_file_offset_opt(usize::MAX as u64)
                .is_none()
        );

        let miss = crafted_reader(24, 1, raw_node(24, 1, 1), vec![0x42, b'a', b'b'], 4, None);
        let ipv4 = ip("203.0.113.1");
        assert!(miss.lookup_borrowed_opt::<Text<'_>>(ipv4).is_none());
        assert_eq!(
            miss.lookup_borrowed_map(ipv4, |record: Text<'_>| record.0)
                .unwrap(),
            None
        );
        assert!(!miss.lookup_exists(ipv4));
        assert!(miss.lookup_borrowed_opt::<Text<'_>>(address).is_none());

        let mut ipv6 = scalar_reader(24);
        assert_eq!(ipv6.lookup_borrowed_opt::<Text<'_>>(ipv4).unwrap().0, "ab");
        assert_eq!(
            ipv6.lookup_borrowed_map(ipv4, |record: Text<'_>| record.0)
                .unwrap(),
            Some("ab")
        );
        ipv6.metadata.ip_version = 9;
        assert!(ipv6.lookup_borrowed_opt::<Text<'_>>(ipv4).is_none());
        assert!(matches!(
            ipv6.lookup_borrowed_map(ipv4, |_record: Text<'_>| ()),
            Err(Error::InvalidIpVersion(9))
        ));

        let mut offset_reader = scalar_reader(24);
        offset_reader.data_pointer_bias = usize::MAX;
        assert!(offset_reader.record_to_file_offset_opt(17).is_none());
        offset_reader.data_pointer_bias = 5;
        offset_reader.metadata_marker = 20;
        assert!(offset_reader.record_to_file_offset_opt(17).is_none());
    }

    #[test]
    fn scalar_traversal_all_record_sizes() {
        for record_size in [24, 28, 32] {
            let reader = scalar_reader(record_size);
            let (value, prefix) = reader.lookup_value_with_prefix(ip("2001:db8::1")).unwrap();
            assert_eq!(value, ValueRef::Utf8("ab"));
            assert_eq!(prefix, 1);
        }
    }

    #[test]
    fn scalar_traversal_packed_fallback() {
        // record_size 40 is not special-cased, so the generic `_` arm and
        // `read_packed_record` must carry the traversal.
        let reader = scalar_reader(40);
        let value = reader.lookup_value(ip("127.0.0.1")).unwrap();
        assert_eq!(value, ValueRef::Utf8("ab"));
    }

    #[test]
    fn scalar_traversal_reports_not_found() {
        let node_count = 1;
        // left record == node_count (no data), right record == data pointer.
        let reader = crafted_reader(
            24,
            node_count,
            node_stream(24, &[(node_count, 17)]),
            vec![0x42, b'a', b'b'],
            6,
            Some(0),
        );
        let err = reader.lookup_value(ip("2001:db8::1")).unwrap_err();
        assert!(matches!(err, Error::NotFound));
    }

    #[test]
    fn prepared_traversal_preserves_bits_nodes_and_prefixes() {
        // Alternating high/low bits exercise both children, including transitions
        // within a byte, across bytes and between the two IPv6 address words.
        let octets = [
            0xa5, 0x5a, 0x93, 0x6c, 0x81, 0x7e, 0xc3, 0x3c, 0xf0, 0x0f, 0x96, 0x69, 0x87, 0x78,
            0xaa, 0x55,
        ];
        for record_size in [24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64] {
            for ip_version in [4, 6] {
                let query = if ip_version == 4 {
                    IpAddr::from([octets[0], octets[1], octets[2], octets[3]])
                } else {
                    IpAddr::from(octets)
                };
                let query_bytes = &octets[..if ip_version == 4 { 4 } else { 16 }];
                for prefix in [1, 2, 7, 8, 9, 17, 31, 32, 63, 64, 65, 127, 128] {
                    if prefix > query_bytes.len() * 8 {
                        continue;
                    }
                    let count = prefix as u64;
                    let data_pointer = count + 16;
                    let mut nodes = Vec::new();
                    for bit in 0..prefix {
                        let side = (query_bytes[bit / 8] >> (7 - bit % 8)) & 1;
                        let next = if bit + 1 == prefix {
                            data_pointer
                        } else {
                            (bit + 1) as u64
                        };
                        nodes.push(if side == 0 {
                            (next, count)
                        } else {
                            (count, next)
                        });
                    }
                    let reader = crafted_reader(
                        record_size,
                        count,
                        node_stream(record_size as u8, &nodes),
                        vec![0x42, b'a', b'b'],
                        ip_version,
                        None,
                    );
                    let traverse = |bytes: &[u8]| {
                        if ip_version == 4 {
                            reader.traverse_ipv4(0, bytes.try_into().unwrap(), 0)
                        } else {
                            reader.traverse_ipv6(0, bytes.try_into().unwrap(), 0)
                        }
                    };
                    assert_eq!(traverse(query_bytes), Some((data_pointer, prefix as u8)));
                    assert_eq!(
                        reader.lookup_value_with_prefix(query).unwrap(),
                        (ValueRef::Utf8("ab"), prefix as u8)
                    );
                    // A differing bit before the registered prefix must miss;
                    // the untouched suffix is intentionally arbitrary.
                    for bit in [0, prefix / 2, prefix - 1] {
                        let mut miss = query_bytes.to_vec();
                        miss[bit / 8] ^= 0x80 >> (bit % 8);
                        assert_eq!(traverse(&miss), None);
                    }
                }
            }
        }
    }

    #[test]
    fn prepared_traversal_terminal_records_and_cycles_do_not_load_children() {
        for record_size in [24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64] {
            let max_record = if record_size == 64 {
                u64::MAX
            } else {
                (1_u64 << record_size) - 1
            };
            let reader = crafted_reader(
                record_size,
                3,
                node_stream(record_size as u8, &[(1, 3), (2, 3), (19, max_record)]),
                vec![0x42, b'a', b'b'],
                6,
                Some(0),
            );
            assert_eq!(reader.traverse_ipv6(1, &[0; 16], 10), Some((19, 12)));
            assert_eq!(
                reader.traverse_ipv6(2, &[0x80; 16], 0),
                Some((max_record, 1))
            );
            assert_eq!(
                reader.traverse_ipv6(max_record, &[0; 16], 7),
                Some((max_record, 7))
            );
            assert_eq!(reader.traverse_ipv6(3, &[0; 16], 0), None);
            assert!(matches!(
                reader.resolve_offset(ip("2000::")),
                Err(Error::InvalidOffset(_))
            ));

            // Reserved pointers still reach the common offset validator.
            let reserved = crafted_reader(
                record_size,
                1,
                node_stream(record_size as u8, &[(2, 2)]),
                vec![0x42, b'a', b'b'],
                6,
                Some(0),
            );
            assert!(matches!(
                reserved.resolve_offset(ip("::")),
                Err(Error::InvalidOffset(_))
            ));

            let cyclic = crafted_reader(
                record_size,
                1,
                node_stream(record_size as u8, &[(0, 0)]),
                Vec::new(),
                6,
                Some(0),
            );
            assert_eq!(cyclic.traverse_ipv6(0, &[0xa5; 16], 0), None);
        }
    }

    #[test]
    fn read_record_all_sizes_and_errors() {
        for record_size in [24, 28, 32] {
            // 3-node tree: node0: {1,2}, node1: {19,19} (data pointer),
            // node2: {3,19} (left = node_count => no data).
            let nodes = [(1, 2), (19, 19), (3, 19)];
            let reader = crafted_reader(
                record_size,
                3,
                node_stream(record_size as u8, &nodes),
                vec![0x42, b'a', b'b'],
                6,
                Some(0),
            );
            assert_eq!(reader.read_record(0, 0).unwrap(), 1);
            assert_eq!(reader.read_record(0, 1).unwrap(), 2);
            assert_eq!(reader.read_record(1, 0).unwrap(), 19);
            assert_eq!(reader.read_record(1, 1).unwrap(), 19);
            assert_eq!(reader.read_record(2, 0).unwrap(), 3);
            assert_eq!(reader.read_record(2, 1).unwrap(), 19);
            assert!(matches!(
                reader.read_record(3, 0).unwrap_err(),
                Error::InvalidNode(3)
            ));
            assert!(matches!(
                reader.read_record(0, 2).unwrap_err(),
                Error::InvalidNode(_)
            ));
        }
        // non-special-cased bits fall through to the packed reader.
        let reader = crafted_reader(
            40,
            1,
            node_stream(40, &[(17, 17)]),
            vec![0x42, b'a', b'b'],
            6,
            Some(0),
        );
        assert_eq!(reader.read_record(0, 0).unwrap(), 17);
        assert_eq!(reader.read_record(0, 1).unwrap(), 17);
    }

    #[test]
    fn read_record_rejects_out_of_bounds_node() {
        // node_count = 2 so node 2 is out of range.
        let reader = crafted_reader(
            24,
            2,
            node_stream(24, &[(0, 0), (0, 0)]),
            vec![0x42, b'a', b'b'],
            6,
            Some(0),
        );
        assert!(matches!(
            reader.read_record(2, 0).unwrap_err(),
            Error::InvalidNode(2)
        ));
    }

    #[test]
    fn record_to_file_offset_error_branches() {
        let reader = crafted_reader(
            24,
            3,
            node_stream(24, &[(0, 0), (0, 0), (0, 0)]),
            vec![0x42, b'a', b'b'],
            6,
            Some(0),
        );
        // record below node_count + 16.
        assert!(matches!(
            reader.record_to_file_offset(3).unwrap_err(),
            Error::InvalidOffset(3)
        ));
        // record that does not fit usize.
        assert!(matches!(
            reader.record_to_file_offset(u64::MAX).unwrap_err(),
            Error::InvalidOffset(usize::MAX)
        ));
        // add overflow of bias.
        assert!(matches!(
            reader
                .record_to_file_offset(0xffff_ffff_ffff_ff00)
                .unwrap_err(),
            Error::InvalidOffset(_)
        ));
        // valid record lands past the metadata marker.
        let reader = crafted_reader(
            24,
            3,
            node_stream(24, &[(0, 0), (0, 0), (0, 0)]),
            vec![],
            6,
            Some(0),
        );
        // bias = 18 - 3 = 15; record 24 => 24 + 15 = 39 >= marker (data_section_start + 0 = 34).
        assert!(matches!(
            reader.record_to_file_offset(24).unwrap_err(),
            Error::InvalidOffset(39)
        ));
    }

    #[test]
    fn lookup_rejects_v6_in_v4_database() {
        let reader = crafted_reader(24, 1, Vec::new(), Vec::new(), 4, None);
        assert!(matches!(
            reader.lookup_value(ip("::1")).unwrap_err(),
            Error::NotFound
        ));
    }

    #[test]
    fn lookup_rejects_invalid_ip_version() {
        let reader = crafted_reader(24, 1, Vec::new(), Vec::new(), 5, None);
        assert!(matches!(
            reader.lookup_value(ip("1.2.3.4")).unwrap_err(),
            Error::InvalidIpVersion(5)
        ));
    }

    #[test]
    fn lookup_v4_in_v6_missing_start_node() {
        // ipv4_start_node None -> NotFound.
        let reader = crafted_reader(24, 1, Vec::new(), Vec::new(), 6, None);
        assert!(matches!(
            reader.lookup_value(ip("1.2.3.4")).unwrap_err(),
            Error::NotFound
        ));
    }

    fn metadata_db(ip_version: u16) -> Vec<u8> {
        let mut writer = Writer::with_metadata(
            MetadataBuilder::new()
                .database_type("reader-tests")
                .ip_version(ip_version)
                .build()
                .unwrap(),
        );
        writer
            .insert_value(
                if ip_version == 6 {
                    "2001:db8::/32".parse().unwrap()
                } else {
                    "10.0.0.0/8".parse().unwrap()
                },
                Value::Map(Into::into({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert("name".into(), Value::Utf8("x".into()));
                    m
                })),
            )
            .unwrap();
        writer.finish().unwrap()
    }

    fn patch_uint16(db: &mut [u8], key: &str, value: u8) {
        let index = db
            .windows(key.len())
            .position(|w| w == key.as_bytes())
            .expect("key present in metadata");
        debug_assert_eq!(db[index + key.len()], 0xA1, "u16 encoded as control+1");
        db[index + key.len() + 1] = value;
    }

    #[test]
    fn from_source_rejects_bad_metadata_values() {
        let mut bad_major = metadata_db(6);
        patch_uint16(&mut bad_major, "binary_format_major_version", 3);
        assert!(matches!(
            Reader::from_bytes(&bad_major).unwrap_err(),
            Error::InvalidMetadata("unsupported binary format major version")
        ));

        let mut bad_ip = metadata_db(6);
        patch_uint16(&mut bad_ip, "ip_version", 5);
        assert!(matches!(
            Reader::from_bytes(&bad_ip).unwrap_err(),
            Error::InvalidIpVersion(5)
        ));

        let mut bad_record_size = metadata_db(6);
        patch_uint16(&mut bad_record_size, "record_size", 16);
        assert!(matches!(
            Reader::from_bytes(&bad_record_size).unwrap_err(),
            Error::InvalidMetadata(msg) if msg.contains("record_size")
        ));
    }

    #[test]
    fn from_source_rejects_broken_separator() {
        let db = metadata_db(6);
        let reader = Reader::from_bytes(&db).unwrap();
        let mut broken = db.clone();
        broken[reader.data_section_start - 1] = 0x01;
        assert!(matches!(
            Reader::from_bytes(&broken).unwrap_err(),
            Error::InvalidDatabase(msg) if msg.contains("separator")
        ));
    }

    #[test]
    fn from_source_and_as_bytes_and_serde_lookup() {
        let db = metadata_db(6);
        assert_eq!(Reader::from_bytes(&db).unwrap().metadata().ip_version, 6);
        let reader = Reader::from_bytes(&db).unwrap();
        let prepared = &reader.prepared_tree as *const PreparedTree;
        assert_eq!(reader.as_bytes(), &db);
        let map: std::collections::BTreeMap<String, String> =
            reader.lookup(ip("2001:db8::1")).unwrap();
        assert_eq!(map["name"], "x");
        assert_eq!(&reader.prepared_tree as *const PreparedTree, prepared);
    }

    #[test]
    fn open_owned_and_mmap_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.mmdb");
        std::fs::write(&path, metadata_db(6)).unwrap();
        let owned = Reader::open(&path).unwrap();
        assert!(matches!(
            owned.lookup_value(ip("2001:db8::1")).unwrap(),
            ValueRef::Map(_)
        ));
        // SAFETY: the temp file is not modified or truncated for the reader's lifetime.
        let mapped = unsafe { Reader::open_mmap(&path) }.unwrap();
        let (value, _) = mapped.lookup_value_with_prefix(ip("2001:db8::1")).unwrap();
        assert!(matches!(value, ValueRef::Map(_)));
    }
}
