#[cfg(feature = "geoip2-rs")]
use memmap2::MmapOptions;
use serde::Serialize;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::env;
use std::fs::{self, File};
use std::hint::black_box;
// io traits unused
use memmap2::Mmap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Barrier;
use std::time::{Duration, Instant};

struct CountingAllocator;

// Thread-local counters avoid turning allocator instrumentation into a shared lock.
thread_local! {
    static ALLOCATIONS: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

fn count_allocation(bytes: usize) {
    let _ = ALLOCATIONS.try_with(|counter| {
        let (calls, total) = counter.get();
        counter.set((calls + 1, total + bytes as u64));
    });
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation(layout.size());
        // SAFETY: forwarding the exact layout received from GlobalAlloc to System.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocation(layout.size());
        // SAFETY: forwarding the exact layout received from GlobalAlloc to System.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr/layout originate from this allocator's System allocation.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count_allocation(new_size);
        // SAFETY: ptr/layout originate from this allocator and new_size is forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Ipv4,
    Ipv6,
}

impl Family {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ipv4" => Ok(Self::Ipv4),
            "ipv6" => Ok(Self::Ipv6),
            _ => Err(format!(
                "unsupported BENCH_FAMILY={value:?}; expected ipv4 or ipv6"
            )),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pattern {
    Hot,
    Sequential,
    Random,
    Absent,
}

impl Pattern {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "hot" => Ok(Self::Hot),
            "sequential" => Ok(Self::Sequential),
            "random" => Ok(Self::Random),
            "absent" => Ok(Self::Absent),
            _ => Err(format!(
                "unsupported BENCH_PATTERN={value:?}; expected hot, sequential, random or absent"
            )),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Sequential => "sequential",
            Self::Random => "random",
            Self::Absent => "absent",
        }
    }
}

#[derive(Debug)]
struct Config {
    dataset: PathBuf,
    workload_dir: PathBuf,
    scenario: String,
    family: Option<Family>,
    pattern: Option<Pattern>,
    workload_size: usize,
    warmup_ops: usize,
    latency_samples_max: usize,
    open_iterations: usize,
    revision: String,
    threads: usize,
    memory_lookups: usize,
    warmup_duration: Duration,
    measurement_duration: Duration,
}

impl Config {
    fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let scenario = env::var("BENCH_SCENARIO").unwrap_or_else(|_| "lookup".into());
        let family = match env::var("BENCH_FAMILY") {
            Ok(value) if !value.is_empty() => Some(Family::parse(&value)?),
            _ => None,
        };
        let pattern = match env::var("BENCH_PATTERN") {
            Ok(value) if !value.is_empty() => Some(Pattern::parse(&value)?),
            _ => None,
        };
        let parse_usize =
            |name: &str, default: usize| -> Result<usize, Box<dyn std::error::Error>> {
                Ok(env::var(name)
                    .ok()
                    .map(|v| v.parse::<usize>())
                    .transpose()?
                    .unwrap_or(default))
            };
        let workload_size = parse_usize("BENCH_WORKLOAD_SIZE", 100_000)?;
        let warmup_ops = parse_usize("BENCH_WARMUP_OPS", 10_000)?;
        let latency_samples_max = parse_usize("BENCH_LATENCY_SAMPLES_MAX", 100_000)?;
        let open_iterations = parse_usize("BENCH_OPEN_ITERS", 200)?;
        let threads = env::var("BENCH_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&v| v > 0)
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1)
            });
        let memory_lookups = parse_usize("BENCH_MEM_LOOKUPS", 1_000_000)?;
        let warmup_duration = Duration::from_secs(parse_usize("BENCH_WARMUP_SECS", 1)? as u64);
        let measurement_duration =
            Duration::from_secs(parse_usize("BENCH_MEASURE_SECS", 2)? as u64);
        let requires_workload = scenario.starts_with("lookup");
        let requires_open = scenario.starts_with("open");
        if (requires_workload && workload_size == 0)
            || (requires_open && open_iterations == 0)
            || latency_samples_max == 0
        {
            return Err("benchmark iteration counts must be greater than zero".into());
        }
        Ok(Self {
            dataset: env::var_os("BENCH_MMDB")
                .map(PathBuf::from)
                .ok_or("BENCH_MMDB is required")?,
            workload_dir: env::var_os("BENCH_WORKLOAD_DIR")
                .map(PathBuf::from)
                .ok_or("BENCH_WORKLOAD_DIR is required")?,
            scenario,
            family,
            pattern,
            workload_size,
            warmup_ops,
            latency_samples_max,
            open_iterations,
            revision: env::var("BENCH_IMPLEMENTATION_REVISION")
                .unwrap_or_else(|_| "unknown".into()),
            threads,
            memory_lookups,
            warmup_duration,
            measurement_duration,
        })
    }
}

#[inline]
fn parse_ip_bytes(bytes: &[u8]) -> Option<IpAddr> {
    if let Some(v4) = parse_ipv4_bytes(bytes) {
        return Some(IpAddr::V4(v4));
    }
    std::str::from_utf8(bytes)
        .ok()?
        .trim()
        .parse::<IpAddr>()
        .ok()
}

#[inline]
fn parse_ipv4_bytes(bytes: &[u8]) -> Option<std::net::Ipv4Addr> {
    let mut octets = [0u8; 4];
    let mut octet_idx = 0;
    let mut val = 0u16;
    let mut has_digit = false;

    for &b in bytes {
        if b.is_ascii_digit() {
            val = val * 10 + (b - b'0') as u16;
            if val > 255 {
                return None;
            }
            has_digit = true;
        } else if b == b'.' {
            if !has_digit || octet_idx >= 3 {
                return None;
            }
            octets[octet_idx] = val as u8;
            octet_idx += 1;
            val = 0;
            has_digit = false;
        } else if b == b'\r' || b == b' ' {
            continue;
        } else {
            return None;
        }
    }
    if !has_digit || octet_idx != 3 {
        return None;
    }
    octets[3] = val as u8;
    Some(std::net::Ipv4Addr::from(octets))
}

#[derive(Debug)]
enum Workload {
    Hot { ip: IpAddr, len: usize },
    List(Vec<IpAddr>),
}

