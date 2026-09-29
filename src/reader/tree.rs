//! Prepared, cache-aligned, native-endian search tree.
//!
//! # Architecture
//!
//! 1. Memory representation: Common 24/28/32-bit records use a contiguous
//!    buffer of left/right `u32` children (8 bytes per node), aligned to 64
//!    bytes. Wider valid records use two decoded `u64` children per node.
//! 2. Simple native loads at runtime: Fetching the narrow child of node `N` for bit
//!    `side` is simply `nodes[(N << 1) | side]`, lowered by LLVM to a single
//!    32-bit load `mov eax, [rdi + rcx*4]`.
//! 3. Radix accelerator tables for narrow records (built during reader open):
//!    - `root_accel`: a `2^R`-entry table for tree node 0 (IPv6 / top-level).
//!      `R` is 8 for tiny trees, 16 from 1024 nodes, and 20 from 2^20 nodes
//!      when the byte-stride arena cannot fit its budget.
//!    - `ipv4_accel`: `2^R`-entry table for `ipv4_start_node` (accelerates
//!      IPv4 lookups in IPv6 databases).
//!
//!    The first `R` bits of the query collapse into a single cache-aligned
//!    table load instead of a chain of `R` dependent loads. For prefixes that
//!    terminate inside the covered `R` bits the whole lookup finishes in one
//!    table load (a `/16` IPv4 network is fully resolved by the `ipv4_accel`).
//!    Each entry stores the reached `node` and the exact `depth` (`1..=R`),
//!    preserving accurate prefix lengths on early matches.
//! 4. IPv6 byte-stride tables consume eight bits per dependent load. Only
//!    reachable stride-boundary nodes are expanded; a bounded arena and a
//!    node-to-table map handle DAGs/cycles without recursive expansion. Large
//!    expansions fall back to the native binary tree. IPv4 keeps its direct
//!    radix-16 table.
//! 5. Traversal specialization: after the table, the remaining bits are walked
//!    by const-unrolled tail loops (`traverse_ipv4` with 12/16/24 steps, IPv6 in
//!    44/48/56 + 64), which operate on 32/64-bit registers.

use super::read_packed_record;
use crate::{Error, Result};

/// Number of tree nodes above which the accelerator tables use `R = 16` bits.
///
/// Below this threshold a 256-entry radix-8 table is used instead: the table
/// write is then a trivial ~2 KiB, so opening tiny databases stays nearly free
/// while the whole tree already fits in L1, hiding the extra tail loads.
const ACCEL_RADIX16_MIN_NODES: usize = 1024;
/// Bits consumed by the radix-16 accelerator tables.
const ACCEL_BITS_16: u8 = 16;
// Large sparse IPv6 trees exhaust the byte arena budget. A wider root table
// replaces several dependent random loads with one contiguous 8 MiB table.
const ACCEL_BITS_20: u8 = 20;
const ACCEL_RADIX20_MIN_NODES: usize = 1 << 20;
/// Bits consumed by the radix-8 accelerator tables.
const ACCEL_BITS_8: u8 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
/// Compact accelerator table entry: node index in bits [0..32), depth in bits [32..40).
/// Uses 8 bytes total, matching the original `RootEntry` size but enabling bit-packed storage.
/// Node indices up to 2^32-1 are supported (far beyond any practical MMDB tree).
pub(crate) struct RootEntry(u64);

impl RootEntry {
    /// Creates a new entry from node index and depth.
    #[inline(always)]
    pub(crate) const fn new(node: u32, depth: u8) -> Self {
        Self((node as u64) | ((depth as u64) << 32))
    }

    /// Returns the node index.
    #[inline(always)]
    pub(crate) const fn node(self) -> u32 {
        (self.0 & 0xFFFF_FFFF) as u32
    }

    /// Returns the depth.
    #[inline(always)]
    pub(crate) const fn depth(self) -> u8 {
        ((self.0 >> 32) & 0xFF) as u8
    }
}

/// Heap-allocated node buffer whose base address is aligned to 64 bytes.
///
/// The search tree stores `node_count` nodes as `(left, right)` `u32` child
/// records (8 bytes per node). A 64-byte-aligned base plus the 8-byte node
/// stride guarantees every node lies wholly inside one cache line (an 8-byte
/// node at an 8-byte-aligned offset can never straddle a 64-byte boundary),
/// so the dependent-load traversal never pays a split-line penalty. The node
/// buffer is never mutated after construction and is freed only when the
/// `PreparedTree` is dropped.
struct AlignedNodes {
    ptr: *mut u32,
    len: usize,
}

// SAFETY: `AlignedNodes` uniquely owns its buffer (no interior mutability), so
// sharing an immutable `&PreparedTree` across threads is memory-safe, matching
// the `Send`/`Sync` of the owning `PreparedTree`.
unsafe impl Send for AlignedNodes {}
unsafe impl Sync for AlignedNodes {}

impl AlignedNodes {
    /// Allocates storage for `len` native child records. The caller must write
    /// every slot before creating a slice or publishing a PreparedTree. Drop
    /// only deallocates, so unwinding during initialization is safe as well.
    fn allocate(len: usize) -> Result<Self> {
        if len == 0 {
            return Ok(Self {
                ptr: std::ptr::NonNull::<u32>::dangling().as_ptr(),
                len: 0,
            });
        }
        let bytes = len.checked_mul(4).ok_or(Error::InvalidMetadata(
            "prepared tree: native size overflow",
        ))?;
        let layout = std::alloc::Layout::from_size_align(bytes, 64)
            .map_err(|_| Error::InvalidMetadata("prepared tree: native size overflow"))?;
        // SAFETY: layout has non-zero size, checked size/isize bounds, and
        // valid alignment. The result is only used for writes until initialized.
        let ptr = unsafe { std::alloc::alloc(layout) }.cast::<u32>();
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Ok(Self { ptr, len })
    }

    #[inline(always)]
    fn as_ptr(&self) -> *const u32 {
        self.ptr
    }
}

impl Drop for AlignedNodes {
    fn drop(&mut self) {
        if self.len == 0 {
            return;
        }
        // SAFETY: `ptr` and `len` mirror the layout used at allocation time
        // (`len * 4` bytes, align 64), matching the allocator contract of the
        // matching `alloc`/`dealloc` pair.
        let layout = unsafe { std::alloc::Layout::from_size_align_unchecked(self.len * 4, 64) };
        // SAFETY: `ptr` was produced by the matching `alloc` with `layout`; the
        // `PreparedTree` is being dropped so the allocation is no longer used.
        unsafe { std::alloc::dealloc(self.ptr.cast::<u8>(), layout) };
    }
}

