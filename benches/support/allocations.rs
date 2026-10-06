//! Count allocator requests on the measured thread, outside benchmark timing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub allocations: usize,
    pub requested_bytes: usize,
}

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static STATS: Cell<Stats> = const { Cell::new(Stats { allocations: 0, requested_bytes: 0 }) };
}

struct CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn record(bytes: usize) {
    if TRACKING.try_with(Cell::get).unwrap_or(false) {
        let _ = STATS.try_with(|stats| {
            let mut current = stats.get();
            current.allocations += 1;
            current.requested_bytes += bytes;
            stats.set(current);
        });
    }
}

// SAFETY: all allocation operations are forwarded unchanged to System. The
// thread-local counters contain only Cells and never invoke the allocator.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        // SAFETY: the caller supplies the layout required by GlobalAlloc.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        // SAFETY: the caller supplies the layout required by GlobalAlloc.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr and layout came from this System-backed allocator.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        // SAFETY: the original pointer/layout and new size are supplied by the caller.
        unsafe { System.realloc(ptr, layout, size) }
    }
}

pub fn measure<T>(operation: impl FnOnce() -> T) -> (T, Stats) {
    struct StopTracking;
    impl Drop for StopTracking {
        fn drop(&mut self) {
            TRACKING.with(|tracking| tracking.set(false));
        }
    }
    STATS.with(|stats| stats.set(Stats::default()));
    TRACKING.with(|tracking| tracking.set(true));
    let guard = StopTracking;
    let result = operation();
    drop(guard);
    (result, STATS.with(Cell::get))
}