impl Workload {
    fn load(cfg: &Config) -> Result<Self, Box<dyn std::error::Error>> {
        let family = cfg.family.ok_or("BENCH_FAMILY is required for lookup")?;
        let pattern = cfg.pattern.ok_or("BENCH_PATTERN is required for lookup")?;
        if pattern == Pattern::Hot {
            let name = match family {
                Family::Ipv4 => "BENCH_HOT_IPV4",
                Family::Ipv6 => "BENCH_HOT_IPV6",
            };
            let default = match family {
                Family::Ipv4 => "81.2.69.160",
                Family::Ipv6 => "2001:db8:123::1",
            };
            let ip: IpAddr = env::var(name).unwrap_or_else(|_| default.into()).parse()?;
            return Ok(Self::Hot {
                ip,
                len: cfg.workload_size,
            });
        }

        let filename = format!("{}-{}.txt", family.as_str(), pattern.as_str());
        let path = cfg.workload_dir.join(filename);
        let file = File::open(&path)?;

        // OPTIMIZATION: Use memory-mapped file for faster parsing
        // This avoids the overhead of BufReader and String allocations
        let mmap = unsafe { Mmap::map(&file)? };
        let mut ips = Vec::with_capacity(cfg.workload_size);

        // Parse IPs directly from mmap'd memory
        let mut start = 0;
        let bytes = &mmap;
        let len = bytes.len();

        while start < len && ips.len() < cfg.workload_size {
            let rel_end = memchr::memchr(b'\n', &bytes[start..]).unwrap_or(len - start);
            let end = start + rel_end;
            let line = &bytes[start..end];
            if let Some(ip) = parse_ip_bytes(line) {
                ips.push(ip);
            }
            start = end + 1;
        }

        if ips.len() != cfg.workload_size {
            return Err(format!(
                "{} contains {} IPs but {} were requested",
                path.display(),
                ips.len(),
                cfg.workload_size
            )
            .into());
        }
        Ok(Self::List(ips))
    }

    fn len(&self) -> usize {
        match self {
            Self::Hot { len, .. } => *len,
            Self::List(ips) => ips.len(),
        }
    }

    #[inline(always)]
    fn get(&self, index: usize) -> IpAddr {
        match self {
            Self::Hot { ip, .. } => *ip,
            Self::List(ips) => {
                // index is always reduced modulo len() by callers.
                ips[index]
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct BenchResult {
    schema_version: u32,
    implementation: &'static str,
    version: &'static str,
    revision: String,
    scenario: String,
    operation: &'static str,
    threads: usize,
    family: Option<&'static str>,
    pattern: Option<&'static str>,
    workload_size: usize,
    measurement_operations: usize,
    warmup_ops: usize,
    latency_sample_count: usize,
    mean_ns: f64,
    median_ns: f64,
    min_ns: u64,
    max_ns: u64,
    p50_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
    variance_ns2: f64,
    stddev_ns: f64,
    throughput_ops_s: f64,
    wall_time_ns: u64,
    cpu_time_ns: u64,
    cpu_utilization_pct: f64,
    allocation_calls: u64,
    allocations_per_op: f64,
    allocated_bytes: u64,
    allocated_bytes_per_op: f64,
    rss_before_bytes: u64,
    rss_after_bytes: u64,
    rss_delta_bytes: i64,
    peak_rss_bytes: u64,
    first_operation_ns: u64,
    timer_overhead_ns: f64,
    dataset_bytes: u64,
    checksum: u64,
}

#[derive(Debug)]
struct Stats {
    mean: f64,
    median: f64,
    min: u64,
    max: u64,
    p50: u64,
    p95: u64,
    p99: u64,
    variance: f64,
    stddev: f64,
}

fn percentile(sorted: &[u64], q: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[index]
}

fn summarize(mut samples: Vec<u64>) -> Stats {
    samples.sort_unstable();
    let n = samples.len() as f64;
    let mean = samples.iter().map(|&v| v as f64).sum::<f64>() / n;
    let variance = samples
        .iter()
        .map(|&v| {
            let delta = v as f64 - mean;
            delta * delta
        })
        .sum::<f64>()
        / n;
    Stats {
        mean,
        median: percentile(&samples, 0.50) as f64,
        min: samples[0],
        max: *samples.last().unwrap_or(&0),
        p50: percentile(&samples, 0.50),
        p95: percentile(&samples, 0.95),
        p99: percentile(&samples, 0.99),
        variance,
        stddev: variance.sqrt(),
    }
}

#[inline]
fn process_cpu_ns() -> u64 {
    #[cfg(unix)]
    unsafe {
        let mut ts: libc::timespec = std::mem::zeroed();
        if libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) == 0 {
            return (ts.tv_sec as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(ts.tv_nsec as u64);
        }
    }
    0
}

fn current_rss_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(statm) = fs::read_to_string("/proc/self/statm") {
            if let Some(rss_pages) = statm
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.parse::<u64>().ok())
            {
                // SAFETY: sysconf is a side-effect-free libc query.
                let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
                if page_size > 0 {
                    return rss_pages.saturating_mul(page_size as u64);
                }
            }
        }
    }
    peak_rss_bytes()
}

fn peak_rss_bytes() -> u64 {
    #[cfg(unix)]
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0 {
            #[cfg(target_os = "macos")]
            return usage.ru_maxrss as u64;
            #[cfg(not(target_os = "macos"))]
            return (usage.ru_maxrss as u64).saturating_mul(1024);
        }
    }
    0
}

fn timer_overhead_ns() -> f64 {
    const SAMPLES: usize = 10_000;
    let mut total = 0_u128;
    for _ in 0..SAMPLES {
        let start = Instant::now();
        black_box(());
        total += start.elapsed().as_nanos();
    }
    total as f64 / SAMPLES as f64
}

fn reset_allocations() {
    ALLOCATIONS.with(|counter| counter.set((0, 0)));
}

fn allocation_snapshot() -> (u64, u64) {
    ALLOCATIONS.with(Cell::get)
}

