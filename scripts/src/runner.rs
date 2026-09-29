use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use crate::builder::{build_all_binaries, build_writer_binaries};
use crate::config::{
    DATASET_PATH, DEFAULT_FAMILIES, DEFAULT_LATENCY_SAMPLES, DEFAULT_LOOKUP_PATTERNS,
    DEFAULT_LOOKUP_SIZES, DEFAULT_RANDOM_SEED, DEFAULT_THREAD_COUNTS, DEFAULT_WARMUP,
    DEFAULT_WRITER_BATCH_SIZE, IMPLEMENTATIONS, LOOKUP_API_SCENARIOS, RESULTS_PATH, ROOT,
    SCALING_DATABASE_SIZES, SCALING_DIR, SCALING_LOOKUP_IPS_COUNT, WORKLOAD_DIR, WRITER_SIZES,
};
use crate::metrics::{
    fmt_bytes, fmt_duration, fmt_rate, fmt_size_tag, style_bold, style_cyan, style_gray,
    style_winner, supports_color,
};
use crate::workloads::{
    build_scaling_tool, generate_scaling_db, generate_scaling_workload, prepare_dataset,
    prepare_workloads,
};

pub fn write_result(row: &Value) {
    if let Some(p) = RESULTS_PATH.parent() {
        let _ = fs::create_dir_all(p);
    }
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&*RESULTS_PATH)
    {
        use std::io::Write;
        let _ = writeln!(file, "{}", serde_json::to_string(row).unwrap_or_default());
    }
}

