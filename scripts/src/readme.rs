use serde_json::Value;
use std::fs;
use std::path::Path;

use crate::builder::{detect_compilers, detect_versions};
use crate::config::SCALING_DATABASE_SIZES;

use crate::metrics::{fmt_bytes, fmt_duration, fmt_rate, fmt_size_tag};
use crate::stats::{extract_summary_items, find_winners_and_slowest};

pub fn generate_readme_benchmark_markdown(rows: &[Value], cpu: &str) -> String {
    let versions = detect_versions();
    let compilers = detect_compilers();
    let summary_items = extract_summary_items(rows);

    let rustc_ver = compilers
        .get("rustc")
        .map(|s| s.split_whitespace().nth(1).unwrap_or(s.as_str()))
        .unwrap_or("unknown");
    let c_ver = compilers
        .get("c_compiler")
        .map(|s| s.split_whitespace().next().unwrap_or(s.as_str()))
        .unwrap_or("gcc");
    let go_ver = compilers
        .get("go")
        .map(|s| s.replace("go version ", ""))
        .unwrap_or_else(|| "go".into());

    let mut lines = vec![
        "## 📊 Benchmarks".to_string(),
        "".to_string(),
        "Reproducible cross-library benchmark suite comparing `libmaxminddb-rs` against industry standard implementations in Rust, C, and Go.".to_string(),
        "".to_string(),
        "### ⚙️ Evaluated Libraries & Reproducibility Specification".to_string(),
        "".to_string(),
        "| Library Name | Language | Role | Evaluated Version | Compiler & Build Flags | Upstream Repository |".to_string(),
        "|:---|:---:|:---:|:---:|:---|:---|".to_string(),
        format!("| 🦀 **`libmaxminddb-rs`** | Rust | Reader & Writer | {} | `rustc {}` (opt-level=3, native) | Current Repository |", versions.get("libmaxminddb-rs").unwrap_or(&"0.1.0".into()), rustc_ver),
        format!("| 🏛️ **`libmaxminddb`** | C | Reader | {} | `{}` (-O3 -march=native) | [maxmind/libmaxminddb](https://github.com/maxmind/libmaxminddb) |", versions.get("libmaxminddb").unwrap_or(&"1.14.1".into()), c_ver),
        format!("| 📦 **`maxminddb-rust`** | Rust | Reader | {} | `rustc {}` (release) | [maxminddb-rust](https://crates.io/crates/maxminddb) |", versions.get("maxminddb-rust").unwrap_or(&"0.32.0".into()), rustc_ver),
        format!("| 🚀 **`geoip2-rs`** | Rust | Reader | {} | `rustc {}` (release) | [geoip2-rs](https://crates.io/crates/geoip2) |", versions.get("geoip2-rs").unwrap_or(&"0.1.8".into()), rustc_ver),
        format!("| 🐹 **`maxminddb-golang`** | Go | Reader | {} | `go {}` (-ldflags=\"-s -w\" -trimpath) | [oschwald/maxminddb-golang](https://github.com/oschwald/maxminddb-golang) |", versions.get("maxminddb-golang").unwrap_or(&"v2.6.0".into()), go_ver),
        format!("| ✍️ **`mmdbwriter`** | Go | Writer | {} | `go {}` (-ldflags=\"-s -w\" -trimpath) | [maxmind/mmdbwriter](https://github.com/maxmind/mmdbwriter) |", versions.get("mmdbwriter").unwrap_or(&"v1.2.0".into()), go_ver),
        "".to_string(),
        format!("*Environment: Linux x86_64 · {cpu} · rustc {rustc_ver} · Deterministic SplitMix64 datasets with pre-allocated memory.*"),
        "".to_string(),
        "Execute all benchmarks and regenerate reports with a single command:".to_string(),
        "```bash".to_string(),
        "make bench-compare".to_string(),
        "```".to_string(),
        "".to_string(),
        "To run the same comparative suite with pinned Rust and Go toolchains in Docker, use `make bench-compare-docker` (Docker with Compose required). It selects the system `default` Docker context, even if Docker Desktop is the current CLI context; set `BENCH_DOCKER_CONTEXT=name` to choose another daemon. The command builds the image, runs `make bench-compare` in a temporary container, and prints the host paths of the generated HTML report, SVG charts, JSON/CSV results, and updated `README.md` when it finishes. Docker results and build caches remain under `target/docker-bench/`. Compare measurements only from compatible host CPUs and Docker resource limits.".to_string(),
        "".to_string(),
        "Both benchmark commands generate an interactive HTML report at `benchmark-report/index.html` with all measured results, charts, and sortable tables. Open this local file after the run.".to_string(),
        "".to_string(),
        "<details>".to_string(),
        "<summary>📊 Full HTML report preview (static image)</summary>".to_string(),
        "".to_string(),
        "<img src=\"docs/images/benchmark-report-full.png\" alt=\"Full benchmark report preview with every section, chart, and table\" width=\"100%\">".to_string(),
        "".to_string(),
        "</details>".to_string(),
        "".to_string(),
        format!("Database-size and writer benchmarks use {} entries, with the same fixed seed, query workload and batch settings at every size.",
            SCALING_DATABASE_SIZES.iter().map(|&size| fmt_size_tag(size)).collect::<Vec<_>>().join(", ")),
        "".to_string(),
        "Reader RSS is also compared at these eight sizes for the four Rust/C libraries, using identical databases and queries, mmap, and three isolated processes per point. The generated HTML report's Memory section shows RSS after open and the lookup peak, with exact hover values and explicit unavailable measurements. See [the memory protocol](docs/MEMORY_BENCHMARKS.md) for reproduction and interpretation.".to_string(),
        "".to_string(),
        "### 🏁 Reader Performance Summary — p99 Tail Latency & Peak Throughput".to_string(),
        "".to_string(),
        format!("*Measured on {cpu} under Linux:*"),
        "".to_string(),
        "| Scenario | Metric | 🦀 `libmaxminddb-rs` | 🏛️ `libmaxminddb` (C) | 📦 `maxminddb-rust` | 🚀 `geoip2-rs` | 🐹 `maxminddb-golang` | ✍️ `mmdbwriter` (Go) |".to_string(),
        "|:---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|".to_string(),
    ];

    for item in &summary_items {
        if item.metric_key == "allocations_per_op" {
            continue;
        }
        let mut row_str = format!("| {} | {} |", item.scenario, item.metric_label);

        // Determine winner and slowest among valid implementations
        let mut vals: Vec<(&str, f64)> = Vec::new();
        for &name in &[
            "libmaxminddb-rs",
            "libmaxminddb",
            "maxminddb-rust",
            "geoip2-rs",
            "maxminddb-golang",
            "mmdbwriter",
        ] {
            if let Some(r) = item.rows.get(name) {
                if let Some(v) = r.get(&item.metric_key).and_then(|x| x.as_f64()) {
                    if !v.is_nan()
                        && v > 0.0
                        && r.get("unsupported").and_then(|u| u.as_bool()) != Some(true)
                    {
                        vals.push((name, v));
                    }
                }
            }
        }
        let (winners, slowest) =
            find_winners_and_slowest(&vals, item.direction == crate::metrics::LOWER_IS_BETTER);

        for &impl_name in &[
            "libmaxminddb-rs",
            "libmaxminddb",
            "maxminddb-rust",
            "geoip2-rs",
            "maxminddb-golang",
            "mmdbwriter",
        ] {
            let cell = match item.rows.get(impl_name) {
                Some(r) => {
                    if r.get("unsupported").and_then(|v| v.as_bool()) == Some(true) {
                        "⚠️ unsupported".to_string()
                    } else if let Some(n) = r.get(&item.metric_key).and_then(|v| v.as_f64()) {
                        let formatted =
                            if item.unit == "ns" || item.unit == "µs" || item.unit == "s" {
                                fmt_duration(Some(n))
                            } else if item.unit == "ops/s" {
                                fmt_rate(Some(n))
                            } else if item.unit == "allocs" {
                                format!("{n:.0}")
                            } else if item.unit == "MB" {
                                fmt_bytes(Some(n))
                            } else {
                                format!("{n:.1}")
                            };
                        if winners.contains(&impl_name) {
                            format!("🏆 **{formatted}**")
                        } else if slowest.contains(&impl_name) {
                            format!("<span style=\"color:#ff3b5c\">{formatted}</span>")
                        } else if impl_name == "libmaxminddb-rs" {
                            format!("**{formatted}**")
                        } else {
                            formatted
                        }
                    } else {
                        "—".to_string()
                    }
                }
                None => "—".to_string(),
            };
            row_str.push_str(&format!(" {cell} |"));
        }
        lines.push(row_str);
    }

    lines.push("".to_string());
    lines.push("### 📈 Visual Benchmark Charts".to_string());
    lines.push("".to_string());
    for (title, image) in [
        (
            "Candlestick Percentile Rank — IPv4 Lookups",
            "candlestick-percentiles-ipv4.svg",
        ),
        (
            "Candlestick Percentile Rank — IPv6 Lookups",
            "candlestick-percentiles-ipv6.svg",
        ),
        (
            "IPv4 Throughput — Random Lookups (1M)",
            "throughput-ipv4-random.svg",
        ),
        (
            "IPv6 Throughput — Random Lookups (1M)",
            "throughput-ipv6-random.svg",
        ),
        (
            "IPv4 Throughput — Absent Keys (1M)",
            "throughput-ipv4-absent.svg",
        ),
        (
            "IPv6 Throughput — Absent Keys (1M)",
            "throughput-ipv6-absent.svg",
        ),
        (
            "Peak Concurrent Throughput — 16 Threads",
            "concurrent-throughput.svg",
        ),
        (
            "Peak Concurrent Throughput — 16 Threads (IPv6)",
            "concurrent-throughput-ipv6.svg",
        ),
        (
            "IPv4 Random Lookup — Worker Scaling by API",
            "lookup-api-ipv4-multithread.svg",
        ),
        (
            "IPv6 Random Lookup — Worker Scaling by API",
            "lookup-api-ipv6-multithread.svg",
        ),
        (
            "p99 Tail Latency vs Database Size",
            "database-size-scaling.svg",
        ),
        ("Peak RSS During Lookups (mmap)", "memory-rss-peak.svg"),
    ] {
        lines.push(format!(
            "<p align=\"center\"><strong>{title}</strong><br><img src=\"benchmarks/charts/{image}\" alt=\"{title}\" width=\"100%\"></p>"
        ));
        lines.push(String::new());
    }
    lines.join("\n")
}

pub fn update_readme_benchmarks(aggregated_rows: &[Value], readme_path: &Path, cpu: &str) {
    if !readme_path.exists() {
        return;
    }
    let content = match fs::read_to_string(readme_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let start_marker = "## 📊 Benchmarks";
    let end_marker = "## 🔥 FlameGraph";

    if let Some(start_pos) = content.find(start_marker) {
        let new_section = generate_readme_benchmark_markdown(aggregated_rows, cpu);
        let updated = if let Some(end_pos) = content[start_pos..].find(end_marker) {
            let full_end = start_pos + end_pos;
            format!(
                "{}\n{}\n\n{}",
                &content[..start_pos].trim_end(),
                new_section.trim(),
                &content[full_end..]
            )
        } else {
            format!("{}\n{}", &content[..start_pos].trim_end(), new_section)
        };
        let _ = fs::write(readme_path, updated);
    }
}