fn make_result(
    implementation: &'static str,
    version: &'static str,
    cfg: &Config,
    operation: &'static str,
    threads: usize,
    family: Option<Family>,
    pattern: Option<Pattern>,
    workload_size: usize,
    measurement_operations: usize,
    warmup_ops: usize,
    latency_samples: Vec<u64>,
    throughput_wall_ns: u64,
    throughput_cpu_ns: u64,
    allocations: u64,
    allocated_bytes: u64,
    rss_before: u64,
    rss_after: u64,
    peak_rss: u64,
    first_operation_ns: u64,
    checksum: u64,
) -> BenchResult {
    let stats = summarize(latency_samples);
    let throughput = if throughput_wall_ns == 0 {
        0.0
    } else {
        measurement_operations as f64 * 1_000_000_000.0 / throughput_wall_ns as f64
    };
    let cpu_utilization = if throughput_wall_ns == 0 {
        0.0
    } else {
        throughput_cpu_ns as f64 / throughput_wall_ns as f64 * 100.0
    };
    BenchResult {
        schema_version: 2,
        implementation,
        version,
        revision: cfg.revision.clone(),
        scenario: cfg.scenario.clone(),
        operation,
        threads,
        family: family.map(Family::as_str),
        pattern: pattern.map(Pattern::as_str),
        workload_size,
        measurement_operations,
        warmup_ops,
        latency_sample_count: stats_count(workload_size, cfg.latency_samples_max, operation),
        mean_ns: stats.mean,
        median_ns: stats.median,
        min_ns: stats.min,
        max_ns: stats.max,
        p50_ns: stats.p50,
        p95_ns: stats.p95,
        p99_ns: stats.p99,
        variance_ns2: stats.variance,
        stddev_ns: stats.stddev,
        throughput_ops_s: throughput,
        wall_time_ns: throughput_wall_ns,
        cpu_time_ns: throughput_cpu_ns,
        cpu_utilization_pct: cpu_utilization,
        allocation_calls: allocations,
        allocations_per_op: allocations as f64 / measurement_operations.max(1) as f64,
        allocated_bytes,
        allocated_bytes_per_op: allocated_bytes as f64 / measurement_operations.max(1) as f64,
        rss_before_bytes: rss_before,
        rss_after_bytes: rss_after,
        rss_delta_bytes: (rss_after as i128)
            .saturating_sub(rss_before as i128)
            .clamp(i64::MIN as i128, i64::MAX as i128) as i64,
        peak_rss_bytes: peak_rss,
        first_operation_ns,
        timer_overhead_ns: timer_overhead_ns(),
        dataset_bytes: fs::metadata(&cfg.dataset).map(|m| m.len()).unwrap_or(0),
        checksum,
    }
}

fn stats_count(workload_size: usize, max_samples: usize, operation: &str) -> usize {
    if operation == "open" {
        workload_size
    } else {
        workload_size.min(max_samples)
    }
}

fn benchmark_lookup<F>(
    implementation: &'static str,
    version: &'static str,
    cfg: &Config,
    workload: &Workload,
    mut op: F,
) -> BenchResult
where
    F: FnMut(IpAddr) -> u64,
{
    let size = workload.len();
    let first_ip = workload.get(0);
    let first_start = Instant::now();
    let first_checksum = black_box(op(first_ip));
    let first_operation_ns = first_start.elapsed().as_nanos() as u64;

    let warm_start = Instant::now();
    let mut warm_checksum = first_checksum;
    let mut warmup_ops = 0usize;
    while warm_start.elapsed() < cfg.warmup_duration {
        warm_checksum ^= black_box(op(workload.get(warmup_ops % size)));
        warmup_ops += 1;
    }
    black_box(warm_checksum);

    let sample_count = size.min(cfg.latency_samples_max);
    let mut samples = Vec::with_capacity(sample_count);
    for i in 0..sample_count {
        let index = if sample_count == size {
            i
        } else {
            i.saturating_mul(size) / sample_count
        };
        let start = Instant::now();
        let value = black_box(op(workload.get(index.min(size - 1))));
        black_box(value);
        samples.push(start.elapsed().as_nanos() as u64);
    }

    let rss_before = current_rss_bytes();
    reset_allocations();
    let cpu_start = process_cpu_ns();
    let wall_start = Instant::now();
    let mut checksum = 0_u64;
    let mut measurement_operations = 0usize;
    while wall_start.elapsed() < cfg.measurement_duration || measurement_operations == 0 {
        for i in 0..size {
            checksum = checksum.rotate_left(1) ^ black_box(op(workload.get(i)));
            measurement_operations += 1;
        }
    }
    black_box(checksum);
    let wall_ns = wall_start.elapsed().as_nanos() as u64;
    let cpu_ns = process_cpu_ns().saturating_sub(cpu_start);
    let (allocations, allocated_bytes) = allocation_snapshot();
    let rss_after = current_rss_bytes();
    let peak_rss = peak_rss_bytes();

    make_result(
        implementation,
        version,
        cfg,
        "lookup_decode",
        1,
        cfg.family,
        cfg.pattern,
        size,
        measurement_operations,
        warmup_ops,
        samples,
        wall_ns,
        cpu_ns,
        allocations,
        allocated_bytes,
        rss_before,
        rss_after,
        peak_rss,
        first_operation_ns,
        checksum,
    )
}

fn benchmark_open<F>(
    implementation: &'static str,
    version: &'static str,
    cfg: &Config,
    mut op: F,
) -> BenchResult
where
    F: FnMut() -> u64,
{
    let iterations = cfg.open_iterations;

    let first_start = Instant::now();
    let first_checksum = black_box(op());
    let first_operation_ns = first_start.elapsed().as_nanos() as u64;

    // OPTIMIZATION: Skip warmup loop entirely if warmup is 0
    let warm_start = Instant::now();
    let mut warm_checksum = first_checksum;
    let mut warmup_ops = 0usize;
    while warm_start.elapsed() < cfg.warmup_duration {
        warm_checksum ^= black_box(op());
        warmup_ops += 1;
    }
    black_box(warm_checksum);

    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let value = black_box(op());
        black_box(value);
        samples.push(start.elapsed().as_nanos() as u64);
    }

    let rss_before = current_rss_bytes();
    reset_allocations();
    let cpu_start = process_cpu_ns();
    let wall_start = Instant::now();
    let mut checksum = 0_u64;
    let mut measurement_operations = 0usize;
    while wall_start.elapsed() < cfg.measurement_duration || measurement_operations == 0 {
        checksum = checksum.rotate_left(1) ^ black_box(op());
        measurement_operations += 1;
    }
    black_box(checksum);
    let wall_ns = wall_start.elapsed().as_nanos() as u64;
    let cpu_ns = process_cpu_ns().saturating_sub(cpu_start);
    let (allocations, allocated_bytes) = allocation_snapshot();
    let rss_after = current_rss_bytes();
    let peak_rss = peak_rss_bytes();

    make_result(
        implementation,
        version,
        cfg,
        "open",
        1,
        None,
        None,
        iterations,
        measurement_operations,
        warmup_ops,
        samples,
        wall_ns,
        cpu_ns,
        allocations,
        allocated_bytes,
        rss_before,
        rss_after,
        peak_rss,
        first_operation_ns,
        checksum,
    )
}