/// Byte-stride index: each dependent load consumes eight address bits. Entries
/// retain the original node and exact consumed depth; the upper 24 bits identify
/// the next 256-entry table. Only reachable stride boundaries get tables.
struct ByteTree {
    entries: Box<[RootEntry]>,
    ipv4_table: usize,
}

impl ByteTree {
    fn build(nodes: &[u32], ipv4_start: usize) -> Option<Self> {
        let max_entries = (nodes.len().saturating_mul(4).saturating_add(1 << 20)).min(64 << 20) / 8;
        Self::build_limited(nodes, max_entries, ipv4_start)
    }

    /// Unwraps into the owned entries buffer and the IPv4 subtree table index.
    /// `PreparedTree` keeps these directly so the hot traversal reads the
    /// entries base with a single load from `self` (no `Option<ByteTree>` +
    /// `Box` double indirection, no extra branch on the miss path).
    fn into_parts(self) -> (Box<[RootEntry]>, usize) {
        (self.entries, self.ipv4_table)
    }

    fn build_limited(nodes: &[u32], max_entries: usize, ipv4_start: usize) -> Option<Self> {
        use std::collections::HashMap;
        let count = nodes.len() / 2;
        if count == 0 || max_entries < 256 {
            return None;
        }
        let mut roots = vec![0_usize];
        let mut indices = HashMap::new();
        indices.insert(0_usize, 0_usize);

        let ipv4_table = if ipv4_start > 0 && ipv4_start < count {
            let idx = roots.len();
            roots.push(ipv4_start);
            indices.insert(ipv4_start, idx);
            idx
        } else {
            0
        };

        let mut entries = Vec::new();
        let mut table_index = 0;
        while table_index < roots.len() {
            let table = build_accel(nodes.as_ptr(), count, roots[table_index], 8);
            for mut entry in table {
                let node = entry.node() as usize;
                if node < count {
                    let next = if let Some(&index) = indices.get(&node) {
                        index
                    } else {
                        let index = roots.len();
                        if (index + 1) * 256 > max_entries {
                            return None;
                        }
                        roots.push(node);
                        indices.insert(node, index);
                        index
                    };
                    entry.0 |= (next as u64) << 40;
                }
                entries.push(entry);
            }
            table_index += 1;
        }
        Some(Self {
            entries: entries.into_boxed_slice(),
            ipv4_table,
        })
    }
}

/// Unrolled 4-byte stride walk over `entries`. `table_start` is the index of
/// the 256-entry table covering the first octet (`0` for the root table).
///
/// Returns `Some((node, depth))` for a data pointer (`node > count`), `None`
/// when the walk settles on a live-tree or "no data" record. Returning a
/// compact 24-byte `Option` instead of a 48-byte `Result` keeps the dominant
/// miss path (which is just traversal) from building or copying the large
/// `Error` value; the reader's offset resolver converts once.
#[inline(always)]
fn byte_walk_v4_impl(
    ptr: *const RootEntry,
    octets: &[u8; 4],
    table_start: usize,
    count: u64,
    prefix_base: u8,
) -> Option<(u64, u8)> {
    let mut table = table_start;
    let mut depth = 0;

    // SAFETY: each live entry links to a complete 256-entry table; an octet
    // cannot index beyond the table prepared for this subtree.
    let entry0 = unsafe { *ptr.add((table << 8) | (octets[0] as usize)) };
    let node0 = u64::from(entry0.node());
    if node0 == count {
        return None;
    }
    depth += entry0.depth();
    if node0 >= count {
        return finish(node0, count, prefix_base, depth);
    }
    table = (entry0.0 >> 40) as usize;

    // SAFETY: each live entry links to a complete 256-entry table; an octet
    // cannot index beyond the table prepared for this subtree.
    let entry1 = unsafe { *ptr.add((table << 8) | (octets[1] as usize)) };
    let node1 = u64::from(entry1.node());
    if node1 == count {
        return None;
    }
    depth += entry1.depth();
    if node1 >= count {
        return finish(node1, count, prefix_base, depth);
    }
    table = (entry1.0 >> 40) as usize;

    // SAFETY: each live entry links to a complete 256-entry table; an octet
    // cannot index beyond the table prepared for this subtree.
    let entry2 = unsafe { *ptr.add((table << 8) | (octets[2] as usize)) };
    let node2 = u64::from(entry2.node());
    if node2 == count {
        return None;
    }
    depth += entry2.depth();
    if node2 >= count {
        return finish(node2, count, prefix_base, depth);
    }
    table = (entry2.0 >> 40) as usize;

    // SAFETY: each live entry links to a complete 256-entry table; an octet
    // cannot index beyond the table prepared for this subtree.
    let entry3 = unsafe { *ptr.add((table << 8) | (octets[3] as usize)) };
    let node3 = u64::from(entry3.node());
    if node3 == count {
        return None;
    }
    depth += entry3.depth();
    finish(node3, count, prefix_base, depth)
}

/// 16-byte stride walk over `entries`, ending at the first table entry whose
/// node is not a live tree node.
#[inline(always)]
fn byte_walk_v6_impl(
    ptr: *const RootEntry,
    octets: &[u8; 16],
    count: u64,
    prefix_base: u8,
) -> Option<(u64, u8)> {
    let mut table = 0;
    let mut node = 0;
    let mut depth = 0;
    for &octet in octets {
        // SAFETY: each live entry links to a complete 256-entry table; an octet
        // cannot index beyond the table prepared for this subtree.
        let entry = unsafe { *ptr.add((table << 8) | (octet as usize)) };
        node = u64::from(entry.node());
        if node == count {
            return None;
        }
        depth += entry.depth();
        if node >= count {
            return finish(node, count, prefix_base, depth);
        }
        table = (entry.0 >> 40) as usize;
    }
    finish(node, count, prefix_base, depth)
}

pub(crate) struct PreparedTree {
    /// Byte-stride accelerator entries. Owned directly by `PreparedTree` so the
    /// hot traversal fetches the entries base with one load from `self` instead
    /// of chasing `Option<ByteTree>` followed by `Box`. Empty when the native
    /// binary tree is used directly (trees below the size threshold).
    byte_entries: Box<[RootEntry]>,
    /// Child-table index inside `byte_entries` at which the IPv4 subtree starts
    /// (0 when the database is IPv4-only or has no IPv4 subtree).
    byte_ipv4_table: usize,
    /// 64-byte-aligned contiguous array of (left, right) node children.
    /// Each node is `[u32; 2]` = 8 bytes, laid out as a flat `u32` stream.
    /// The node buffer is never mutated after construction and is freed only when
    /// `PreparedTree` is dropped. The hot traversal reads through the cached `node_ptr`.
    /// This field is intentionally never read directly; it exists solely to keep the
    /// aligned buffer alive for the duration of `PreparedTree`'s lifetime.
    #[allow(dead_code)]
    nodes: AlignedNodes,

