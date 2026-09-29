//! Repeated A/B replay of already-built reader executables. Build every variant
//! with identical flags first; this runner never substitutes a cached executable.
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use bench_runner::{builder, config, runner};
use serde_json::{Value, json};

fn sha256(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("sha256sum").arg(path).output()?;
    if !output.status.success() {
        return Err(format!("cannot hash {}", path.display()).into());
    }
    Ok(String::from_utf8(output.stdout)?
        .split_whitespace()
        .next()
        .ok_or("empty SHA-256")?
        .into())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let out = PathBuf::from(
        args.next()
            .ok_or("usage: measure_reader OUTPUT_DIR LABEL=EXECUTABLE ...")?,
    );
    let binaries: Vec<_> = args
        .map(|arg| {
            let (label, path) = arg.split_once('=').ok_or("expected LABEL=EXECUTABLE")?;
            let path = fs::canonicalize(path)?;
            Ok::<_, Box<dyn std::error::Error>>((label.to_string(), sha256(&path)?, path))
        })
        .collect::<Result<_, _>>()?;
    if binaries.is_empty() {
        return Err("at least one executable is required".into());
    }
    let repeats: usize = env::var("BENCH_REPEATS")
        .unwrap_or_else(|_| "5".into())
        .parse()?;
    if repeats == 0 {
        return Err("BENCH_REPEATS must be positive".into());
    }
    fs::create_dir_all(&out)?;
    // Refuse to overwrite an earlier baseline.
    let mut results = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out.join("results.jsonl"))?;
    fs::write(
        out.join("environment.json"),
        serde_json::to_vec_pretty(&json!({
            "cpu": builder::cpu_model(), "os": builder::get_os_info(),
            "compilers": builder::detect_compilers(), "revision": builder::get_git_revision(),
            "binaries": binaries, "repeats": repeats,
            "rust_flags": "-C target-cpu=native -C opt-level=3; release fat LTO, codegen-units=1",
            "note": "Build flags describe the comparison build command; caller must use it for each binary."
        }))?,
    )?;
    // Hash all immutable inputs before any timing, once per scenario.
    let mut scenarios = Vec::new();
    for family in ["ipv4", "ipv6"] {
        for pattern in ["hot", "random", "sequential", "absent"] {
            let large = family == "ipv6" && pattern == "absent";
            let workloads = if large {
                config::DATA_DIR.join("ipv6-absent-v1")
            } else {
                config::WORKLOAD_DIR.to_path_buf()
            };
            let dataset = if large {
                workloads.join("ipv6.mmdb")
            } else {
                config::DATASET_PATH.to_path_buf()
            };
            let query_path = workloads.join(format!("{family}-{pattern}.txt"));
            if pattern == "hot" {
                let expected = if family == "ipv4" {
                    "81.2.69.160"
                } else {
                    "2001:db8:123::1"
                };
                let queries = fs::read_to_string(&query_path)?;
                if queries.lines().count() != 1_000_000 || !queries.lines().all(|ip| ip == expected)
                {
                    return Err("hot query file must match the harness's repeated address".into());
                }
            }
            let queries = sha256(&query_path)?;
            scenarios.push((
                family,
                pattern,
                sha256(&dataset)?,
                queries,
                dataset,
                workloads,
            ));
        }
    }
    for repeat in 0..repeats {
        for (family, pattern, dataset_hash, query_hash, dataset, workloads) in &scenarios {
            for step in 0..binaries.len() {
                let index = if repeat % 2 == 0 {
                    step
                } else {
                    binaries.len() - 1 - step
                };
                let (label, binary_hash, binary) = &binaries[index];
                let mut row = runner::run_case(
                    binary,
                    label,
                    binary_hash,
                    dataset,
                    "lookup",
                    Some(family),
                    Some(pattern),
                    1_000_000,
                    Some(1),
                    10_000,
                    25_000,
                    200,
                    repeat,
                    None,
                    Some(workloads),
                );
                row["variant"] = json!(label);
                row["binary_sha256"] = json!(binary_hash);
                row["dataset_sha256"] = json!(dataset_hash);
                row["workload_sha256"] = json!(query_hash);
                row["measurement_protocol"] = json!("reader-replay-affinity-v1");
                writeln!(results, "{row}")?;
                results.flush()?;
                if row.get("failed").and_then(Value::as_bool) == Some(true) {
                    return Err(format!("{label}/{family}/{pattern}: {}", row["error"]).into());
                }
                println!(
                    "{repeat} {family}/{pattern} {label}: {:.3} M/s, p99 {} ns",
                    row["throughput_ops_s"]
                        .as_f64()
                        .ok_or("missing throughput")?
                        / 1e6,
                    row["p99_ns"]
                );
            }
        }
    }
    Ok(())
}