#[derive(Debug, Serialize)]
struct MemoryBenchResult {
    schema_version: u32,
    implementation: &'static str,
    version: &'static str,
    revision: String,
    scenario: &'static str,
    operation: &'static str,
    threads: usize,
    family: &'static str,
    pattern: &'static str,
    workload_size: usize,
    lookups: usize,
    warmup_ops: usize,
    hits: usize,
    misses: usize,
    pid: u32,
    auxiliary_bytes: usize,
    rss_before_open_bytes: u64,
    rss_after_open_bytes: u64,
    rss_peak_bytes: Option<u64>,
    rss_after_bytes: u64,
    peak_unavailable_reason: Option<String>,
    opening_mode: &'static str,
    measurement_protocol: &'static str,
    rss_method: &'static str,
    checksum: u64,
    dataset_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum SelectedOutput {
    Bench(BenchResult),
    Memory(MemoryBenchResult),
}

fn memory_rss_snapshot() -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let text = fs::read_to_string("/proc/self/status")?;
    let read = |key: &str| -> Result<u64, Box<dyn std::error::Error>> {
        let line = text
            .lines()
            .find(|line| line.starts_with(key))
            .ok_or("Missing RSS field")?;
        let mut fields = line.split_whitespace().skip(1);
        let bytes = fields
            .next()
            .ok_or("Missing RSS value")?
            .parse::<u64>()?
            .checked_mul(1024)
            .ok_or("RSS overflow")?;
        if fields.next() != Some("kB") || bytes == 0 {
            return Err("Invalid RSS measurement".into());
        }
        Ok(bytes)
    };
    Ok((read("VmRSS:")?, read("VmHWM:")?))
}

type MemoryLookup = Box<dyn FnMut(IpAddr) -> Result<bool, String>>;

/// Each invocation is a fresh exec. The fixed binary query buffer is loaded
/// before the pre-open RSS sample; its exact capacity is reported separately.
/// Reset Linux VmHWM after opening, before any lookup (including warmup), so
/// temporary allocations during open cannot be mislabeled as a lookup peak.
fn benchmark_memory_rss(
    implementation: &'static str,
    version: &'static str,
    cfg: &Config,
    open: impl FnOnce() -> Result<MemoryLookup, Box<dyn std::error::Error>>,
) -> Result<MemoryBenchResult, Box<dyn std::error::Error>> {
    let queries = fs::read(cfg.workload_dir.join("memory-queries.bin"))?;
    let lookups = cfg.memory_lookups;
    if lookups != 1_000_000 || queries.len() != lookups * 4 || cfg.warmup_ops != 1_000 {
        return Err(
            "memory-rss-v2 requires exactly 1M binary queries and 1000 warmup lookups".into(),
        );
    }
    let rss_before_open = memory_rss_snapshot()?.0;
    let mut op = open()?;
    let rss_after_open = memory_rss_snapshot()?.0;
    let peak_unavailable_reason = fs::write("/proc/self/clear_refs", b"5\n")
        .err()
        .map(|e| format!("Cannot reset VmHWM after open: {e}"));
    let mut checksum = 0u64;
    let mut hits = 0;
    let mut misses = 0;
    for (index, bytes) in queries
        .chunks_exact(4)
        .take(cfg.warmup_ops)
        .chain(queries.chunks_exact(4))
        .enumerate()
    {
        let raw = u32::from_be_bytes(bytes.try_into()?);
        let hit = op(IpAddr::V4(std::net::Ipv4Addr::from(black_box(raw))))?;
        if hit != (raw & 1 == 0) {
            return Err(format!("Unexpected memory-workload result for {raw:#x}").into());
        }
        if index >= cfg.warmup_ops {
            hits += usize::from(hit);
            misses += usize::from(!hit);
            checksum = checksum.rotate_left(1) ^ u64::from(raw) ^ u64::from(hit);
        }
    }
    black_box(checksum);
    let (rss_after, peak) = memory_rss_snapshot()?;
    black_box((&queries, &op));
    Ok(MemoryBenchResult {
        schema_version: 2,
        implementation,
        version,
        revision: cfg.revision.clone(),
        scenario: "memory_rss",
        operation: "lookup_decode",
        threads: 1,
        family: "ipv4",
        pattern: "mixed_50_50",
        workload_size: lookups,
        lookups,
        warmup_ops: cfg.warmup_ops,
        hits,
        misses,
        pid: std::process::id(),
        auxiliary_bytes: queries.capacity(),
        rss_before_open_bytes: rss_before_open,
        rss_after_open_bytes: rss_after_open,
        rss_peak_bytes: peak_unavailable_reason.is_none().then_some(peak),
        rss_after_bytes: rss_after,
        peak_unavailable_reason,
        opening_mode: "mmap",
        measurement_protocol: "memory-rss-v2",
        rss_method: "linux-vmrss-vmhwm-reset-after-open",
        checksum,
        dataset_bytes: fs::metadata(&cfg.dataset)?.len(),
    })
}