    /// Pre-computed child `u32` cursor: `nodes.as_ptr()` is cached so the
    /// hot traversal never re-derives the pointer from the owning allocation.
    node_ptr: *const u32,

    /// `node_count` cached as `usize` (the only width the hot loops use).
    count: usize,

    /// `u64` node count retained for the `finish` helper's public result.
    node_count: u64,
    record_size: u8,

    /// Number of bits consumed by the accelerator tables (`8`, `16` or `20`), fixed
    /// at build time from `count`. Both tables share the same radix so the
    /// const-specialized tail loops can be shared too.
    root_bits: u8,

    /// Radix table for node 0, built during tree preparation.
    root_accel: Box<[RootEntry]>,

    /// Radix table for a distinct IPv4 subtree; empty when the root is shared.
    ipv4_accel: Box<[RootEntry]>,

    /// The node from which `ipv4_accel` was built (0 when none).
    ipv4_start: u64,

    /// Prefetch hint retained for API and tests.
    pub(crate) prefetch_hint: i32,

    /// Wider MMDB records are decoded once at open into two u64 children per
    /// node. The common 24/28/32-bit layouts keep the compact aligned u32 path.
    wide_nodes: Box<[u64]>,
}

// SAFETY: `PreparedTree` owns its buffers exclusively, and never mutates them
// after construction. node_ptr points into its owned AlignedNodes allocation.
// Moving or sharing the tree does not invalidate that allocation.
unsafe impl Send for PreparedTree {}
unsafe impl Sync for PreparedTree {}

impl std::fmt::Debug for PreparedTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedTree")
            .field("node_count", &self.node_count)
            .field("record_size", &self.record_size)
            .field("root_bits", &self.root_bits)
            .field("prefetch_hint", &self.prefetch_hint)
            .finish_non_exhaustive()
    }
}

impl PreparedTree {
    pub(crate) fn build(
        raw: &[u8],
        record_size: u8,
        node_count: u64,
        ipv4_start_node: Option<u64>,
    ) -> Result<Self> {
        if !(24..=64).contains(&record_size) || !record_size.is_multiple_of(4) {
            return Err(Error::InvalidMetadata(
                "prepared tree: unsupported record size",
            ));
        }
        let node_size = usize::from(record_size) / 4;
        let node_count_usize = match usize::try_from(node_count) {
            Ok(n) => n,
            Err(_) => return Err(Error::InvalidMetadata("prepared tree: node count overflow")),
        };
        let tree_len = node_count_usize
            .checked_mul(node_size)
            .ok_or(Error::InvalidMetadata("prepared tree: size overflow"))?;
        if raw.len() < tree_len {
            return Err(Error::UnexpectedEof);
        }

        let native_len = node_count_usize
            .checked_mul(2)
            .ok_or(Error::InvalidMetadata(
                "prepared tree: native size overflow",
            ))?;
        if record_size > 32 {
            let mut wide_nodes = Vec::new();
            wide_nodes
                .try_reserve_exact(native_len)
                .map_err(|_| Error::InvalidMetadata("prepared tree: wide size overflow"))?;
            for packed in raw[..tree_len].chunks_exact(node_size) {
                wide_nodes.push(read_packed_record(packed, usize::from(record_size), 0)?);
                wide_nodes.push(read_packed_record(packed, usize::from(record_size), 1)?);
            }
            let aligned = AlignedNodes::allocate(0)?;
            return Ok(Self {
                wide_nodes: wide_nodes.into_boxed_slice(),
                byte_entries: Box::new([]),
                byte_ipv4_table: 0,
                node_ptr: aligned.as_ptr(),
                count: node_count_usize,
                nodes: aligned,
                node_count,
                record_size,
                root_bits: 0,
                root_accel: Box::new([]),
                ipv4_accel: Box::new([]),
                ipv4_start: 0,
                prefetch_hint: 0,
            });
        }
        let aligned = AlignedNodes::allocate(native_len)?;
        let ptr = raw.as_ptr();

        match record_size {
            24 => {
                for i in 0..node_count_usize {
                    let off = i * 6;
                    // SAFETY: off + 6 <= tree_len <= raw.len()
                    let left = unsafe {
                        (u32::from(*ptr.add(off)) << 16)
                            | (u32::from(*ptr.add(off + 1)) << 8)
                            | u32::from(*ptr.add(off + 2))
                    };
                    // SAFETY: off + 6 <= tree_len <= raw.len().
                    let right = unsafe {
                        (u32::from(*ptr.add(off + 3)) << 16)
                            | (u32::from(*ptr.add(off + 4)) << 8)
                            | u32::from(*ptr.add(off + 5))
                    };
                    // SAFETY: i < node_count_usize and native_len == 2 *
                    // node_count_usize. Each child slot is written once; the
                    // allocation is not read or shared until the loop completes.
                    unsafe {
                        aligned.ptr.add(i * 2).write(left);
                        aligned.ptr.add(i * 2 + 1).write(right);
                    }
                }
            }
            28 => {
                for i in 0..node_count_usize {
                    let off = i * 7;
                    // SAFETY: off + 7 <= tree_len <= raw.len()
                    let mid = unsafe { *ptr.add(off + 3) };
                    // SAFETY: off + 3 <= tree_len <= raw.len().
                    let left = unsafe {
                        (u32::from(mid >> 4) << 24)
                            | (u32::from(*ptr.add(off)) << 16)
                            | (u32::from(*ptr.add(off + 1)) << 8)
                            | u32::from(*ptr.add(off + 2))
                    };
                    // SAFETY: off + 7 <= tree_len <= raw.len().
                    let right = unsafe {
                        (u32::from(mid & 0x0f) << 24)
                            | (u32::from(*ptr.add(off + 4)) << 16)
                            | (u32::from(*ptr.add(off + 5)) << 8)
                            | u32::from(*ptr.add(off + 6))
                    };
                    // SAFETY: i < node_count_usize and native_len == 2 *
                    // node_count_usize. Each child slot is written once; the
                    // allocation is not read or shared until the loop completes.
                    unsafe {
                        aligned.ptr.add(i * 2).write(left);
                        aligned.ptr.add(i * 2 + 1).write(right);
                    }
                }
            }
            32 => {
                for i in 0..node_count_usize {
                    let off = i * 8;
                    // SAFETY: off + 8 <= tree_len <= raw.len()
                    let left = unsafe {
                        u32::from_be(core::ptr::read_unaligned(ptr.add(off).cast::<u32>()))
                    };
                    // SAFETY: off + 8 <= tree_len <= raw.len(); unaligned load.
                    let right = unsafe {
                        u32::from_be(core::ptr::read_unaligned(ptr.add(off + 4).cast::<u32>()))
                    };
                    // SAFETY: i < node_count_usize and native_len == 2 *
                    // node_count_usize. Each child slot is written once; the
                    // allocation is not read or shared until the loop completes.
                    unsafe {
                        aligned.ptr.add(i * 2).write(left);
                        aligned.ptr.add(i * 2 + 1).write(right);
                    }
                }
            }
            _ => unreachable!(),
        }

        // SAFETY: every match arm initialized exactly two u32 records per
        // node. The checked allocation covers native_len elements; the empty
        // allocation uses a non-null, u32-aligned dangling pointer.
        let nodes = unsafe { std::slice::from_raw_parts(aligned.as_ptr(), native_len) };
        let root_bits = if node_count_usize >= ACCEL_RADIX16_MIN_NODES {
            ACCEL_BITS_16
        } else {
            ACCEL_BITS_8
        };

        let prefetch_hint = detect_prefetch_hint();
        // Only a live, distinct subtree needs its own table. Normalizing a
        // terminal start also keeps defensive callers out of an empty table.
        let ipv4_start = ipv4_start_node
            .filter(|&node| node < node_count)
            .unwrap_or(0);
        let (byte_entries, byte_ipv4_table) = if node_count >= 128 {
            ByteTree::build(nodes, ipv4_start as usize)
                .map(ByteTree::into_parts)
                .unwrap_or((Box::new([]), 0))
        } else {
            (Box::<[RootEntry]>::default(), 0)
        };
        let root_bits = if byte_entries.is_empty() && node_count_usize >= ACCEL_RADIX20_MIN_NODES {
            ACCEL_BITS_20
        } else {
            root_bits
        };
        let root_accel = if byte_entries.is_empty() {
            build_accel(aligned.as_ptr(), node_count_usize, 0, root_bits)
        } else {
            Box::new([])
        };
        let ipv4_accel = if byte_entries.is_empty()
            && ipv4_start > 0
            && (ipv4_start as usize) < node_count_usize
        {
            build_accel(
                aligned.as_ptr(),
                node_count_usize,
                ipv4_start as usize,
                root_bits,
            )
        } else {
            // start == 0 always selects root_accel before ipv4_accel. Keeping a
            // second copy here wastes up to 8 MiB and is never read.
            Box::new([])
        };

        Ok(Self {
            wide_nodes: Box::new([]),
            byte_entries,
            byte_ipv4_table,
            node_ptr: aligned.as_ptr(),
            count: node_count as usize,
            nodes: aligned,
            node_count,
            record_size,
            root_bits,
            root_accel,
            ipv4_accel,
            ipv4_start,
            prefetch_hint,
        })
    }

