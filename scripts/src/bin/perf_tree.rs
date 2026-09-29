use std::env;
use std::process::Command;

use bench_runner::config::{BIN_DIR, ROOT};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = env::var("PERF_TREE_MODE").unwrap_or_else(|_| "simd-eager-tree".into());
    let filter = env::var("PERF_FILTER").unwrap_or_else(|_| "lookup/tree".into());
    let iters = env::var("PERF_ITERS").unwrap_or_else(|_| "250000".into());
    let repeats = env::var("PERF_STAT_REPEATS").unwrap_or_else(|_| "5".into());
    let default_events = "cycles,instructions,branches,branch-misses,cache-references,cache-misses,L1-dcache-loads,L1-dcache-load-misses";
    let events = env::var("PERF_EVENTS").unwrap_or_else(|_| default_events.into());
    let cpu = env::var("PERF_CPU").ok();

    let bench_bin = BIN_DIR.join("libmaxminddb-rs");
    if !bench_bin.exists() {
        eprintln!(
            "⚠️ Binary not found at {}. Building first...",
            bench_bin.display()
        );
        bench_runner::builder::build_rust_binary(
            "libmaxminddb-rs",
            "-C target-cpu=native -C opt-level=3",
        )?;
    }

    let mut perf_cmd = Vec::new();
    if let Some(c) = cpu {
        perf_cmd.extend(["taskset".to_string(), "-c".to_string(), c]);
    }
    perf_cmd.extend([
        "perf".to_string(),
        "stat".to_string(),
        "-r".to_string(),
        repeats,
        "-e".to_string(),
        events,
        "--".to_string(),
        bench_bin.to_string_lossy().to_string(),
    ]);

    println!("📊 Profiling mode={mode} filter={filter} iters={iters} with perf stat...");
    let mut child = Command::new(&perf_cmd[0]);
    child.args(&perf_cmd[1..]);
    child.current_dir(&*ROOT);
    child.env("PERF_FILTER", &filter);
    child.env("PERF_ITERS", &iters);

    let status = child.status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