fn benchmark_lookup_concurrent<F>(
    implementation: &'static str,
    version: &'static str,
    cfg: &Config,
    workload: &Workload,
    op: F,
) -> BenchResult
where
    F: Fn(IpAddr) -> u64 + Sync + Send,
{
    let threads = cfg.threads.max(1);
    let size = workload.len();
    let first_ip = workload.get(0);
    let first_start = Instant::now();
    let first_checksum = black_box(op(first_ip));
    let first_operation_ns = first_start.elapsed().as_nanos() as u64;

    // Warmup across all threads
    let warmup_start = Instant::now();
    // Leave a small allowance for starting all workers after the clock begins.
    let warmup_deadline = warmup_start + cfg.warmup_duration + Duration::from_millis(10);
    let warmup_counts = std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(threads);
        for t in 0..threads {
            let op_ref = &op;
            let offset = t * size / threads;
            handles.push(s.spawn(move || {
                let mut count = 0usize;
                let mut warm_checksum = first_checksum;
                while Instant::now() < warmup_deadline {
                    warm_checksum ^= black_box(op_ref(workload.get((offset + count) % size)));
                    count += 1;
                }
                black_box(warm_checksum);
                count
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or(0))
            .sum::<usize>()
    });

    // Sample the same evenly spaced addresses for every implementation, with
    // simultaneous workers. Allocation counters are local to each worker.
    let sample_count = size.min(cfg.latency_samples_max);
    let mut samples = vec![0_u64; sample_count];
    let sample_chunk = sample_count.div_ceil(threads);
    let sample_workers = sample_count.div_ceil(sample_chunk);
    let sample_barrier = Barrier::new(sample_workers);
    std::thread::scope(|scope| {
        for (t, out) in samples.chunks_mut(sample_chunk).enumerate() {
            let op = &op;
            let barrier = &sample_barrier;
            scope.spawn(move || {
                barrier.wait();
                for (i, sample) in out.iter_mut().enumerate() {
                    let ip = workload.get((t * sample_chunk + i) * size / sample_count);
                    let start = Instant::now();
                    black_box(op(ip));
                    *sample = start.elapsed().as_nanos() as u64;
                }
            });
        }
    });

    let rss_before = current_rss_bytes();
    let barrier = Barrier::new(threads + 1);
    let chunk_size = size.div_ceil(threads);
    let mut worker_results = vec![(0_u64, 0_u64, 0_u64, 0usize); threads];
    let measurement_deadline =
        Instant::now() + cfg.measurement_duration + Duration::from_millis(10);
    let (wall_ns, cpu_ns) = std::thread::scope(|scope| {
        for (t, result) in worker_results.iter_mut().enumerate() {
            let barrier = &barrier;
            let op = &op;
            scope.spawn(move || {
                let start_idx = (t * chunk_size).min(size);
                let end_idx = ((t + 1) * chunk_size).min(size);
                reset_allocations();
                barrier.wait(); // Ready: excludes thread creation from timing.
                barrier.wait(); // Go: clock starts before workers are released.
                let mut checksum = 0_u64;
                let chunk_len = end_idx.saturating_sub(start_idx).max(1);
                let mut count = 0usize;
                while Instant::now() < measurement_deadline || count == 0 {
                    let index = start_idx + count % chunk_len;
                    checksum = checksum.rotate_left(1) ^ black_box(op(workload.get(index)));
                    count += 1;
                }
                let (calls, bytes) = allocation_snapshot();
                *result = (checksum, calls, bytes, count);
                barrier.wait(); // Done: excludes thread teardown from timing.
            });
        }
        barrier.wait();
        let cpu_start = process_cpu_ns();
        let wall_start = Instant::now();
        barrier.wait();
        barrier.wait();
        (
            wall_start.elapsed().as_nanos() as u64,
            process_cpu_ns().saturating_sub(cpu_start),
        )
    });
    let allocations = worker_results.iter().map(|r| r.1).sum();
    let allocated_bytes = worker_results.iter().map(|r| r.2).sum();
    let measurement_operations = worker_results.iter().map(|r| r.3).sum();
    let rss_after = current_rss_bytes();
    let peak_rss = peak_rss_bytes();

    // Throughput workers execute different numbers of operations as each API
    // variant has a different speed. Validate every variant over the same
    // deterministic query sequence, outside the measured window, so checksum
    // comparisons verify results rather than encode throughput.
    let mut checksum = 0_u64;
    for index in 0..size {
        checksum = checksum.rotate_left(1) ^ black_box(op(workload.get(index)));
    }
    black_box(checksum);

    make_result(
        implementation,
        version,
        cfg,
        "lookup_decode",
        threads,
        cfg.family,
        cfg.pattern,
        size,
        measurement_operations,
        warmup_counts,
        samples,
        wall_ns,
        cpu_ns,
        allocations,
        allocated_bytes,
        rss_before,
        rss_after,
        peak_rss,
        first_operation_ns,
        checksum,
    )
}

fn checksum_ip(ip: IpAddr) -> u64 {
    match ip {
        IpAddr::V4(v4) => u32::from(v4) as u64,
        IpAddr::V6(v6) => {
            let value = u128::from(v6);
            (value as u64) ^ ((value >> 64) as u64)
        }
    }
}

#[cfg(feature = "ours")]
mod city;

#[cfg(feature = "ours")]
fn benchmark_ours_lookup_api<const MODE: u8>(
    cfg: &Config,
    operation: &'static str,
) -> Result<SelectedOutput, Box<dyn std::error::Error>> {
    let workload = Workload::load(cfg)?;
    // SAFETY: the benchmark owns the file and never modifies/truncates it while mapped.
    let reader = unsafe { libmaxminddb_rs::Reader::open_mmap(&cfg.dataset)? };
    // MODE is constant at each call site, so dispatch is outside the measured
    // operation after monomorphization. Every mode consumes one identical IP.
    let mut result =
        benchmark_lookup_concurrent("libmaxminddb-rs", "local", cfg, &workload, |ip| {
            let ip = black_box(ip);
            let present = match MODE {
                0 => reader
                    .lookup_borrowed_map(ip, |city: city::CityRecord<'_>| {
                        black_box(city);
                        1_u64
                    })
                    .ok()
                    .flatten()
                    .unwrap_or(0),
                1 => match city::lookup(&reader, ip) {
                    Ok(city) => {
                        black_box(city);
                        1
                    }
                    Err(_) => 0,
                },
                2 => match reader.lookup_borrowed_opt::<city::CityRecord<'_>>(ip) {
                    Some(city) => {
                        black_box(city);
                        1
                    }
                    None => 0,
                },
                3 => match reader.lookup_value(ip) {
                    Ok(value) => {
                        black_box(value);
                        1
                    }
                    Err(_) => 0,
                },
                4 => match reader.lookup_value_with_prefix(ip) {
                    Ok(value) => {
                        black_box(value);
                        1
                    }
                    Err(_) => 0,
                },
                6 => match reader.lookup::<serde_json::Value>(ip) {
                    Ok(value) => {
                        black_box(value);
                        1
                    }
                    Err(_) => 0,
                },
                7 => match reader.lookup_many(&[ip]).into_iter().next() {
                    Some(Ok(value)) => {
                        black_box(value);
                        1
                    }
                    _ => 0,
                },
                9 => u64::from(reader.lookup_exists(ip)),
                _ => unreachable!("unknown lookup benchmark mode"),
            };
            present ^ checksum_ip(ip)
        });
    result.operation = operation;
    Ok(SelectedOutput::Bench(result))
}