    /// Returns the 8-, 16-, or 20-bit index into the radix tables for a query.
    #[inline(always)]
    fn table_index(&self, key: u32) -> usize {
        (key >> (32 - self.root_bits)) as usize
    }

    /// Resolves the accelerator entry for `start_node` over the first
    /// `root_bits` bits of the query (the leading bits of the big-endian 32-bit `key`).
    ///
    /// - `start_node == 0` -> `root_accel` (prepared once).
    /// - `start_node == ipv4_start` -> `ipv4_accel` (prepared once).
    /// - Otherwise (defensive / crafted readers) a scalar walk of `root_bits`.
    #[inline(always)]
    fn accel_entry(&self, start_node: u64, key: u32) -> RootEntry {
        let idx = self.table_index(key);
        if start_node == 0 {
            // SAFETY: idx < 2^root_bits == root_accel.len() for radix 8, 16 or 20.
            unsafe { *self.root_accel.get_unchecked(idx) }
        } else if start_node == self.ipv4_start {
            // SAFETY: idx < 2^root_bits == ipv4_accel.len() for radix 8, 16 or 20.
            unsafe { *self.ipv4_accel.get_unchecked(idx) }
        } else {
            self.walk_root_scalar(start_node as usize, key)
        }
    }

    /// Scalar walk of `root_bits` bits from `start` (only when no accelerator
    /// table applies). Precisely mirrors `build_accel` for the same key.
    #[inline(always)]
    fn walk_root_scalar(&self, mut node: usize, key: u32) -> RootEntry {
        let node_count = self.count;
        let nodes_ptr = self.node_ptr;
        let bits = self.root_bits;
        let k = key >> (32 - bits);
        let mut depth = 0_u8;
        for i in (0..bits).rev() {
            if node >= node_count {
                break;
            }
            let side = ((k >> i) & 1) as usize;
            // SAFETY: node < node_count, side is 0 or 1, and the native
            // allocation contains exactly two initialized records per node.
            node = unsafe { *nodes_ptr.add((node << 1) | side) } as usize;
            depth += 1;
        }
        RootEntry::new(node as u32, depth)
    }

    #[inline(always)]
    pub(crate) fn traverse_ipv4(
        &self,
        start_node: u64,
        octets: &[u8; 4],
        prefix_base: u8,
    ) -> Option<(u64, u8)> {
        if !self.byte_entries.is_empty() {
            if start_node == 0 {
                return byte_walk_v4_impl(
                    self.byte_entries.as_ptr(),
                    octets,
                    0,
                    self.node_count,
                    prefix_base,
                );
            }
            if start_node == self.ipv4_start {
                return byte_walk_v4_impl(
                    self.byte_entries.as_ptr(),
                    octets,
                    self.byte_ipv4_table,
                    self.node_count,
                    prefix_base,
                );
            }
        }
        if self.record_size > 32 {
            return self.walk_wide(start_node, octets, prefix_base);
        }
        let node_count = self.count;
        let entry = self.accel_entry(start_node, u32::from_be_bytes(*octets));
        if (entry.node() as usize) >= node_count {
            return finish(
                entry.node() as u64,
                self.node_count,
                prefix_base,
                entry.depth(),
            );
        }
        let node = entry.node() as usize;
        let depth = entry.depth();
        let bits = u32::from_be_bytes(*octets) << self.root_bits;
        match 32 - self.root_bits as usize {
            12 => self.walk_v4::<12>(node, depth, bits, prefix_base),
            16 => self.walk_v4::<16>(node, depth, bits, prefix_base),
            24 => self.walk_v4::<24>(node, depth, bits, prefix_base),
            _ => unreachable!("root_bits is 8, 16 or 20"),
        }
    }