// Apply affinity in the child only. Concurrent runs retain the caller's CPU set.
// Using sched_getaffinity also works inside cpusets where CPU 0 is unavailable.
fn pin_lookup_process(cmd: &mut Command, threads: usize) -> Result<Option<usize>, String> {
    if threads != 1 || !crate::config::cpu_pinning_enabled() {
        return Ok(None);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: cpu_set_t is a plain integer bitset; all-zero is valid.
        let mut allowed: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        // SAFETY: allowed is writable for the size passed; pid 0 means this process.
        if unsafe { libc::sched_getaffinity(0, std::mem::size_of_val(&allowed), &mut allowed) } != 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let available = |cpu: usize| {
            cpu < libc::CPU_SETSIZE as usize
                // SAFETY: cpu is within cpu_set_t and allowed was initialized above.
                && unsafe { libc::CPU_ISSET(cpu, &allowed) }
        };
        let cpu = match std::env::var("BENCH_CPU") {
            Ok(cpu) => cpu
                .parse::<usize>()
                .map_err(|e| format!("BENCH_CPU: {e}"))?,
            Err(_) => (0..libc::CPU_SETSIZE as usize)
                .find(|&cpu| available(cpu))
                .ok_or("empty CPU affinity")?,
        };
        if !available(cpu) {
            return Err(format!("BENCH_CPU={cpu} is outside the allowed CPU set"));
        }
        // SAFETY: the bitset is initialized and cpu is in range. The pre_exec
        // closure only calls sched_setaffinity and constructs an OS error; it
        // performs no allocation or locking after fork.
        unsafe {
            let mut selected: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_SET(cpu, &mut selected);
            cmd.pre_exec(move || {
                if libc::sched_setaffinity(0, std::mem::size_of_val(&selected), &selected) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
        return Ok(Some(cpu));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cmd;
        Ok(None)
    }
}

pub fn run_case(
    binary: &Path,
    implementation: &str,
    revision: &str,
    dataset: &Path,
    scenario: &str,
    family: Option<&str>,
    pattern: Option<&str>,
    size: usize,
    threads: Option<usize>,
    warmup: usize,
    latency_samples: usize,
    open_iters: usize,
    repeat: usize,
    label_scenario: Option<&str>,
    workload_dir: Option<&Path>,
) -> Value {
    let mut cmd = Command::new(binary);
    cmd.current_dir(&*ROOT);

    cmd.env("BENCH_MMDB", dataset);
    cmd.env("BENCH_WORKLOAD_DIR", workload_dir.unwrap_or(&*WORKLOAD_DIR));
    cmd.env("BENCH_SCENARIO", scenario);
    if scenario == "memory_rss" {
        cmd.env("BENCH_MEM_LOOKUPS", crate::memory::LOOKUPS.to_string());
    }
    cmd.env("BENCH_WORKLOAD_SIZE", size.to_string());
    cmd.env("BENCH_WARMUP_OPS", warmup.to_string());
    // All comparison harnesses consume the same wall-clock phase boundaries.
    // The counters remain available for fixed-protocol scenarios such as RSS.
    cmd.env("BENCH_WARMUP_SECS", "1");
    cmd.env("BENCH_MEASURE_SECS", "1");
    cmd.env("BENCH_LATENCY_SAMPLES_MAX", latency_samples.to_string());
    cmd.env("BENCH_OPEN_ITERS", open_iters.to_string());
    cmd.env("BENCH_IMPLEMENTATION_REVISION", revision);
    cmd.env("BENCH_HOT_IPV4", "81.2.69.160");
    cmd.env("BENCH_HOT_IPV6", "2001:db8:123::1");

    if let Some(f) = family {
        cmd.env("BENCH_FAMILY", f);
    }
    if let Some(p) = pattern {
        cmd.env("BENCH_PATTERN", p);
    }
    if let Some(t) = threads {
        cmd.env("BENCH_THREADS", t.to_string());
    }

    let context = serde_json::json!({
        "implementation": implementation, "revision": revision,
        "scenario": scenario, "family": family, "pattern": pattern,
        "workload_size": size, "threads": threads.unwrap_or(1), "repeat": repeat,
        "label_scenario": label_scenario,
    });
    let failure = |error: String, unsupported: bool| {
        let mut row = context.clone();
        row["failed"] = Value::Bool(true);
        row["unsupported"] = Value::Bool(unsupported);
        row["error"] = Value::String(error);
        row
    };
    let pinned_cpu = match pin_lookup_process(&mut cmd, threads.unwrap_or(1)) {
        Ok(cpu) => cpu,
        Err(error) => return failure(format!("CPU affinity: {error}"), false),
    };
    let output = match cmd.output() {
        Ok(out) => out,
        Err(e) => return failure(e.to_string(), false),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return failure(
            format!("status {}: {}", output.status, stderr.trim()),
            output.status.code() == Some(64) || stderr.contains("unsupported"),
        );
    }
    for line in stdout.lines().rev() {
        if let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(line.trim()) {
            map.insert("pinned_cpu".into(), serde_json::json!(pinned_cpu));
            // Non-lookup scenarios (e.g. memory_rss) tag their own protocol; do
            // not overwrite it with the lookup-affinity marker.
            map.entry("measurement_protocol")
                .or_insert_with(|| Value::from("lookup-affinity-timed-v2"));
            if scenario != "memory_rss" {
                map.entry("warmup_minimum_ns")
                    .or_insert_with(|| Value::from(1_000_000_000_u64));
                if let Some(window_ns) = map.get("wall_time_ns").cloned() {
                    map.entry("measurement_window_ns").or_insert(window_ns);
                }
            }
            for (key, value) in context.as_object().unwrap() {
                if !value.is_null() {
                    map.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
            return Value::Object(map);
        }
    }
    failure(format!("Missing JSON output: {}", stderr.trim()), false)
}

pub fn run_benchmarks_compare_suite(skip_competitors: bool) -> Result<(), String> {
    let use_color = supports_color();
    println!(
        "{}",
        style_bold("🚀 Starting comparative benchmark suite...", use_color)
    );

    // Clear previous results
    if RESULTS_PATH.exists() {
        let _ = fs::remove_file(&*RESULTS_PATH);
    }

    // 1. Build Binaries
    println!(
        "{}",
        style_cyan("🔧 Building benchmark binaries...", use_color)
    );
    let binaries = build_all_binaries()?;
    println!(
        "   {} Compiled benchmark harnesses",
        style_winner("✅", use_color)
    );

    // 2. Prepare Dependencies & Workloads
    println!(
        "{}",
        style_cyan("📦 Preparing dependencies and datasets...", use_color)
    );
    println!("   🗄️ Creating MMDB databases...");
    prepare_dataset(&DATASET_PATH)?;
    println!("   📥 Generating workload addresses...");
    prepare_workloads(
        &WORKLOAD_DIR,
        DEFAULT_LOOKUP_SIZES,
        DEFAULT_LOOKUP_PATTERNS,
        DEFAULT_FAMILIES,
    )?;
    println!("   {} Datasets ready", style_winner("✅", use_color));

    let absent_dir = prepare_ipv6_absent_fixture()?;

    // Phase 1: Database Open
    println!(
        "\n{}",
        style_bold("📁 [Phase 1/5] Database Open Benchmarks", use_color)
    );
    for &scenario in &["open_file", "open_buffer", "open_mmap"] {
        for &impl_name in IMPLEMENTATIONS {
            if skip_competitors && impl_name != "libmaxminddb-rs" {
                continue;
            }
            if let Some(bin) = binaries.get(impl_name) {
                let res = run_case(
                    bin,
                    impl_name,
                    "release",
                    &DATASET_PATH,
                    scenario,
                    None,
                    None,
                    1,
                    None,
                    20,
                    200,
                    200,
                    0,
                    None,
                    None,
                );
                let is_unsupported = res
                    .get("unsupported")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if is_unsupported {
                    println!(
                        "   {:<18} | {:<12} | {}",
                        impl_name,
                        scenario,
                        style_gray("⚠️ unsupported", use_color)
                    );
                } else {
                    let med = res.get("median_ns").and_then(|v| v.as_f64());
                    println!(
                        "   {:<18} | {:<12} | {}",
                        impl_name,
                        scenario,
                        style_winner(&fmt_duration(med), use_color)
                    );
                }
                write_result(&res);
            }
        }
    }

    // Phase 2: Single-threaded Lookups
    println!(
        "\n{}",
        style_bold(
            "📊 [Phase 2/5] Running Reader Single-threaded Benchmarks",
            use_color
        )
    );
    for &family in DEFAULT_FAMILIES {
        for &pattern in DEFAULT_LOOKUP_PATTERNS {
            let verified_absent = family == "ipv6" && pattern == "absent";
            let dataset = if verified_absent {
                absent_dir.join("ipv6.mmdb")
            } else {
                DATASET_PATH.to_path_buf()
            };
            println!("   🔍 Scenario: {family} ({pattern}, 1M lookups)");
            for &impl_name in IMPLEMENTATIONS {
                if skip_competitors && impl_name != "libmaxminddb-rs" {
                    continue;
                }
                if let Some(bin) = binaries.get(impl_name) {
                    let mut res = run_case(
                        bin,
                        impl_name,
                        "release",
                        &dataset,
                        "lookup",
                        Some(family),
                        Some(pattern),
                        1_000_000,
                        Some(1),
                        DEFAULT_WARMUP,
                        DEFAULT_LATENCY_SAMPLES,
                        200,
                        0,
                        None,
                        if verified_absent {
                            Some(absent_dir.as_path())
                        } else {
                            None
                        },
                    );
                    if verified_absent {
                        validate_absent_result(&res)?;
                        annotate_absent(&mut res, &absent_dir)?;
                    }
                    let is_unsupported = res
                        .get("unsupported")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if is_unsupported {
                        println!(
                            "     {:<18} | {}",
                            impl_name,
                            style_gray("⚠️ unsupported", use_color)
                        );
                    } else {
                        let p99 = res.get("p99_ns").and_then(|v| v.as_f64());
                        let rate = res.get("throughput_ops_s").and_then(|v| v.as_f64());
                        println!(
                            "     {:<18} | p99: {:>10} | throughput: {:>12}",
                            impl_name,
                            style_winner(&fmt_duration(p99), use_color),
                            style_cyan(&fmt_rate(rate), use_color),
                        );
                    }
                    write_result(&res);
                }
            }
        }
    }

    // Phase 3: Concurrent Multi-threaded Lookups
    println!(
        "\n{}",
        style_bold(
            "⚡ [Phase 3/5] Running Concurrent Multi-threaded Benchmarks",
            use_color
        )
    );
    for &family in &["ipv4", "ipv6"] {
        for &threads in DEFAULT_THREAD_COUNTS {
            println!("   ⚡ {threads} Threads ({family} random, 1M lookups)");
            for &impl_name in IMPLEMENTATIONS {
                if skip_competitors && impl_name != "libmaxminddb-rs" {
                    continue;
                }
                if let Some(bin) = binaries.get(impl_name) {
                    let res = run_case(
                        bin,
                        impl_name,
                        "release",
                        &DATASET_PATH,
                        "lookup_concurrent",
                        Some(family),
                        Some("random"),
                        1_000_000,
                        Some(threads),
                        DEFAULT_WARMUP,
                        DEFAULT_LATENCY_SAMPLES,
                        200,
                        0,
                        None,
                        None,
                    );
                    let rate = res.get("throughput_ops_s").and_then(|v| v.as_f64());
                    println!(
                        "     {:<18} | throughput: {:>14}",
                        impl_name,
                        style_cyan(&fmt_rate(rate), use_color)
                    );
                    write_result(&res);
                    if impl_name == "libmaxminddb-rs" {
                        for &(scenario, operation) in LOOKUP_API_SCENARIOS {
                            let variant = run_case(
                                bin,
                                impl_name,
                                "release",
                                &DATASET_PATH,
                                scenario,
                                Some(family),
                                Some("random"),
                                1_000_000,
                                Some(threads),
                                DEFAULT_WARMUP,
                                DEFAULT_LATENCY_SAMPLES,
                                200,
                                0,
                                None,
                                None,
                            );
                            if let (Some(expected), Some(actual)) = (
                                res.get("checksum").and_then(Value::as_u64),
                                variant.get("checksum").and_then(Value::as_u64),
                            ) && expected != actual
                            {
                                return Err(format!(
                                    "{operation} checksum mismatch for {family} {threads}T: {expected} != {actual}"
                                ));
                            }
                            let api_rate = variant.get("throughput_ops_s").and_then(Value::as_f64);
                            println!(
                                "     {:<18} | {operation}: {:>14}",
                                impl_name,
                                style_cyan(&fmt_rate(api_rate), use_color)
                            );
                            write_result(&variant);
                        }
                    }
                }
            }
        }
    }

    for row in run_size_benchmarks(
        &binaries,
        skip_competitors,
        SCALING_DATABASE_SIZES,
        WRITER_SIZES,
    )? {
        write_result(&row);
    }

    // Phase 4b: resident memory (RSS) vs database size for the four target readers.
    for row in run_memory_benchmarks(&binaries, skip_competitors, SCALING_DATABASE_SIZES)? {
        write_result(&row);
    }

    println!(
        "\n{}",
        style_winner(
            "✅ Comparative benchmark suite completed successfully.",
            use_color
        )
    );
    Ok(())
}

fn validate_scaling_measurement(row: &Value) -> Result<(), String> {
    if row["failed"] == true
        || row["unsupported"] == true
        || !row["throughput_ops_s"]
            .as_f64()
            .is_some_and(|v| v.is_finite() && v > 0.0)
        || !row["p99_ns"]
            .as_f64()
            .is_some_and(|v| v.is_finite() && v >= 0.0)
    {
        return Err(format!("invalid size-scaling measurement: {row}"));
    }
    Ok(())
}

fn run_size_benchmarks(
    binaries: &std::collections::HashMap<String, std::path::PathBuf>,
    skip_competitors: bool,
    reader_sizes: &[usize],
    writer_sizes: &[usize],
) -> Result<Vec<Value>, String> {
    let use_color = supports_color();
    let mut measured = Vec::new();
    // Phase 4: Database Size Scaling
    println!(
        "\n{}",
        style_bold(
            "📈 [Phase 4/5] Database Size Scaling Benchmarks (1K to 5M)",
            use_color
        )
    );
    let gen_tool = build_scaling_tool()?;
    let scaling_workload = SCALING_DIR.join("ipv4-random.txt");
    generate_scaling_workload(
        &gen_tool,
        &scaling_workload,
        "ipv4",
        SCALING_LOOKUP_IPS_COUNT,
        DEFAULT_RANDOM_SEED,
    )?;

    for &db_size in reader_sizes {
        let tag = fmt_size_tag(db_size);
        let db_path = SCALING_DIR.join(format!("bench_scale_ipv4_{db_size}.mmdb"));
        println!(
            "   🗄️ Generating {tag} ({db_size} entries) MMDB with batch size {DEFAULT_WRITER_BATCH_SIZE}..."
        );
        let t0 = Instant::now();
        generate_scaling_db(
            &gen_tool,
            &db_path,
            "ipv4",
            db_size,
            DEFAULT_RANDOM_SEED,
            DEFAULT_WRITER_BATCH_SIZE,
        )?;
        let dt = t0.elapsed().as_secs_f64();
        let sz = fs::metadata(&db_path)
            .map(|m| m.len() as f64)
            .unwrap_or(0.0);
        println!(
            "   {} Database ready: {} in {:.2}s",
            style_winner("✅", use_color),
            fmt_bytes(Some(sz)),
            dt
        );

        for &impl_name in IMPLEMENTATIONS {
            if skip_competitors && impl_name != "libmaxminddb-rs" {
                continue;
            }
            {
                let bin = binaries
                    .get(impl_name)
                    .ok_or_else(|| format!("missing {impl_name} harness"))?;
                let mut res = run_case(
                    bin,
                    impl_name,
                    "scaling",
                    &db_path,
                    "lookup",
                    Some("ipv4"),
                    Some("random"),
                    SCALING_LOOKUP_IPS_COUNT,
                    Some(1),
                    100,
                    SCALING_LOOKUP_IPS_COUNT,
                    10,
                    0,
                    Some("db_size_scaling"),
                    Some(&*SCALING_DIR),
                );
                if let Some(map) = res.as_object_mut() {
                    map.insert("database_size".into(), Value::from(db_size as u64));
                    map.insert("scenario".into(), Value::String("db_size_scaling".into()));
                }
                let p99 = res.get("p99_ns").and_then(|v| v.as_f64());
                println!(
                    "     {:<18} | p99: {:>10}",
                    impl_name,
                    style_winner(&fmt_duration(p99), use_color)
                );
                validate_scaling_measurement(&res)?;
                measured.push(res);
            }
        }
    }

    // Phase 5: MMDB Writer Benchmark
    println!(
        "\n{}",
        style_bold("✍️ [Phase 5/5] MMDB Writer Benchmarks", use_color)
    );
    for &size in writer_sizes {
        let tag = fmt_size_tag(size);
        println!("   📥 Generating {tag} ({size} entries) MMDB Writer comparison...");

        for (name, binary_name, size_flag, batch_flag) in [
            (
                "libmaxminddb-rs",
                "libmaxminddb-rs-writer",
                "--size",
                "--batch-size",
            ),
            ("mmdbwriter", "mmdbwriter", "-size", "-batch"),
        ] {
            if skip_competitors && name != "libmaxminddb-rs" {
                continue;
            }
            let bin = binaries
                .get(binary_name)
                .ok_or_else(|| format!("missing {binary_name} harness"))?;
            let run_once = || -> Result<Value, String> {
                let output = Command::new(bin)
                    .args([
                        size_flag,
                        &size.to_string(),
                        batch_flag,
                        &DEFAULT_WRITER_BATCH_SIZE.to_string(),
                    ])
                    .output()
                    .map_err(|e| format!("{name} at {size} entries: {e}"))?;
                if !output.status.success() {
                    return Err(format!(
                        "{name} at {size} entries failed ({}): {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .rev()
                    .find_map(|line| {
                        serde_json::from_str::<Value>(line)
                            .ok()
                            .filter(Value::is_object)
                    })
                    .ok_or_else(|| format!("{name} at {size} entries returned no JSON measurement"))
            };

            // Writer runs are complete database builds. Repeat whole builds so
            // both the discarded warm-up phase and measured phase last >= 1 s.
            let warmup_start = Instant::now();
            let mut warmup_operations = 0_u64;
            while warmup_start.elapsed() < std::time::Duration::from_secs(1) {
                let _ = run_once()?;
                warmup_operations = warmup_operations.saturating_add(size as u64);
            }
            let warmup_duration_ns = warmup_start.elapsed().as_nanos() as u64;

            let measurement_start = Instant::now();
            let mut measurement_operations = 0_u64;
            let mut aggregate_wall_ns = 0_u64;
            let mut aggregate_insert_ns = 0_u64;
            let mut aggregate_build_ns = 0_u64;
            let mut peak_rss_bytes = 0_u64;
            let mut row = loop {
                let next = run_once()?;
                measurement_operations = measurement_operations
                    .saturating_add(next["entries"].as_u64().unwrap_or(size as u64));
                aggregate_wall_ns =
                    aggregate_wall_ns.saturating_add(next["wall_time_ns"].as_u64().unwrap_or(0));
                aggregate_insert_ns = aggregate_insert_ns
                    .saturating_add(next["insert_time_ns"].as_u64().unwrap_or(0));
                aggregate_build_ns =
                    aggregate_build_ns.saturating_add(next["build_time_ns"].as_u64().unwrap_or(0));
                peak_rss_bytes = peak_rss_bytes.max(next["peak_rss_bytes"].as_u64().unwrap_or(0));
                if measurement_start.elapsed() >= std::time::Duration::from_secs(1) {
                    break next;
                }
            };
            row["warmup_operations"] = Value::from(warmup_operations);
            row["warmup_duration_ns"] = Value::from(warmup_duration_ns);
            row["warmup_minimum_ns"] = Value::from(1_000_000_000_u64);
            row["measurement_operations"] = Value::from(measurement_operations);
            row["measurement_window_ns"] =
                Value::from(measurement_start.elapsed().as_nanos() as u64);
            row["measurement_protocol"] = Value::from("timed-comparison-v2");
            row["wall_time_ns"] = Value::from(aggregate_wall_ns);
            row["insert_time_ns"] = Value::from(aggregate_insert_ns);
            row["build_time_ns"] = Value::from(aggregate_build_ns);
            row["peak_rss_bytes"] = Value::from(peak_rss_bytes);
            row["throughput_ops_s"] = Value::from(if aggregate_wall_ns > 0 {
                measurement_operations as f64 * 1_000_000_000.0 / aggregate_wall_ns as f64
            } else {
                0.0
            });
            row["implementation"] = Value::from(name);
            row["scenario"] = Value::from("writer_generation");
            row["database_size"] = Value::from(size);
            row["entries"] = Value::from(size);
            validate_scaling_measurement(&row)?;
            println!(
                "     {name:<18} | rate: {:>12} | total: {:>8} | peak RSS: {:>9}",
                fmt_rate(row["throughput_ops_s"].as_f64()),
                fmt_duration(row["wall_time_ns"].as_f64()),
                fmt_bytes(row["peak_rss_bytes"].as_f64())
            );
            measured.push(row);
        }
    }
    Ok(measured)
}

pub fn run_all_benchmarks_suite(skip_competitors: bool) -> Result<(), String> {
    run_benchmarks_compare_suite(skip_competitors)
}

/// Remeasures both writers at every configured size and preserves reader results.
pub fn run_writer_only_suite() -> Result<(), String> {
    let binaries = build_writer_binaries()?;
    let measured = run_size_benchmarks(&binaries, false, &[], WRITER_SIZES)?;
    let previous = if RESULTS_PATH.exists() {
        fs::read_to_string(&*RESULTS_PATH).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    let mut text = String::new();
    for line in previous.lines() {
        let is_writer = serde_json::from_str::<Value>(line)
            .is_ok_and(|row| row["scenario"] == "writer_generation");
        if !is_writer {
            text.push_str(line);
            text.push('\n');
        }
    }
    for row in measured {
        text.push_str(&serde_json::to_string(&row).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    fs::create_dir_all(RESULTS_PATH.parent().unwrap()).map_err(|e| e.to_string())?;
    if RESULTS_PATH.exists() {
        fs::copy(
            &*RESULTS_PATH,
            RESULTS_PATH.with_extension("jsonl.before-writer"),
        )
        .map_err(|e| e.to_string())?;
    }
    let pending = RESULTS_PATH.with_extension("jsonl.pending");
    fs::write(&pending, text).map_err(|e| e.to_string())?;
    fs::rename(pending, &*RESULTS_PATH).map_err(|e| e.to_string())
}

/// RSS comparisons share the isolated protocol with the --memory-only runner.
pub fn run_memory_benchmarks(
    binaries: &std::collections::HashMap<String, std::path::PathBuf>,
    skip_competitors: bool,
    sizes: &[usize],
) -> Result<Vec<Value>, String> {
    crate::memory::run(binaries, skip_competitors, sizes)
}

/// Remeasure the large-size scenarios (>= 1.5M: 1.5M, 2M, 5M), preserving all other results.
pub fn run_large_sizes_suite() -> Result<(), String> {
    let binaries = build_all_binaries()?;
    let sizes: Vec<_> = SCALING_DATABASE_SIZES
        .iter()
        .copied()
        .filter(|&size| size >= 1_500_000)
        .collect();
    let mut measured = run_size_benchmarks(&binaries, false, &sizes, &sizes)?;
    measured.extend(run_memory_benchmarks(&binaries, false, &sizes)?);
    let previous = if RESULTS_PATH.exists() {
        fs::read_to_string(&*RESULTS_PATH).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    let mut text = String::new();
    for line in previous.lines() {
        let replaces = serde_json::from_str::<Value>(line).is_ok_and(|row| {
            matches!(
                row["scenario"].as_str(),
                Some("db_size_scaling" | "writer_generation" | "memory_rss")
            ) && row["database_size"]
                .as_u64()
                .is_some_and(|size| sizes.contains(&(size as usize)))
        });
        if !replaces {
            text.push_str(line);
            text.push('\n');
        }
    }
    for row in measured {
        text.push_str(&serde_json::to_string(&row).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    fs::create_dir_all(RESULTS_PATH.parent().unwrap()).map_err(|e| e.to_string())?;
    if RESULTS_PATH.exists() {
        fs::copy(
            &*RESULTS_PATH,
            RESULTS_PATH.with_extension("jsonl.before-large-sizes"),
        )
        .map_err(|e| e.to_string())?;
    }
    let pending = RESULTS_PATH.with_extension("jsonl.pending");
    fs::write(&pending, text).map_err(|e| e.to_string())?;
    fs::rename(pending, &*RESULTS_PATH).map_err(|e| e.to_string())
}

fn prepare_ipv6_absent_fixture() -> Result<std::path::PathBuf, String> {
    let dir = crate::config::DATA_DIR.join("ipv6-absent-v1");
    // Cargo checks freshness; always regenerate and verify outside benchmark timers.
    crate::workloads::run_command(
        &[
            "cargo",
            "run",
            "--quiet",
            "--release",
            "--manifest-path",
            &ROOT
                .join("tools/gen-scaling-bench-data/Cargo.toml")
                .to_string_lossy(),
            "--",
            "ipv6-absent",
            &dir.to_string_lossy(),
            "1000000",
        ],
        Some(&ROOT),
    )?;
    Ok(dir)
}

fn sha256(path: &Path) -> Result<String, String> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("cannot hash {}", path.display()));
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| "missing SHA-256".into())
}

fn annotate_absent(row: &mut Value, dir: &Path) -> Result<(), String> {
    row["measurement_protocol"] = Value::from("ipv6-absent-v1");
    row["database_entries"] = Value::from(1_000_000);
    row["random_seed"] = Value::from(DEFAULT_RANDOM_SEED);
    row["dataset_sha256"] = Value::from(sha256(&dir.join("ipv6.mmdb"))?);
    row["workload_sha256"] = Value::from(sha256(&dir.join("ipv6-absent.txt"))?);
    row["preflight_absent_queries"] = Value::from(1_000_000);
    Ok(())
}

fn validate_absent_result(row: &Value) -> Result<(), String> {
    if row.get("failed").and_then(Value::as_bool) == Some(true)
        || row.get("unsupported").and_then(Value::as_bool) == Some(true)
        || !row
            .get("throughput_ops_s")
            .and_then(Value::as_f64)
            .is_some_and(|x| x.is_finite() && x > 0.0)
        || row.get("workload_size").and_then(Value::as_u64) != Some(1_000_000)
    {
        return Err(format!("IPv6 absent benchmark failed: {row}"));
    }
    Ok(())
}

/// Rerun this scenario for every reader, preserving unrelated recorded measurements.
pub fn run_ipv6_absent_suite() -> Result<(), String> {
    let dir = prepare_ipv6_absent_fixture()?;
    let mut measured = Vec::new();
    for &name in IMPLEMENTATIONS {
        println!("Building and measuring {name}: IPv6 absent, 1M entries / 1M queries");
        let bin = match name {
            "libmaxminddb" => crate::builder::build_c_binary(name)?,
            "maxminddb-golang" => crate::builder::build_go_binary(name, "./cmd/reader")?,
            _ => crate::builder::build_rust_binary(name, "-C target-cpu=native -C opt-level=3")?,
        };
        let mut row = run_case(
            &bin,
            name,
            &crate::builder::get_git_revision(),
            &dir.join("ipv6.mmdb"),
            "lookup",
            Some("ipv6"),
            Some("absent"),
            1_000_000,
            Some(1),
            DEFAULT_WARMUP,
            DEFAULT_LATENCY_SAMPLES,
            200,
            0,
            None,
            Some(&dir),
        );
        validate_absent_result(&row)?;
        annotate_absent(&mut row, &dir)?;
        println!("{name}: {} ops/s", row["throughput_ops_s"]);
        measured.push(row);
    }
    // Preserve unrelated JSON lines verbatim, including floating-point precision.
    let previous = if RESULTS_PATH.exists() {
        fs::read_to_string(&*RESULTS_PATH).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    let mut text = String::new();
    for line in previous.lines() {
        let replaces = serde_json::from_str::<Value>(line).is_ok_and(|r| {
            r["scenario"] == "lookup" && r["family"] == "ipv6" && r["pattern"] == "absent"
        });
        if !replaces {
            text.push_str(line);
            text.push('\n');
        }
    }
    for row in measured {
        text.push_str(&serde_json::to_string(&row).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    if RESULTS_PATH.exists() {
        fs::copy(
            &*RESULTS_PATH,
            RESULTS_PATH.with_extension("jsonl.before-ipv6-absent"),
        )
        .map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(RESULTS_PATH.parent().unwrap()).map_err(|e| e.to_string())?;
    let pending = RESULTS_PATH.with_extension("jsonl.pending");
    fs::write(&pending, text).map_err(|e| e.to_string())?;
    fs::rename(pending, &*RESULTS_PATH).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn affinity_is_applied_only_to_the_child() {
        let before = fs::read_to_string("/proc/self/status").unwrap();
        let mut cmd = Command::new("/bin/cat");
        cmd.arg("/proc/self/status");
        if let Some(cpu) = pin_lookup_process(&mut cmd, 1).unwrap() {
            let output = cmd.output().unwrap();
            assert!(output.status.success());
            let status = String::from_utf8(output.stdout).unwrap();
            let cpus = status
                .lines()
                .find(|s| s.starts_with("Cpus_allowed_list:"))
                .unwrap();
            assert_eq!(cpus.split_whitespace().last().unwrap(), cpu.to_string());
        }
        let after = fs::read_to_string("/proc/self/status").unwrap();
        assert_eq!(
            before.lines().find(|s| s.starts_with("Cpus_allowed_list:")),
            after.lines().find(|s| s.starts_with("Cpus_allowed_list:")),
        );
        assert!(
            pin_lookup_process(&mut Command::new("/bin/true"), 4)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn failed_process_keeps_scenario_dimensions_and_is_not_a_measurement() {
        let row = run_case(
            Path::new("/bin/false"),
            "maxminddb-rust",
            "test",
            Path::new("unused.mmdb"),
            "lookup",
            Some("ipv6"),
            Some("absent"),
            1_000_000,
            Some(1),
            0,
            1,
            1,
            0,
            None,
            None,
        );
        assert_eq!(row["failed"], true);
        assert!(row["error"].as_str().unwrap().contains("status"));
        assert_eq!(row["family"], "ipv6");
        assert_eq!(row["pattern"], "absent");
        assert!(row.get("throughput_ops_s").is_none());
        assert!(validate_absent_result(&row).is_err());
        let rows = crate::stats::aggregate_results(&[row]);
        assert_eq!(
            crate::stats::get_largest_lookup(&rows, "ipv6", "absent").len(),
            1
        );
        let items = [("maxminddb-rust", 0.0, "⚠️ failed")];
        let (_, svg) = crate::charts::svg_bar_chart("failure", &items, 600, 42, "max", "");
        assert!(svg.contains("⚠️ failed"));
    }
}