#[cfg(feature = "ours")]
fn run_selected(cfg: &Config) -> Result<SelectedOutput, Box<dyn std::error::Error>> {
    const IMPL: &str = "libmaxminddb-rs";
    const VERSION: &str = "local";
    match cfg.scenario.as_str() {
        "memory_rss" => Ok(SelectedOutput::Memory(benchmark_memory_rss(
            IMPL,
            VERSION,
            cfg,
            move || {
                // SAFETY: prepared fixtures are immutable throughout this process.
                let reader = unsafe { libmaxminddb_rs::Reader::open_mmap(&cfg.dataset)? };
                Ok(Box::new(move |ip| {
                    match city::lookup(&reader, black_box(ip)) {
                        Ok(record) => {
                            black_box(record);
                            Ok(true)
                        }
                        Err(libmaxminddb_rs::Error::NotFound) => Ok(false),
                        Err(error) => Err(error.to_string()),
                    }
                }))
            },
        )?)),
        "lookup" => {
            let workload = Workload::load(cfg)?;
            // SAFETY: the benchmark owns the file and never modifies/truncates it while mapped.
            let reader = unsafe { libmaxminddb_rs::Reader::open_mmap(&cfg.dataset)? };
            if cfg.pattern == Some(Pattern::Absent) {
                for i in 0..workload.len() {
                    match reader.lookup_value(workload.get(i)) {
                        Err(libmaxminddb_rs::Error::NotFound) => {}
                        other => return Err(format!("absent preflight failed: {other:?}").into()),
                    }
                }
            }
            let mut result = benchmark_lookup(IMPL, VERSION, cfg, &workload, |ip| {
                // Mirror the miss-path shape used by the maxminddb-rust and
                // geoip2-rs harnesses: the decoded value is only black_boxed on
                // a hit. black_boxing a 256-byte `Option<CityRecord>` on every
                // miss forces a dead copy that the competitors' ops never pay,
                // skewing the absent/random categories against us.
                reader
                    .lookup_borrowed_map(black_box(ip), |city: city::CityRecord<'_>| {
                        let p = u64::from(city.city.is_some());
                        black_box(city);
                        p
                    })
                    .ok()
                    .flatten()
                    .unwrap_or(0)
            });
            result.operation = "lookup_borrowed_map";
            Ok(SelectedOutput::Bench(result))
        }
        "lookup_concurrent" | "lookup_concurrent_map" => {
            benchmark_ours_lookup_api::<0>(cfg, "lookup_borrowed_map")
        }
        "lookup_concurrent_borrowed" => benchmark_ours_lookup_api::<1>(cfg, "lookup_borrowed"),
        "lookup_concurrent_borrowed_opt" => {
            benchmark_ours_lookup_api::<2>(cfg, "lookup_borrowed_opt")
        }
        "lookup_concurrent_value" => benchmark_ours_lookup_api::<3>(cfg, "lookup_value"),
        "lookup_concurrent_value_prefix" => {
            benchmark_ours_lookup_api::<4>(cfg, "lookup_value_with_prefix")
        }
        "lookup_concurrent_serde" => {
            benchmark_ours_lookup_api::<6>(cfg, "lookup (serde_json::Value)")
        }
        "lookup_concurrent_many" => benchmark_ours_lookup_api::<7>(cfg, "lookup_many(1)"),
        "lookup_concurrent_exists" => benchmark_ours_lookup_api::<9>(cfg, "lookup_exists"),
        "open_file" => Ok(SelectedOutput::Bench(benchmark_open(
            IMPL,
            VERSION,
            cfg,
            || {
                let reader = libmaxminddb_rs::Reader::open(black_box(&cfg.dataset)).unwrap();
                let result = reader.metadata().node_count;
                black_box(reader);
                result
            },
        ))),
        "open_buffer" => {
            let bytes = fs::read(&cfg.dataset)?;
            Ok(SelectedOutput::Bench(benchmark_open(
                IMPL,
                VERSION,
                cfg,
                || {
                    let reader =
                        libmaxminddb_rs::Reader::from_bytes(black_box(bytes.as_slice())).unwrap();
                    let result = reader.metadata().node_count;
                    black_box(reader);
                    result
                },
            )))
        }
        "open_mmap" => Ok(SelectedOutput::Bench(benchmark_open(
            IMPL,
            VERSION,
            cfg,
            || {
                // SAFETY: benchmark fixture is immutable for the lifetime of each mapping.
                let reader =
                    unsafe { libmaxminddb_rs::Reader::open_mmap(black_box(&cfg.dataset)).unwrap() };
                let result = reader.metadata().node_count;
                black_box(reader);
                result
            },
        ))),
        other => Err(format!("unsupported scenario for {IMPL}: {other}").into()),
    }
}