    #[inline(always)]
    pub(crate) fn traverse_ipv6(
        &self,
        start_node: u64,
        octets: &[u8; 16],
        prefix_base: u8,
    ) -> Option<(u64, u8)> {
        if start_node == 0 && !self.byte_entries.is_empty() {
            return byte_walk_v6_impl(
                self.byte_entries.as_ptr(),
                octets,
                self.node_count,
                prefix_base,
            );
        }
        if self.record_size > 32 {
            return self.walk_wide(start_node, octets, prefix_base);
        }
        let node_count = self.count;
        if start_node == 0 {
            let entry = self.accel_entry(0, u32::from_be_bytes(octets[..4].try_into().unwrap()));
            if (entry.node() as usize) >= node_count {
                return finish(
                    entry.node() as u64,
                    self.node_count,
                    prefix_base,
                    entry.depth(),
                );
            }
            let node = entry.node() as usize;
            let depth = entry.depth();
            return match self.root_bits {
                ACCEL_BITS_20 => self.walk_v6::<44>(node, depth, octets, prefix_base),
                ACCEL_BITS_16 => self.walk_v6::<48>(node, depth, octets, prefix_base),
                ACCEL_BITS_8 => self.walk_v6::<56>(node, depth, octets, prefix_base),
                _ => unreachable!("root_bits is 8, 16 or 20"),
            };
        }
        // start_node != 0 (crafted readers / defensive path): scalar 128-bit walk.
        self.walk_scalar_128(start_node as usize, octets, prefix_base)
    }

    #[inline(always)]
    fn walk_wide(&self, mut node: u64, octets: &[u8], prefix_base: u8) -> Option<(u64, u8)> {
        let mut depth = 0_u8;
        for &octet in octets {
            for shift in (0..8).rev() {
                if node >= self.node_count {
                    return finish(node, self.node_count, prefix_base, depth);
                }
                let side = usize::from((octet >> shift) & 1);
                // node is below count, and build initialized two children per node.
                node = self.wide_nodes[((node as usize) << 1) | side];
                depth += 1;
            }
        }
        finish(node, self.node_count, prefix_base, depth)
    }

    /// Native-bit tail after the root accelerator (12, 16 or 24 bits).
    /// The caller establishes node < count, and every subsequent child is
    /// checked before it can be used as an index. The bound is const-specialized.
    #[inline(always)]
    fn walk_v4<const N: usize>(
        &self,
        mut node: usize,
        mut depth: u8,
        mut bits: u32,
        prefix_base: u8,
    ) -> Option<(u64, u8)> {
        let nodes_ptr = self.node_ptr;
        let node_count = self.count;

        // Process N bits with const-unrolled loop
        for _ in 0..N {
            let side = (bits >> 31) as usize;
            bits <<= 1;
            depth += 1;

            // Load child node pointer - NO bounds check
            let next = unsafe { *nodes_ptr.add((node << 1) | side) } as usize;

            // Early exit if we hit a data pointer (>= node_count)
            if next >= node_count {
                return finish(next as u64, self.node_count, prefix_base, depth);
            }

            node = next;
        }

        finish(node as u64, self.node_count, prefix_base, depth)
    }

    /// Walks the IPv6 bits after the accelerator. `HIGH` is the number of
    /// remaining bits in the first (high) 64-bit word (44 for radix-20, 48 for
    /// radix-16, 56 for radix-8); the low 64 bits are always walked after it.
    #[inline(always)]
    fn walk_v6<const HIGH: usize>(
        &self,
        mut node: usize,
        mut depth: u8,
        octets: &[u8; 16],
        prefix_base: u8,
    ) -> Option<(u64, u8)> {
        let nodes_ptr = self.node_ptr;
        let node_count = self.count;

        // Long native tails have the measured working-set size for this hint.
        #[cfg(target_arch = "x86_64")]
        if HIGH == 44 {
            // SAFETY: PREFETCHNTA is a non-faulting hint, not a dereference.
            // wrapping_add makes no in-bounds promise about the speculative
            // address. The native allocation stays owned by self during the walk.
            unsafe {
                core::arch::x86_64::_mm_prefetch(
                    nodes_ptr.wrapping_add((node << 1) + 16).cast(),
                    core::arch::x86_64::_MM_HINT_NTA,
                );
            }
        }

        // SAFETY: octets contains 16 bytes; each unaligned u64 load below
        // stays within one of its two complete eight-byte halves.
        let high_be =
            u64::from_be(unsafe { core::ptr::read_unaligned(octets.as_ptr().cast::<u64>()) });
        let mut high = high_be << (64 - HIGH);

        // Walk through HIGH bits
        for _ in 0..HIGH {
            let side = (high >> 63) as usize;
            high <<= 1;
            depth += 1;

            // SAFETY: the accelerator or preceding iteration established
            // node < node_count; side is 0 or 1, selecting an allocated child.
            let next = unsafe { *nodes_ptr.add((node << 1) | side) } as usize;

            if next >= node_count {
                return finish(next as u64, self.node_count, prefix_base, depth);
            }

            node = next;
        }

        // SAFETY: bytes 8..16 are within the 16-byte address.
        let mut low = u64::from_be(unsafe {
            core::ptr::read_unaligned(octets.as_ptr().add(8).cast::<u64>())
        });
        for _ in 0..64 {
            let side = (low >> 63) as usize;
            low <<= 1;
            depth += 1;

            // SAFETY: the accelerator or preceding iteration established
            // node < node_count; side is 0 or 1, selecting an allocated child.
            let next = unsafe { *nodes_ptr.add((node << 1) | side) } as usize;

            if next >= node_count {
                return finish(next as u64, self.node_count, prefix_base, depth);
            }

            node = next;
        }

        finish(node as u64, self.node_count, prefix_base, depth)
    }

    /// Defensive scalar 128-bit walk from an arbitrary start node.
    #[allow(dead_code)]
    #[cold]
    #[inline(never)]
    fn walk_scalar_128(
        &self,
        mut node: usize,
        octets: &[u8; 16],
        prefix_base: u8,
    ) -> Option<(u64, u8)> {
        let nodes_ptr = self.node_ptr;
        let node_count = self.count;
        let mut depth = 0_u8;

        let mut high = u64::from_be_bytes(octets[0..8].try_into().unwrap());
        for _ in 0..64 {
            if node >= node_count {
                return finish(node as u64, self.node_count, prefix_base, depth);
            }
            let side = (high >> 63) as usize;
            high <<= 1;
            depth += 1;

            // SAFETY: node < node_count checked above.
            node = unsafe { *nodes_ptr.add((node << 1) | side) } as usize;
        }
        let mut low = u64::from_be_bytes(octets[8..16].try_into().unwrap());
        for _ in 0..64 {
            if node >= node_count {
                return finish(node as u64, self.node_count, prefix_base, depth);
            }
            let side = (low >> 63) as usize;
            low <<= 1;
            depth += 1;

            // SAFETY: node < node_count checked above.
            node = unsafe { *nodes_ptr.add((node << 1) | side) } as usize;
        }
        finish(node as u64, self.node_count, prefix_base, depth)
    }
}

