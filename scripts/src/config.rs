use std::path::PathBuf;
use std::sync::LazyLock;

pub static ROOT: LazyLock<PathBuf> = LazyLock::new(|| {
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let p = PathBuf::from(manifest_dir);
        if p.ends_with("scripts") {
            return p.parent().unwrap_or(&p).to_path_buf();
        }
        return p;
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
});

pub static TARGET: LazyLock<PathBuf> = LazyLock::new(|| ROOT.join("target"));
pub static DATA_DIR: LazyLock<PathBuf> = LazyLock::new(|| TARGET.join("bench-data"));
pub static WORKLOAD_DIR: LazyLock<PathBuf> = LazyLock::new(|| DATA_DIR.join("workloads"));
pub static SCALING_DIR: LazyLock<PathBuf> = LazyLock::new(|| DATA_DIR.join("scaling"));
pub static DATASET_PATH: LazyLock<PathBuf> =
    LazyLock::new(|| DATA_DIR.join("GeoIP2-City-Bench.mmdb"));

pub static BIN_DIR: LazyLock<PathBuf> = LazyLock::new(|| TARGET.join("benchmark-bin"));
pub static BUILD_DIR: LazyLock<PathBuf> = LazyLock::new(|| TARGET.join("benchmark-build"));
pub static DEPS_DIR: LazyLock<PathBuf> = LazyLock::new(|| TARGET.join("bench-deps"));

pub static RESULTS_PATH: LazyLock<PathBuf> =
    LazyLock::new(|| TARGET.join("benchmark-results").join("results.jsonl"));
pub static RESULTS_JSON_PATH: LazyLock<PathBuf> =
    LazyLock::new(|| TARGET.join("benchmark-results").join("results.json"));
pub static RESULTS_CSV_PATH: LazyLock<PathBuf> =
    LazyLock::new(|| TARGET.join("benchmark-results").join("results.csv"));

pub static REPORT_DIR: LazyLock<PathBuf> = LazyLock::new(|| ROOT.join("benchmark-report"));
pub static REPORT_PATH: LazyLock<PathBuf> = LazyLock::new(|| REPORT_DIR.join("index.html"));
pub static CHARTS_DIR: LazyLock<PathBuf> = LazyLock::new(|| ROOT.join("benchmarks").join("charts"));
pub static DOCS_CHARTS_DIR: LazyLock<PathBuf> =
    LazyLock::new(|| ROOT.join("docs").join("images").join("benchmarks"));
pub static README_PATH: LazyLock<PathBuf> = LazyLock::new(|| ROOT.join("README.md"));

pub fn cpu_pinning_enabled() -> bool {
    std::env::var("BENCH_PIN").unwrap_or_else(|_| "1".into()) == "1"
}

pub const IMPLEMENTATIONS: &[&str] = &[
    "libmaxminddb-rs",
    "libmaxminddb",
    "maxminddb-rust",
    "geoip2-rs",
    "maxminddb-golang",
];

/// Readers covered by the per-size resident-memory (RSS) benchmark. The eight
/// requested sizes apply to every one of these libraries; nothing else is
/// measured so an unavailable point can never be silently read as zero.
pub const MEMORY_IMPLEMENTATIONS: &[&str] = &[
    "libmaxminddb-rs",
    "libmaxminddb",
    "maxminddb-rust",
    "geoip2-rs",
];

/// Protocol tag recorded by the memory harnesses and preserved by the runner.
pub const MEMORY_PROTOCOL: &str = "memory-rss-v2";

pub const WRITER_IMPLEMENTATIONS: &[&str] = &["libmaxminddb-rs", "mmdbwriter"];

/// Human-readable name; result keys keep their original implementation identifier.
pub fn display_name(impl_name: &str) -> &str {
    match impl_name {
        "libmaxminddb" => "libmaxminddb (C)",
        _ => impl_name,
    }
}

pub fn get_color(impl_name: &str) -> &'static str {
    match impl_name {
        "lookup_borrowed" => "#ff8800",
        "lookup_borrowed_map" => "#00f5a0",
        "lookup_borrowed_opt" => "#f5c542",
        "lookup_value" => "#00add8",
        "lookup_value_with_prefix" => "#4986f5",
        "lookup (serde_json::Value)" => "#ff3b5c",
        "lookup_many(1)" => "#e39d5c",
        "lookup_exists" => "#d0e85e",
        "libmaxminddb-rs" => "#00f5a0",
        "libmaxminddb" => "#f5c542",
        "maxminddb-rust" => "#ff8800",
        "geoip2-rs" => "#ff3b5c",
        "maxminddb-golang" => "#00add8",
        "mmdbwriter" => "#a06ee1",
        _ => "#8b949e",
    }
}

/// Shared by dataset generation, measurements and every size-scaling report.
pub const SCALING_DATABASE_SIZES: &[usize] = &[
    1_000, 10_000, 100_000, 500_000, 1_000_000, 1_500_000, 2_000_000, 5_000_000,
];
pub const SCALING_LOOKUP_IPS_COUNT: usize = 1_000;
pub const DEFAULT_RANDOM_SEED: &str = "0x5EED2024CAFEBABE";

pub const WRITER_SIZES: &[usize] = SCALING_DATABASE_SIZES;
pub const DEFAULT_WRITER_BATCH_SIZE: usize = 5_000;

pub const DEFAULT_LOOKUP_SIZES: &[usize] = &[1_000_000];
pub const DEFAULT_LOOKUP_PATTERNS: &[&str] = &["random", "sequential", "hot", "absent"];
pub const DEFAULT_FAMILIES: &[&str] = &["ipv4", "ipv6"];
pub const DEFAULT_THREAD_COUNTS: &[usize] = &[1, 4, 8, 16];
/// Extra variants run for our reader after the default `lookup_borrowed_map`.
/// Every variant uses one address per operation in the same concurrent harness.
pub const LOOKUP_API_SCENARIOS: &[(&str, &str)] = &[
    ("lookup_concurrent_borrowed", "lookup_borrowed"),
    ("lookup_concurrent_borrowed_opt", "lookup_borrowed_opt"),
    ("lookup_concurrent_value", "lookup_value"),
    ("lookup_concurrent_value_prefix", "lookup_value_with_prefix"),
    ("lookup_concurrent_serde", "lookup (serde_json::Value)"),
    ("lookup_concurrent_many", "lookup_many(1)"),
    ("lookup_concurrent_exists", "lookup_exists"),
];
pub const LOOKUP_API_OPERATIONS: &[&str] = &[
    "lookup_borrowed_map",
    "lookup_borrowed",
    "lookup_borrowed_opt",
    "lookup_value",
    "lookup_value_with_prefix",
    "lookup (serde_json::Value)",
    "lookup_many(1)",
    "lookup_exists",
];
pub const DEFAULT_WARMUP: usize = 10_000;
pub const DEFAULT_LATENCY_SAMPLES: usize = 25_000;
pub const DEFAULT_OPEN_ITERS: usize = 200;
pub const DEFAULT_COLD_SAMPLES: usize = 30;