#[cfg(all(not(feature = "ours"), feature = "maxminddb-rust"))]
fn run_selected(cfg: &Config) -> Result<SelectedOutput, Box<dyn std::error::Error>> {
    const IMPL: &str = "maxminddb-rust";
    const VERSION: &str = "0.32.0";
    match cfg.scenario.as_str() {
        "memory_rss" => Ok(SelectedOutput::Memory(benchmark_memory_rss(
            IMPL,
            VERSION,
            cfg,
            move || {
                // SAFETY: prepared fixtures are immutable throughout this process.
                let reader = unsafe { maxminddb::Reader::open_mmap(&cfg.dataset)? };
                Ok(Box::new(move |ip| {
                    let lookup = reader.lookup(black_box(ip)).map_err(|e| e.to_string())?;
                    let record = lookup
                        .decode::<maxminddb::geoip2::City>()
                        .map_err(|e| e.to_string())?;
                    Ok(match record {
                        Some(record) => {
                            black_box(record);
                            true
                        }
                        None => false,
                    })
                }))
            },
        )?)),
        "lookup" => {
            let workload = Workload::load(cfg)?;
            // SAFETY: benchmark fixture is immutable while the mapping exists.
            let reader = unsafe { maxminddb::Reader::open_mmap(&cfg.dataset)? };
            if cfg.pattern == Some(Pattern::Absent) {
                for i in 0..workload.len() {
                    if reader
                        .lookup(workload.get(i))?
                        .decode::<maxminddb::geoip2::City>()?
                        .is_some()
                    {
                        return Err("absent preflight found a record".into());
                    }
                }
            }
            Ok(SelectedOutput::Bench(benchmark_lookup(
                IMPL,
                VERSION,
                cfg,
                &workload,
                |ip| {
                    let present = match reader.lookup(black_box(ip)) {
                        Ok(lookup) => match lookup.decode::<maxminddb::geoip2::City>() {
                            Ok(city) => {
                                let p = city.is_some() as u64;
                                black_box(city);
                                p
                            }
                            Err(_) => 0,
                        },
                        Err(_) => 0,
                    };
                    present ^ checksum_ip(ip)
                },
            )))
        }
        "lookup_concurrent" => {
            let workload = Workload::load(cfg)?;
            // SAFETY: benchmark fixture is immutable while the mapping exists.
            let reader = unsafe { maxminddb::Reader::open_mmap(&cfg.dataset)? };
            Ok(SelectedOutput::Bench(benchmark_lookup_concurrent(
                IMPL,
                VERSION,
                cfg,
                &workload,
                |ip| {
                    let present = match reader.lookup(black_box(ip)) {
                        Ok(lookup) => match lookup.decode::<maxminddb::geoip2::City>() {
                            Ok(city) => {
                                let p = city.is_some() as u64;
                                black_box(city);
                                p
                            }
                            Err(_) => 0,
                        },
                        Err(_) => 0,
                    };
                    present ^ checksum_ip(ip)
                },
            )))
        }
        "open_file" => Ok(SelectedOutput::Bench(benchmark_open(
            IMPL,
            VERSION,
            cfg,
            || {
                let reader = maxminddb::Reader::open_readfile(black_box(&cfg.dataset)).unwrap();
                let result = reader.metadata().node_count as u64;
                black_box(reader);
                result
            },
        ))),
        "open_buffer" => {
            let bytes = fs::read(&cfg.dataset)?;
            Ok(SelectedOutput::Bench(benchmark_open(
                IMPL,
                VERSION,
                cfg,
                || {
                    let reader =
                        maxminddb::Reader::from_source(black_box(bytes.as_slice())).unwrap();
                    let result = reader.metadata().node_count as u64;
                    black_box(reader);
                    result
                },
            )))
        }
        "open_mmap" => Ok(SelectedOutput::Bench(benchmark_open(
            IMPL,
            VERSION,
            cfg,
            || {
                // SAFETY: benchmark fixture is immutable while the mapping exists.
                let reader =
                    unsafe { maxminddb::Reader::open_mmap(black_box(&cfg.dataset)).unwrap() };
                let result = reader.metadata().node_count as u64;
                black_box(reader);
                result
            },
        ))),
        other => Err(format!("unsupported scenario for {IMPL}: {other}").into()),
    }
}

#[cfg(all(
    not(feature = "ours"),
    not(feature = "maxminddb-rust"),
    feature = "geoip2-rs"
))]
fn run_selected(cfg: &Config) -> Result<SelectedOutput, Box<dyn std::error::Error>> {
    const IMPL: &str = "geoip2-rs";
    const VERSION: &str = "0.1.8";
    match cfg.scenario.as_str() {
        "memory_rss" => Ok(SelectedOutput::Memory(benchmark_memory_rss(
            IMPL,
            VERSION,
            cfg,
            move || {
                let file = File::open(&cfg.dataset)?;
                // SAFETY: prepared fixtures are immutable for the lifetime of the map.
                let mmap = unsafe { MmapOptions::new().map(&file)? };
                // This standalone process owns exactly one mapping until exit. Leaking
                // the owner gives geoip2's borrowed Reader a stable static lifetime.
                let mmap: &'static memmap2::Mmap = Box::leak(Box::new(mmap));
                let reader = geoip2::Reader::<geoip2::City>::from_bytes(mmap.as_ref())
                    .map_err(|e| format!("geoip2 open: {e:?}"))?;
                Ok(Box::new(move |ip| match reader.lookup(black_box(ip)) {
                    Ok(record) => {
                        black_box(record);
                        Ok(true)
                    }
                    Err(geoip2::Error::NotFound) => Ok(false),
                    Err(error) => Err(format!("geoip2 decode: {error:?}")),
                }))
            },
        )?)),
        "lookup" => {
            let workload = Workload::load(cfg)?;
            let file = File::open(&cfg.dataset)?;
            // SAFETY: benchmark fixture is immutable while the mapping exists.
            let mmap = unsafe { MmapOptions::new().map(&file)? };
            let reader = geoip2::Reader::<geoip2::City>::from_bytes(mmap.as_ref())
                .map_err(|e| format!("geoip2 open: {e:?}"))?;
            if cfg.pattern == Some(Pattern::Absent) {
                for i in 0..workload.len() {
                    match reader.lookup(workload.get(i)) {
                        Err(geoip2::Error::NotFound) => {}
                        _ => return Err("absent preflight expected NotFound".into()),
                    }
                }
            }
            Ok(SelectedOutput::Bench(benchmark_lookup(
                IMPL,
                VERSION,
                cfg,
                &workload,
                |ip| {
                    let present = match reader.lookup(black_box(ip)) {
                        Ok(city) => {
                            black_box(city);
                            1
                        }
                        Err(_) => 0,
                    };
                    present ^ checksum_ip(ip)
                },
            )))
        }
        "lookup_concurrent" => {
            let workload = Workload::load(cfg)?;
            let file = File::open(&cfg.dataset)?;
            // SAFETY: benchmark fixture is immutable while the mapping exists.
            let mmap = unsafe { MmapOptions::new().map(&file)? };
            let reader = geoip2::Reader::<geoip2::City>::from_bytes(mmap.as_ref())
                .map_err(|e| format!("geoip2 open: {e:?}"))?;
            Ok(SelectedOutput::Bench(benchmark_lookup_concurrent(
                IMPL,
                VERSION,
                cfg,
                &workload,
                |ip| {
                    let present = match reader.lookup(black_box(ip)) {
                        Ok(city) => {
                            black_box(city);
                            1
                        }
                        Err(_) => 0,
                    };
                    present ^ checksum_ip(ip)
                },
            )))
        }
        "open_file" => Ok(SelectedOutput::Bench(benchmark_open(
            IMPL,
            VERSION,
            cfg,
            || {
                let bytes = fs::read(black_box(&cfg.dataset)).unwrap();
                let reader = geoip2::Reader::<geoip2::City>::from_bytes(bytes.as_slice()).unwrap();
                black_box(reader);
                drop(bytes);
                1
            },
        ))),
        "open_buffer" => {
            let bytes = fs::read(&cfg.dataset)?;
            Ok(SelectedOutput::Bench(benchmark_open(
                IMPL,
                VERSION,
                cfg,
                || {
                    let reader =
                        geoip2::Reader::<geoip2::City>::from_bytes(black_box(bytes.as_slice()))
                            .unwrap();
                    black_box(reader);
                    1
                },
            )))
        }
        "open_mmap" => Ok(SelectedOutput::Bench(benchmark_open(
            IMPL,
            VERSION,
            cfg,
            || {
                let file = File::open(black_box(&cfg.dataset)).unwrap();
                // SAFETY: benchmark fixture is immutable while the mapping exists.
                let mmap = unsafe { MmapOptions::new().map(&file).unwrap() };
                let reader = geoip2::Reader::<geoip2::City>::from_bytes(mmap.as_ref()).unwrap();
                black_box(reader);
                drop(mmap);
                drop(file);
                1
            },
        ))),
        other => Err(format!("unsupported scenario for {IMPL}: {other}").into()),
    }
}