/// Builds a `2^bits`-entry radix table by walking from `start` for up to
/// `bits` bits, exactly mirroring the traversal semantics.
fn build_accel(base: *const u32, node_count: usize, start: usize, bits: u8) -> Box<[RootEntry]> {
    let len = 1_usize << bits;
    let mut table = vec![RootEntry::default(); len].into_boxed_slice();
    let mut ctx = AccelCtx {
        base,
        node_count,
        bits,
        table: &mut table,
    };
    fill_accel(&mut ctx, start, 0, 0, 0);
    table
}

/// Fixed parameters shared across the recursion in [`fill_accel`].
struct AccelCtx<'a> {
    base: *const u32,
    node_count: usize,
    bits: u8,
    table: &'a mut [RootEntry],
}

/// Recursively fills a contiguous run of table entries.
///
/// `node` sits at `prefix` (a `consumed`-bit index). While `node` is a live
/// tree node and the table radix is not exhausted, the recursion descends both
/// children, partitioning the remaining index space exactly once per leaf. Any
/// other state (a data pointer, "no data", or the radix boundary) writes one
/// `RootEntry` across the whole remaining `2^(bits - consumed)` suffix, so
/// every entry in `[0, 2^bits)` is written exactly once.
fn fill_accel(ctx: &mut AccelCtx<'_>, node: usize, depth: u8, prefix: usize, consumed: u8) {
    let bits = ctx.bits;
    if node >= ctx.node_count || consumed == bits {
        let entry = RootEntry::new(node as u32, depth);
        let remaining = usize::from(bits - consumed);
        let span = 1_usize << remaining;
        let base_index = prefix << remaining;
        // SAFETY: `prefix < 2^consumed` by construction (each recursion level
        // shifts in one bit), so `base_index + span <= 2^bits == table.len()`.
        let run = unsafe { ctx.table.get_unchecked_mut(base_index..base_index + span) };
        for slot in run {
            *slot = entry;
        }
        return;
    }
    // SAFETY: node < node_count is checked above and the flat buffer holds
    // exactly node_count * 2 u32s, so both child slots exist.
    let left = unsafe { *ctx.base.add(node << 1) };
    let right = unsafe { *ctx.base.add((node << 1) | 1) };
    fill_accel(ctx, left as usize, depth + 1, prefix << 1, consumed + 1);
    fill_accel(
        ctx,
        right as usize,
        depth + 1,
        (prefix << 1) | 1,
        consumed + 1,
    );
}

