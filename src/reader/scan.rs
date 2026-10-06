//! Checked streaming traversal and bounded parallel subtree scheduling.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use super::Reader;
use crate::{Error, IpNetwork, MmdbDecode, Result};

// Repeated scalar scans crossed over by ~24k nodes with eight workers, including
// scoped-thread setup. Smaller trees remain sensitive to scheduling overhead.
const PARALLEL_MIN_NODES: u64 = 24_000;
const MAX_WORKERS: usize = 256;
const TASKS_PER_WORKER: usize = 8;
const BUDGET_BLOCK: u64 = 256;
type Entry = (u64, u128, u8);

struct Task {
    entry: Entry,
    ancestors: [u64; 128],
}

/// Each worker reserves credits in blocks, never exceeding the scan's total
/// budget. Credits stay local across tasks, so no shared atomic is needed per
/// entry. Unused credits are conservative slack, not additional work allowance.
struct Credits<'a> {
    shared: &'a AtomicU64,
    local: u64,
}

impl Credits<'_> {
    fn consume(&mut self) -> Result<()> {
        if self.local == 0 {
            let mut remaining = self.shared.load(Ordering::Relaxed);
            loop {
                if remaining == 0 {
                    return Err(budget_error());
                }
                let block = remaining.min(BUDGET_BLOCK);
                match self.shared.compare_exchange_weak(
                    remaining,
                    remaining - block,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        self.local = block;
                        break;
                    }
                    Err(actual) => remaining = actual,
                }
            }
        }
        self.local -= 1;
        Ok(())
    }
}

fn budget_error() -> Error {
    Error::ResourceLimit("MMDB scan tree-entry budget exceeded")
}