#[cfg(not(any(feature = "ours", feature = "maxminddb-rust", feature = "geoip2-rs")))]
fn run_selected(_: &Config) -> Result<SelectedOutput, Box<dyn std::error::Error>> {
    Err("enable exactly one feature: ours, maxminddb-rust or geoip2-rs".into())
}

#[cfg(any(
    all(feature = "ours", feature = "maxminddb-rust"),
    all(feature = "ours", feature = "geoip2-rs"),
    all(feature = "maxminddb-rust", feature = "geoip2-rs")
))]
compile_error!("enable exactly one benchmark implementation feature");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::from_env()?;
    if !cfg.dataset.is_file() {
        return Err(format!(
            "benchmark database does not exist: {}",
            cfg.dataset.display()
        )
        .into());
    }
    let result = run_selected(&cfg)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

#[cfg(test)]
mod instrumentation_tests {
    use super::*;

    #[test]
    #[cfg(feature = "ours")]
    fn comparison_lookups_borrow_strings_for_both_address_families() {
        use libmaxminddb_rs::{MetadataBuilder, Reader, Writer};
        let mut writer =
            Writer::with_metadata(MetadataBuilder::new().ip_version(6).build().unwrap());
        let record = serde_json::json!({
            "city": {"geoname_id": 123, "names": {"en": "Paris"}},
            "country": {"iso_code": "FR"},
            "subdivisions": [{"iso_code": "IDF"}],
            "location": {"latitude": 48.85, "longitude": 2.35}
        });
        for network in ["10.1.2.0/24", "2001:db8::/48"] {
            writer.insert(network.parse().unwrap(), &record).unwrap();
        }
        let data = writer.finish().unwrap();
        let reader = Reader::from_bytes(&data).unwrap();
        for address in ["10.1.2.3", "2001:db8::1"] {
            let ip = address.parse().unwrap();
            drop(city::lookup(&reader, ip).unwrap()); // warm lookup and decoder paths
            reset_allocations();
            let decoded = city::lookup(&reader, ip).unwrap();
            let (calls, _) = allocation_snapshot();
            assert_eq!(calls, 1, "only the requested subdivisions Vec allocates");
            let name = decoded.city.unwrap().names.unwrap().en.unwrap();
            assert_eq!(name, "Paris");
            assert!(
                (data.as_ptr() as usize..data.as_ptr() as usize + data.len())
                    .contains(&(name.as_ptr() as usize))
            );
            assert_eq!(decoded.country.unwrap().iso_code, Some("FR"));
            assert_eq!(decoded.subdivisions.unwrap()[0].iso_code, Some("IDF"));
            assert_eq!(decoded.location.unwrap().latitude, Some(48.85));
        }
        for address in ["192.0.2.1", "3001::1"] {
            reset_allocations();
            let result = city::lookup(&reader, address.parse().unwrap());
            assert_eq!(allocation_snapshot(), (0, 0));
            assert!(matches!(result, Err(libmaxminddb_rs::Error::NotFound)));
        }
    }

    #[test]
    fn allocation_counters_are_independent_across_workers() {
        let barrier = Barrier::new(4);
        std::thread::scope(|scope| {
            let mut workers = Vec::new();
            for worker in 1..=4 {
                let barrier = &barrier;
                workers.push(scope.spawn(move || {
                    reset_allocations();
                    barrier.wait();
                    for _ in 0..worker * 100 {
                        let layout = Layout::from_size_align(64, 8).unwrap();
                        // SAFETY: allocate and release the same valid layout;
                        // the pointer is checked before it is deallocated.
                        unsafe {
                            let ptr = GLOBAL_ALLOCATOR.alloc(layout);
                            assert!(!ptr.is_null());
                            black_box(ptr);
                            GLOBAL_ALLOCATOR.dealloc(ptr, layout);
                        }
                    }
                    barrier.wait();
                    assert_eq!(allocation_snapshot(), (worker * 100, worker * 6400));
                }));
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
    }
}