fn detect_prefetch_hint() -> i32 {
    #[cfg(target_arch = "x86_64")]
    {
        use core::arch::x86_64::{_MM_HINT_T0, _MM_HINT_T1, _MM_HINT_T2};
        if is_x86_feature_detected!("avx512f") {
            return _MM_HINT_T1;
        }
        if is_x86_feature_detected!("avx2") {
            return _MM_HINT_T0;
        }
        _MM_HINT_T2
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

#[inline(always)]
fn finish(node: u64, node_count: u64, prefix_base: u8, depth: u8) -> Option<(u64, u8)> {
    if node > node_count {
        Some((node, prefix_base.saturating_add(depth)))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node_bytes(record_size: u8, left: u64, right: u64) -> Vec<u8> {
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
            _ => panic!("unexpected record size {record_size}"),
        }
    }

    // node_count = 3:
    //   node0: left -> 1, right -> 2
    //   node1: left -> 19 (data pointer), right -> 19
    //   node2: left -> 3 (== node_count, no data), right -> 19
    fn small_tree(record_size: u8) -> (Vec<u8>, u64) {
        let node_count = 3_u64;
        let mut raw = Vec::new();
        raw.extend_from_slice(&node_bytes(record_size, 1, 2));
        raw.extend_from_slice(&node_bytes(record_size, 19, 19));
        raw.extend_from_slice(&node_bytes(record_size, 3, 19));
        raw.extend_from_slice(&[0_u8; 16]);
        (raw, node_count)
    }

    fn build_tree(record_size: u8) -> PreparedTree {
        let (raw, node_count) = small_tree(record_size);
        PreparedTree::build(&raw, record_size, node_count, None).expect("build should succeed")
    }

    #[test]
    fn aligned_native_allocation_rejects_size_overflow() {
        assert!(AlignedNodes::allocate(usize::MAX).is_err());
        assert!(AlignedNodes::allocate(isize::MAX as usize / 4 + 1).is_err());
        let storage = AlignedNodes::allocate(3).unwrap();
        assert_eq!(storage.as_ptr() as usize % 64, 0);
        // Dropping an allocation before initialization must not read its slots.
        drop(storage);
    }

    #[test]
    fn empty_tree_never_reads_a_node() {
        for bits in [24, 28, 32, 36, 64] {
            let tree = PreparedTree::build(&[0; 16], bits, 0, None).unwrap();
            assert!(tree.traverse_ipv4(0, &[0; 4], 0).is_none());
            assert!(tree.traverse_ipv6(0, &[0; 16], 0).is_none());
            assert!(tree.ipv4_accel.is_empty());
        }
    }

    #[test]
    fn terminal_ipv4_start_does_not_index_an_empty_accelerator() {
        let (raw, count) = small_tree(32);
        for start in [count, count + 16, u64::from(u32::MAX)] {
            let tree = PreparedTree::build(&raw, 32, count, Some(start)).unwrap();
            assert!(tree.ipv4_accel.is_empty());
            assert_eq!(
                tree.traverse_ipv4(start, &[0; 4], 7),
                finish(start, count, 7, 0)
            );
            assert_eq!(
                tree.traverse_ipv6(start, &[0; 16], 7),
                finish(start, count, 7, 0)
            );
        }
    }

    #[test]
    fn wide_root_preserves_exact_prefixes_and_cyclic_bounds() {
        let octets = [0xa5; 16];
        for depth in [1_u8, 8, 16, 19, 20, 21, 24, 32, 63, 64, 65, 96, 127, 128] {
            let count = u64::from(depth);
            let mut raw = Vec::new();
            for bit in 0..depth {
                let side = (octets[bit as usize / 8] >> (7 - bit % 8)) & 1;
                let next = if bit + 1 == depth {
                    count + 16
                } else {
                    u64::from(bit + 1)
                };
                let (left, right) = if side == 0 {
                    (next, count)
                } else {
                    (count, next)
                };
                raw.extend(node_bytes(28, left, right));
            }
            let mut tree = PreparedTree::build(&raw, 28, count, None).unwrap();
            // Force the wide fallback on a small, independently checkable tree.
            tree.byte_entries = Box::new([]);
            tree.root_bits = ACCEL_BITS_20;
            tree.root_accel = build_accel(tree.node_ptr, tree.count, 0, ACCEL_BITS_20);
            assert_eq!(tree.traverse_ipv6(0, &octets, 0), Some((count + 16, depth)));
            assert_eq!(
                tree.traverse_ipv6(0, &octets, 250),
                Some((count + 16, 250_u8.saturating_add(depth)))
            );
            let ipv4 = tree.traverse_ipv4(0, &[0xa5; 4], 0);
            assert_eq!(ipv4, (depth <= 32).then_some((count + 16, depth)));
            for bit in 0..depth {
                let mut miss = octets;
                miss[bit as usize / 8] ^= 1 << (7 - bit % 8);
                assert!(tree.traverse_ipv6(0, &miss, 0).is_none());
            }
        }
        let mut tree = PreparedTree::build(&node_bytes(32, 0, 0), 32, 1, None).unwrap();
        tree.root_bits = ACCEL_BITS_20;
        tree.root_accel = build_accel(tree.node_ptr, tree.count, 0, ACCEL_BITS_20);
        assert!(tree.traverse_ipv6(0, &octets, 0).is_none());
        assert!(tree.traverse_ipv4(0, &[0; 4], 0).is_none());
    }

    #[test]
    fn byte_stride_matches_bit_traversal_on_dags_cycles_and_early_leaves() {
        let mut seed = 0x12345678_u32;
        let mut random = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for count in [1, 17, 129, 1024] {
            let mut nodes = Vec::new();
            for i in 0..count {
                for _ in 0..2 {
                    let r = random();
                    // Include forward links, backward links, empty records,
                    // reserved pointers and data pointers: no DAG assumption.
                    nodes.push(match r % 5 {
                        0 => r % count,
                        1 => (i + 1).min(count),
                        2 => count,
                        3 => count + 1 + r % 15,
                        _ => count + 16 + r % 100,
                    });
                }
            }
            let tree = ByteTree::build(&nodes, 0).unwrap();
            for _ in 0..2048 {
                let octets: [u8; 16] = std::array::from_fn(|_| random() as u8);
                let mut node = 0_u64;
                let mut depth = 0;
                for byte in octets {
                    for bit in (0..8).rev() {
                        if node < u64::from(count) {
                            node =
                                u64::from(nodes[node as usize * 2 + ((byte >> bit) & 1) as usize]);
                            depth += 1;
                        }
                    }
                }
                assert_eq!(
                    format!(
                        "{:?}",
                        byte_walk_v6_impl(tree.entries.as_ptr(), &octets, u64::from(count), 0)
                    ),
                    format!("{:?}", finish(node, u64::from(count), 0, depth)),
                );
            }
        }
        // A fully cyclic tree consumes all 128 bits and terminates, without
        // unbounded preparation or traversal.
        let tree = ByteTree::build(&[0, 0], 0).unwrap();
        assert!(byte_walk_v6_impl(tree.entries.as_ptr(), &[0; 16], 1, 0).is_none());
    }

    #[test]
    fn byte_stride_preserves_every_prefix_depth_through_128_bits() {
        for depth in 1..=128_u32 {
            let mut nodes = Vec::new();
            let mut query = [0_u8; 16];
            for i in 0..depth {
                let next = if i + 1 == depth { depth + 16 } else { i + 1 };
                let side = i % 2;
                query[i as usize / 8] |= (side as u8) << (7 - i % 8);
                nodes.extend(if side == 0 {
                    [next, depth]
                } else {
                    [depth, next]
                });
            }
            let tree = ByteTree::build(&nodes, 0).unwrap();
            assert_eq!(
                byte_walk_v6_impl(tree.entries.as_ptr(), &query, depth as u64, 0).unwrap(),
                (u64::from(depth + 16), depth as u8)
            );
            for bit in 0..depth {
                let mut miss = query;
                miss[bit as usize / 8] ^= 1 << (7 - bit % 8);
                assert!(byte_walk_v6_impl(tree.entries.as_ptr(), &miss, depth as u64, 0).is_none());
            }
        }
    }

    #[test]
    fn byte_stride_budget_falls_back_without_partial_index() {
        assert!(ByteTree::build(&[], 0).is_none());
        assert!(ByteTree::build_limited(&[0, 0], 255, 0).is_none());
        let nodes: Vec<u32> = (0..128).flat_map(|i| [i + 1, i + 1]).collect();
        assert!(ByteTree::build_limited(&nodes, 256, 0).is_none());
        let tree = ByteTree::build(&nodes, 0).unwrap();
        assert!(byte_walk_v6_impl(tree.entries.as_ptr(), &[0; 16], 128, 0).is_none());
    }

    #[test]
    fn build_prepares_wide_nodes() {
        let tree = PreparedTree::build(&[0_u8; 10], 40, 1, None).unwrap();
        assert_eq!(tree.wide_nodes.len(), 2);
        assert_eq!(tree.traverse_ipv4(0, &[0; 4], 0), None);
    }

    #[test]
    fn build_rejects_unsupported_record_size() {
        assert!(PreparedTree::build(&[0_u8; 64], 16, 1, None).is_err());
    }

    #[test]
    fn build_rejects_short_buffer() {
        assert!(PreparedTree::build(&[0_u8; 20], 24, 10, None).is_err());
    }

    #[test]
    fn traverse_all_record_sizes() {
        for record_size in [24_u8, 28, 32] {
            let tree = build_tree(record_size);
            // Radix-8 root consumes the first byte: leading bits 00 -> node1
            // left -> data pointer 19.
            let (node, _prefix) = tree.traverse_ipv4(0, &[0; 4], 0).unwrap();
            assert_eq!(node, 19, "record_size={record_size}");
            // Leading bits 10 -> node2 -> left is no-data (== node_count).
            assert!(
                tree.traverse_ipv4(0, &[0x80, 0, 0, 0], 0).is_none(),
                "record_size={record_size}"
            );
            assert_eq!(tree.traverse_ipv6(19, &[0; 16], 0), Some((19, 0)));
            assert_eq!(tree.traverse_ipv4(1, &[0; 4], 0), Some((19, 1)));
        }
    }

    #[test]
    fn traverse_v4_and_v6_dispatch() {
        let tree = build_tree(24);
        // IPv4 path consumes the first byte through the radix-8 root table.
        let (node, _) = tree.traverse_ipv4(0, &[0; 4], 0).unwrap();
        assert_eq!(node, 19);
        // IPv6 path uses the same radix-8 root table for the first byte.
        let (node, _) = tree.traverse_ipv6(0, &[0; 16], 0).unwrap();
        assert_eq!(node, 19);
        assert!(tree.traverse_ipv6(0, &[0x80; 16], 0).is_none());
    }

    #[test]
    fn large_tree_radix16_matches_small_tree() {
        // Force the radix-16 path with a dense tree whose leaves are data
        // pointers at various depths, and compare every /16 prefix against the
        // exact walk expected from the packed records.
        let record_size = 24_u8;
        let node_count = 2_048_u64;
        let mut raw = Vec::with_capacity(node_count as usize * 6 + 16);
        for i in 0..node_count {
            let left = (i + 1) % node_count;
            let right = ((i * 7 + 3) % node_count) + node_count; // data pointer
            raw.extend_from_slice(&node_bytes(record_size, left, right));
        }
        raw.extend_from_slice(&[0_u8; 16]);
        let tree =
            PreparedTree::build(&raw, record_size, node_count, None).expect("build should succeed");
        assert_eq!(tree.root_bits, ACCEL_BITS_16);
        let nodes_ptr = tree.node_ptr;
        // Reference scalar walk from node 0.
        fn walk(nodes_ptr: *const u32, node_count: u64, key: u32) -> (u32, u8) {
            let mut node = 0_usize;
            let mut depth = 0_u8;
            for i in (0..32).rev() {
                if node as u64 >= node_count {
                    break;
                }
                let side = ((key >> i) & 1) as usize;
                // SAFETY: test harness, node bounded by node_count.
                node = unsafe { *nodes_ptr.add((node << 1) | side) } as usize;
                depth += 1;
            }
            (node as u32, depth)
        }
        for k in 0..4096_u32 {
            let expected = walk(nodes_ptr, node_count, k);
            match tree.traverse_ipv4(0, &k.to_be_bytes(), 0) {
                Some((node, depth)) => {
                    assert_eq!(
                        (node as u32, depth),
                        expected,
                        "key {k}: ended on a data pointer"
                    );
                    assert!(
                        expected.0 as u64 > node_count,
                        "key {k}: Some implies data pointer"
                    );
                }
                None => {
                    assert!(
                        expected.0 as u64 <= node_count,
                        "key {k}: None implies live/no-data end"
                    );
                }
            }
        }
    }

    #[test]
    fn accel_table_matches_scalar_walk_for_all_16bit_prefixes() {
        for record_size in [24_u8, 28, 32] {
            let (raw, node_count) = small_tree(record_size);
            let tree = PreparedTree::build(&raw, record_size, node_count, None).unwrap();
            let nodes_ptr = tree.node_ptr;
            for key in 0..=u16::MAX {
                let entry = tree.accel_entry(0, u32::from(key) << 16);
                // Reference: walk up to root_bits bits from node 0. For radix-8
                // the covered bits are the top byte of the 16-bit key.
                let k = if tree.root_bits == ACCEL_BITS_16 {
                    key
                } else {
                    key >> 8
                };
                let mut node = 0_usize;
                let mut depth = 0_u8;
                for i in (0..tree.root_bits).rev() {
                    if node as u64 >= node_count {
                        break;
                    }
                    let side = ((k >> i) & 1) as usize;
                    // SAFETY: test harness.
                    node = unsafe { *nodes_ptr.add((node << 1) | side) } as usize;
                    depth += 1;
                }
                assert_eq!(
                    (entry.node(), entry.depth()),
                    (node as u32, depth),
                    "record_size={record_size} key={key}"
                );
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn traverse_covers_all_prefetch_hints() {
        use core::arch::x86_64::{_MM_HINT_NTA, _MM_HINT_T0, _MM_HINT_T2};
        let mut tree = build_tree(24);
        for hint in [_MM_HINT_T0, _MM_HINT_T2, _MM_HINT_NTA] {
            tree.prefetch_hint = hint;
            assert!(tree.traverse_ipv4(0, &[0; 4], 0).is_some());
            assert!(tree.traverse_ipv4(0, &[0x80, 0, 0, 0], 0).is_none());
        }
    }

    #[test]
    fn debug_fmt_and_auto_traits() {
        let tree = build_tree(24);
        let text = format!("{tree:?}");
        assert!(text.contains("node_count"));
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PreparedTree>();
    }

    #[test]
    fn build_and_drop_large_tree() {
        for record_size in [24_u8, 28, 32] {
            let node_count = 1024_u64;
            let node_size = usize::from(record_size) / 4;
            let mut raw = Vec::with_capacity(1024 * node_size + 16);
            for i in 0..node_count {
                let left = (i + 1).min(node_count + 1);
                let right = (i + 2).min(node_count + 1);
                raw.extend_from_slice(&node_bytes(record_size, left, right));
            }
            raw.extend_from_slice(&[0_u8; 16]);
            let tree = PreparedTree::build(&raw, record_size, node_count, None)
                .expect("1024-node build should succeed");
            assert_eq!(tree.node_count, node_count);
        }
    }

    #[test]
    fn nonroot_walks_cross_64_bits_and_stop_at_exact_terminal_depth() {
        let count = 66_u64;
        let mut raw = Vec::new();
        for index in 0..count {
            let left = if index + 1 == count {
                count + 16
            } else {
                index + 1
            };
            raw.extend_from_slice(&node_bytes(24, left, count));
        }
        raw.extend_from_slice(&[0; 16]);
        let tree = PreparedTree::build(&raw, 24, count, None).unwrap();

        // A non-root IPv6 start uses the full scalar 128-bit walk. Its leaf
        // occurs after the first 64-bit word, in the second word.
        assert_eq!(tree.traverse_ipv6(1, &[0; 16], 0), Some((count + 16, 65)));
        assert!(tree.traverse_ipv4(1, &[0; 4], 0).is_none());
        assert_eq!(tree.traverse_ipv6(1, &[0; 16], 3), Some((count + 16, 68)));
        assert!(tree.traverse_ipv6(1, &[0xff; 16], 0).is_none());
    }

    #[test]
    fn distinct_ipv4_start_uses_its_own_accelerator() {
        let (raw, count) = small_tree(28);
        let tree = PreparedTree::build(&raw, 28, count, Some(1)).unwrap();
        assert!(!tree.ipv4_accel.is_empty());
        assert_eq!(tree.traverse_ipv4(1, &[0; 4], 0), Some((19, 1)));
        assert_eq!(tree.traverse_ipv4(0, &[0; 4], 0), Some((19, 2)));
    }
}
