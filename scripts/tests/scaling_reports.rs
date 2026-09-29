use std::collections::HashMap;
use std::fs;

use bench_runner::charts::generate_standalone_svg_charts;
use bench_runner::config::{IMPLEMENTATIONS, SCALING_DATABASE_SIZES, WRITER_SIZES};
use bench_runner::html_report::generate_html_report;
use bench_runner::readme::{generate_readme_benchmark_markdown, update_readme_benchmarks};
use bench_runner::stats::{
    aggregate_results, export_results_csv, export_results_json, extract_summary_items,
};
use serde_json::json;

#[test]
fn five_million_measurements_reach_exports_tables_and_charts() {
    // A sparse result set also exercises correctly labelled chart points when
    // earlier sizes have no measurements. Test data never enters real reports.
    // 5M is checked immediately after the existing 2M scenario, mirroring
    // SCALING_DATABASE_SIZES ordering.
    let mut raw = Vec::new();
    for size in [1_500_000, 2_000_000, 5_000_000] {
        for name in IMPLEMENTATIONS {
            raw.push(json!({
                "implementation": name,
                "scenario": "db_size_scaling",
                "database_size": size,
                "family": "ipv4",
                "pattern": "random",
                "workload_size": 1_000,
                "p99_ns": size / 1_000,
                "throughput_ops_s": 123_456.0,
            }));
        }
        for name in ["libmaxminddb-rs", "mmdbwriter"] {
            raw.push(json!({
                "implementation": name,
                "scenario": "writer_generation",
                "database_size": size,
                "entries": size,
                "family": "ipv4",
                "p99_ns": size / 1_000,
                "throughput_ops_s": 123_456.0,
                "wall_time_ns": 2_000_000_000.0,
                "peak_rss_bytes": 1_048_576,
                "database_size_bytes": 1_048_576,
            }));
        }
    }
    assert!(
        SCALING_DATABASE_SIZES
            .windows(2)
            .any(|pair| pair == [1_500_000, 2_000_000])
    );
    assert!(
        SCALING_DATABASE_SIZES
            .windows(2)
            .any(|pair| pair == [2_000_000, 5_000_000])
    );
    assert!(
        WRITER_SIZES
            .windows(2)
            .any(|pair| pair == [1_500_000, 2_000_000])
    );
    assert!(
        WRITER_SIZES
            .windows(2)
            .any(|pair| pair == [2_000_000, 5_000_000])
    );

    let rows = aggregate_results(&raw);
    let dir = std::env::temp_dir().join(format!("mmdb-scaling-report-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let charts = generate_standalone_svg_charts(&rows, &dir);
    assert!(
        charts
            .iter()
            .any(|path| path.ends_with("database-size-p99-5000000.svg"))
    );
    for filename in [
        "database-size-scaling.svg",
        "writer-throughput.svg",
        "writer-build-time.svg",
        "writer-database-size.svg",
        "writer-p99.svg",
        "writer-peak-rss.svg",
    ] {
        let svg = fs::read_to_string(dir.join(filename)).unwrap();
        assert!(
            svg.find(">1.5M</text>").unwrap() < svg.find(">2M</text>").unwrap(),
            "{filename}"
        );
        assert!(
            svg.find(">2M</text>").unwrap() < svg.find(">5M</text>").unwrap(),
            "{filename}"
        );
        assert!(svg.contains(" @ 5M</title>"), "{filename}");
    }
    let bar = fs::read_to_string(dir.join("database-size-p99-5000000.svg")).unwrap();
    for name in [
        "libmaxminddb-rs",
        "libmaxminddb (C)",
        "maxminddb-rust",
        "geoip2-rs",
        "maxminddb-golang",
    ] {
        assert!(bar.contains(name));
    }
    let svg = fs::read_to_string(dir.join("database-size-scaling.svg")).unwrap();
    assert!(svg.contains("maxminddb-rust: 5000.00 ns @ 5M</title>"));

    let html_path = dir.join("index.html");
    generate_html_report(&rows, &raw, &HashMap::new(), &html_path, true).unwrap();
    let html = fs::read_to_string(html_path).unwrap();
    let scaling_table = html
        .split("Scaling Table — p99 Tail Latency by Database Size")
        .nth(1)
        .unwrap();
    assert!(
        scaling_table.find("<strong>1.5M</strong>").unwrap()
            < scaling_table.find("<strong>2M</strong>").unwrap()
    );
    assert!(
        scaling_table.find("<strong>2M</strong>").unwrap()
            < scaling_table.find("<strong>5M</strong>").unwrap()
    );
    assert!(html.contains("(5000000)"));
    // HTML shows the 5M scenario in the scaling and writer tables, but the
    // executive summary deliberately omits Writer rows, so it must be checked
    // in the per-size table instead of a "Writer Generation" summary label.
    assert!(
        html.contains("<strong>5M</strong> <small style='color:var(--muted)'>(5000000)</small>")
    );

    // Console and README summaries use the same extracted rows.
    let summary = extract_summary_items(&rows);
    let writer = summary
        .iter()
        .find(|item| item.scenario == "Writer Generation (5M)")
        .unwrap();
    assert_eq!(writer.rows.len(), 2);
    let cpu = "Benchmark CPU 1234";
    let rustc_version = bench_runner::builder::detect_compilers()
        .get("rustc")
        .and_then(|version| version.split_whitespace().nth(1))
        .unwrap_or("unknown")
        .to_owned();
    let readme = generate_readme_benchmark_markdown(&rows, cpu);
    assert!(readme.contains("Writer Generation (5M)"));
    assert!(readme.contains("2M, 5M entries"));
    assert!(readme.contains("make bench-compare-docker"));
    assert!(readme.contains(&format!(
        "*Environment: Linux x86_64 · {cpu} · rustc {rustc_version} ·"
    )));
    assert!(readme.contains(&format!("*Measured on {cpu} under Linux:*")));

    let readme_path = dir.join("README.md");
    fs::write(
        &readme_path,
        "# Project\n\n## 📊 Benchmarks\n\nOld results\n\n## 🔥 FlameGraph\n",
    )
    .unwrap();
    update_readme_benchmarks(&rows, &readme_path, cpu);
    let updated = fs::read_to_string(&readme_path).unwrap();
    assert!(updated.contains(&format!(
        "*Environment: Linux x86_64 · {cpu} · rustc {rustc_version} ·"
    )));
    assert!(updated.contains(&format!("*Measured on {cpu} under Linux:*")));
    assert!(!updated.contains("Old results"));

    export_results_json(&rows, &dir.join("results.json")).unwrap();
    export_results_csv(&rows, &dir.join("results.csv")).unwrap();
    let exported: Vec<serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(dir.join("results.json")).unwrap()).unwrap();
    assert_eq!(
        exported
            .iter()
            .filter(|row| row["database_size"] == 5_000_000)
            .count(),
        7
    );
    assert_eq!(
        fs::read_to_string(dir.join("results.csv"))
            .unwrap()
            .lines()
            .filter(|line| line.contains(",5000000,"))
            .count(),
        7
    );
    fs::remove_dir_all(dir).unwrap();
}