impl Reader<'_> {
    /// Visits borrowed records, using scoped worker threads for large trees.
    ///
    /// The callback is `Fn + Sync` and can run concurrently. Order is not
    /// guaranteed, including on machines where this method uses the sequential
    /// fallback. Strings and bytes borrow this reader; records are decoded and
    /// consumed on the same worker, so `T` need not be `Send` or `Sync`.
    ///
    /// Trees with fewer than 24,000 nodes, or only one available CPU, use
    /// [`Self::visit_borrowed_records`]. Otherwise the worker limit comes from
    /// `std::thread::available_parallelism`, capped at 256. No Rayon dependency
    /// or complete record list is required: a bounded frontier of subtrees is
    /// shared and each worker traverses/decodes locally.
    /// This size heuristic is based on synthetic scalar scans; the crossover
    /// depends on the machine, tree shape, schema and callback. Use the explicit
    /// worker-limit variant to tune a particular workload.
    ///
    /// Address-family rules and pointer/cycle/depth checks match the sequential
    /// scan. The whole scan shares its `256 * (node_count + 1)` entry budget;
    /// block reservations can reject heavily aliased trees slightly earlier.
    /// Scheduling and thread creation allocate bounded memory; borrowed scalar
    /// decoding does not allocate per record.
    ///
    /// On a callback, decoding, traversal or thread-creation error, cancellation
    /// is cooperative: other callbacks may run before workers observe it. All
    /// started workers finish before returning an error. If several fail, the
    /// returned error is one of those failures, with no ordering guarantee.
    /// Collected results are partial on failure. Callback panics propagate after
    /// joining the workers, and also trigger cooperative cancellation.
    ///
    /// Requires only `reader`; deriving `MmdbDecode` additionally needs `derive`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "derive")]
    /// # fn main() -> libmaxminddb_rs::Result<()> {
    /// use libmaxminddb_rs::{MmdbDecode, Reader};
    /// use std::sync::atomic::{AtomicUsize, Ordering};
    /// #[derive(MmdbDecode)]
    /// struct Record<'a> { category: &'a str }
    /// let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/doc.mmdb"));
    /// let reader = Reader::from_bytes(bytes)?;
    /// let count = AtomicUsize::new(0);
    /// reader.visit_borrowed_records_parallel(|_, record: Record<'_>| {
    ///     assert!(!record.category.is_empty());
    ///     count.fetch_add(1, Ordering::Relaxed);
    ///     Ok(())
    /// })?;
    /// assert!(count.into_inner() > 0);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "derive"))] fn main() {}
    /// ```
    pub fn visit_borrowed_records_parallel<'s, T>(
        &'s self,
        visitor: impl Fn(IpNetwork, T) -> Result<()> + Sync,
    ) -> Result<()>
    where
        T: MmdbDecode<'s>,
    {
        let workers = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        if workers <= 1 || self.metadata.node_count < PARALLEL_MIN_NODES {
            return self.visit_borrowed_records(visitor);
        }
        self.parallel_scan(workers, &visitor)
    }

    /// Visits borrowed records with an explicit worker limit.
    ///
    /// Same behavior as [`Self::visit_borrowed_records_parallel`], but bypasses
    /// its tree-size/CPU heuristic. `workers = 1` uses the sequential scan.
    /// The limit is capped at 256 and at the number of frontier tasks. Useful
    /// for controlling resource use and measuring the parallel crossover.
    pub fn visit_borrowed_records_parallel_with_workers<'s, T>(
        &'s self,
        workers: NonZeroUsize,
        visitor: impl Fn(IpNetwork, T) -> Result<()> + Sync,
    ) -> Result<()>
    where
        T: MmdbDecode<'s>,
    {
        if workers.get() == 1 {
            return self.visit_borrowed_records(visitor);
        }
        self.parallel_scan(workers.get(), &visitor)
    }

    fn parallel_scan<'s, T, F>(&'s self, workers: usize, visitor: &F) -> Result<()>
    where
        T: MmdbDecode<'s>,
        F: Fn(IpNetwork, T) -> Result<()> + Sync,
    {
        let workers = workers.min(MAX_WORKERS);
        let mut remaining = self.scan_budget();
        let tasks = self.scan_frontier(workers * TASKS_PER_WORKER, &mut remaining)?;
        if tasks.is_empty() {
            return Ok(());
        }
        let budget = AtomicU64::new(remaining);
        let cancelled = AtomicBool::new(false);
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(workers.min(tasks.len()));
            let mut error = None;
            for _ in 0..workers.min(tasks.len()) {
                let spawn = std::thread::Builder::new().spawn_scoped(scope, || {
                    // Also cancel when a callback unwinds. Scoped threads are
                    // joined before the panic leaves the public method.
                    struct CancelOnDrop<'a>(&'a AtomicBool);
                    impl Drop for CancelOnDrop<'_> {
                        fn drop(&mut self) {
                            self.0.store(true, Ordering::Relaxed);
                        }
                    }
                    let cancel_on_drop = CancelOnDrop(&cancelled);
                    let mut credits = Credits {
                        shared: &budget,
                        local: 0,
                    };
                    let result = (|| {
                        loop {
                            if cancelled.load(Ordering::Relaxed) {
                                break;
                            }
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(task) = tasks.get(index) else { break };
                            self.visit_offsets_from(
                                task.entry,
                                task.ancestors,
                                || {
                                    if cancelled.load(Ordering::Relaxed) {
                                        return Ok(false);
                                    }
                                    credits.consume()?;
                                    Ok(true)
                                },
                                |network, offset| {
                                    let mut decoder = crate::decoder::RawDecoder::new(
                                        self.source.bytes(),
                                        self.data_section_start,
                                        self.metadata_marker,
                                        offset,
                                    );
                                    visitor(network, T::decode_raw(&mut decoder)?)
                                },
                            )?;
                        }
                        Ok(())
                    })();
                    if result.is_ok() {
                        // Successful completion must not cancel other workers.
                        std::mem::forget(cancel_on_drop);
                    }
                    result
                });
                match spawn {
                    Ok(handle) => handles.push(handle),
                    Err(io) => {
                        cancelled.store(true, Ordering::Relaxed);
                        error = Some(Error::Io(io));
                        break;
                    }
                }
            }
            let mut panic = None;
            for handle in handles {
                match handle.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(failure)) => {
                        if error.is_none() {
                            error = Some(failure);
                        }
                    }
                    Err(payload) => {
                        if panic.is_none() {
                            panic = Some(payload);
                        }
                    }
                }
            }
            if let Some(payload) = panic {
                std::panic::resume_unwind(payload);
            }
            error.map_or(Ok(()), Err)
        })
    }

    fn scan_budget(&self) -> u64 {
        self.metadata
            .node_count
            .saturating_add(1)
            .saturating_mul(256)
    }

    fn scan_bits(&self) -> u8 {
        if self.metadata.ip_version == 4 {
            32
        } else {
            128
        }
    }

    fn check_scan_node(&self, node: u64, depth: u8, ancestors: &[u64; 128]) -> Result<()> {
        if ancestors[..usize::from(depth)].contains(&node) {
            return Err(Error::InvalidDatabase("cyclic search tree"));
        }
        if depth >= self.scan_bits() {
            return Err(Error::InvalidDatabase("search tree exceeds address width"));
        }
        Ok(())
    }

    /// Split shallow internal tasks first. Empty branches are discarded, so a
    /// long common prefix does not prevent parallelism in the populated subtree.
    /// Every task retains the full ancestor prefix for cross-frontier cycles.
    fn scan_frontier(&self, target: usize, remaining: &mut u64) -> Result<Vec<Task>> {
        let mut tasks = Vec::with_capacity(target);
        tasks.push(Task {
            entry: (0, 0, 0),
            ancestors: [0; 128],
        });
        while tasks.len() < target {
            let Some(index) = tasks
                .iter()
                .enumerate()
                .filter(|(_, task)| task.entry.0 < self.metadata.node_count)
                .min_by_key(|(_, task)| task.entry.2)
                .map(|(index, _)| index)
            else {
                break;
            };
            let mut task = tasks.swap_remove(index);
            if *remaining == 0 {
                return Err(budget_error());
            }
            *remaining -= 1;
            let (node, prefix, depth) = task.entry;
            self.check_scan_node(node, depth, &task.ancestors)?;
            task.ancestors[usize::from(depth)] = node;
            let left = self.read_record(node, 0)?;
            let right = self.read_record(node, 1)?;
            let shift = u32::from(self.scan_bits() - depth - 1);
            for (child, child_prefix) in [(left, prefix), (right, prefix | (1_u128 << shift))] {
                if child == self.metadata.node_count {
                    // Skipped no-data pointers still consume the global budget.
                    if *remaining == 0 {
                        return Err(budget_error());
                    }
                    *remaining -= 1;
                } else {
                    tasks.push(Task {
                        entry: (child, child_prefix, depth + 1),
                        ancestors: task.ancestors,
                    });
                }
            }
        }
        Ok(tasks)
    }

    pub(super) fn visit_record_offsets(
        &self,
        visitor: impl FnMut(IpNetwork, usize) -> Result<()>,
    ) -> Result<()> {
        let mut remaining = self.scan_budget();
        self.visit_offsets_from(
            (0, 0, 0),
            [0; 128],
            || {
                if remaining == 0 {
                    return Err(budget_error());
                }
                remaining -= 1;
                Ok(true)
            },
            visitor,
        )
    }

    /// One fixed DFS stack and ancestor array per worker. Generic closures keep
    /// sequential accounting local and inline, with no atomic or heap overhead.
    fn visit_offsets_from(
        &self,
        root: Entry,
        mut ancestors: [u64; 128],
        mut before_entry: impl FnMut() -> Result<bool>,
        mut visitor: impl FnMut(IpNetwork, usize) -> Result<()>,
    ) -> Result<()> {
        let bits = self.scan_bits();
        let mut stack = [(0_u64, 0_u128, 0_u8); 129];
        stack[0] = root;
        let mut stack_len = 1;
        while stack_len != 0 {
            if !before_entry()? {
                return Ok(());
            }
            stack_len -= 1;
            let (node, prefix, depth) = stack[stack_len];
            if node >= self.metadata.node_count {
                if node == self.metadata.node_count {
                    continue;
                }
                let offset = self.record_to_file_offset(node)?;
                let network = if bits == 32 {
                    IpNetwork::new(IpAddr::V4(Ipv4Addr::from(prefix as u32)), depth)
                } else if depth >= 96 && prefix >> 32 == 0 {
                    IpNetwork::new(IpAddr::V4(Ipv4Addr::from(prefix as u32)), depth - 96)
                } else {
                    IpNetwork::new(IpAddr::V6(Ipv6Addr::from(prefix)), depth)
                }
                .map_err(|_| Error::InvalidDatabase("invalid scanned network prefix"))?;
                visitor(network, offset)?;
                continue;
            }
            self.check_scan_node(node, depth, &ancestors)?;
            ancestors[usize::from(depth)] = node;
            let left = self.read_record(node, 0)?;
            let right = self.read_record(node, 1)?;
            let shift = u32::from(bits - depth - 1);
            stack[stack_len] = (right, prefix | (1_u128 << shift), depth + 1);
            stack[stack_len + 1] = (left, prefix, depth + 1);
            stack_len += 2;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_budget_never_multiplies_with_workers_or_loses_the_partial_block() {
        for initial in [0, 1, 255, 256, 257, 1069] {
            let shared = AtomicU64::new(initial);
            let consumed = AtomicU64::new(0);
            std::thread::scope(|scope| {
                for _ in 0..8 {
                    scope.spawn(|| {
                        let mut credits = Credits {
                            shared: &shared,
                            local: 0,
                        };
                        while credits.consume().is_ok() {
                            consumed.fetch_add(1, Ordering::Relaxed);
                        }
                    });
                }
            });
            assert_eq!(consumed.into_inner(), initial);
            assert_eq!(shared.into_inner(), 0);
        }
    }
}
