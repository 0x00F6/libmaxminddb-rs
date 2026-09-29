use bench_runner::{
    charts::{memory_charts, svg_line_chart},
    config::{MEMORY_IMPLEMENTATIONS, MEMORY_PROTOCOL, SCALING_DATABASE_SIZES},
    html_report::generate_html_report,
    memory::cell,
    stats::{aggregate_results, group_key},
};
use serde_json::{Value, json};

fn measurement(name: &str, size: usize) -> Value {
    json!({
        "implementation": name, "scenario": "memory_rss", "database_size": size,
        "measurement_protocol": MEMORY_PROTOCOL, "opening_mode": "mmap",
        "rss_method": "linux-vmrss-vmhwm-reset-after-open", "lookups": 1_000_000,
        "warmup_ops": 1000, "dataset_sha256": format!("database-{size}"),
        "workload_sha256": format!("queries-{size}"),
        "rss_before_open_bytes": 4_194_304, "rss_after_open_bytes": 8_388_608,
        "rss_peak_bytes": 12_582_912,
    })
}

#[test]
fn all_four_libraries_and_eight_sizes_reach_both_charts_and_html() {
    let rows: Vec<_> = SCALING_DATABASE_SIZES
        .iter()
        .flat_map(|&size| {
            MEMORY_IMPLEMENTATIONS
                .iter()
                .map(move |name| measurement(name, size))
        })
        .collect();
    let charts = memory_charts(&rows);
    assert_eq!(charts.len(), 2);
    for (_, svg) in &charts {
        assert_eq!(svg.matches("</title>").count(), 32);
        for name in [
            "libmaxminddb-rs",
            "libmaxminddb (C)",
            "maxminddb-rust",
            "geoip2-rs",
        ] {
            assert_eq!(svg.matches(&format!("<title>{name}:")).count(), 8);
        }
        assert!(svg.contains(" @ 1.5M</title>"));
        assert!(svg.contains(" @ 5M</title>"));
        assert!(svg.contains(" bytes) @ "));
    }
    assert!(charts[0].1.contains("8 MiB (8388608 bytes)"));
    let dir = std::env::temp_dir().join(format!("mmdb-memory-report-{}", std::process::id()));
    let path = dir.join("index.html");
    generate_html_report(&rows, &rows, &Default::default(), &path, false).unwrap();
    let html = std::fs::read_to_string(&path).unwrap();
    let section = html
        .split("<section id=\"memory\">")
        .nth(1)
        .unwrap()
        .split("</section>")
        .next()
        .unwrap();
    assert_eq!(section.matches("data-memory-size=").count(), 32);
    assert!(!section.contains("Unavailable —"));
    assert!(section.contains("/proc/self/clear_refs = 5"));
    assert!(section.contains("4,000,000-byte query buffer"));
    for (_, svg) in charts {
        assert!(section.contains(&svg));
    }

    // An absent library is visibly unavailable, never a zero-valued curve.
    let partial: Vec<_> = rows
        .into_iter()
        .filter(|r| r["implementation"] != "geoip2-rs")
        .collect();
    generate_html_report(&partial, &partial, &Default::default(), &path, false).unwrap();
    let html = std::fs::read_to_string(&path).unwrap();
    assert_eq!(html.matches("Unavailable — Not measured").count(), 24);
    let charts = memory_charts(&partial);
    assert!(charts[0].1.contains("geoip2-rs")); // legend remains present
    assert_eq!(charts[0].1.matches("</title>").count(), 24);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rss_aggregation_uses_median_open_max_peak_and_keeps_unavailable_repeats() {
    let mut rows: Vec<_> = (0..3)
        .map(|_| measurement("libmaxminddb-rs", 1000))
        .collect();
    for (row, (open, peak)) in rows
        .iter_mut()
        .zip([(8192, 16384), (4096, 24576), (12288, 20480)])
    {
        row["rss_after_open_bytes"] = json!(open);
        row["rss_peak_bytes"] = json!(peak);
    }
    let aggregated = aggregate_results(&rows);
    assert_eq!(aggregated.len(), 1);
    assert_eq!(aggregated[0]["rss_after_open_bytes"], 8192);
    assert_eq!(aggregated[0]["rss_peak_bytes"], 24576);
    assert_eq!(aggregated[0]["run_count"], 3);
    rows[1]["rss_peak_bytes"] = Value::Null;
    let aggregated = aggregate_results(&rows);
    assert!(cell(&aggregated, "libmaxminddb-rs", 1000, "rss_peak_bytes").is_err());
    assert!(cell(&aggregated, "libmaxminddb-rs", 1000, "rss_after_open_bytes").is_ok());
    rows[1]["failed"] = json!(true);
    rows[1]["error"] = json!("child failed");
    let aggregated = aggregate_results(&rows);
    assert_eq!(
        cell(&aggregated, "libmaxminddb-rs", 1000, "rss_after_open_bytes").unwrap_err(),
        "child failed"
    );
}

#[test]
fn incompatible_inputs_and_protocols_are_never_plotted_together() {
    let ours = measurement("libmaxminddb-rs", 1000);
    for key in [
        "opening_mode",
        "rss_method",
        "dataset_sha256",
        "workload_sha256",
        "measurement_protocol",
        "lookups",
        "warmup_ops",
    ] {
        let mut changed = ours.clone();
        changed[key] = if ours[key].is_string() {
            json!("different")
        } else {
            json!(2)
        };
        assert_ne!(group_key(&ours), group_key(&changed), "{key}");
    }
    let mut peer = measurement("geoip2-rs", 1000);
    peer["workload_sha256"] = json!("other queries");
    assert!(
        cell(
            &[ours.clone(), peer],
            "libmaxminddb-rs",
            1000,
            "rss_peak_bytes"
        )
        .unwrap_err()
        .contains("Incompatible workload")
    );
    let mut legacy = ours;
    legacy["measurement_protocol"] = json!("memory-rss-v1");
    assert!(cell(&[legacy], "libmaxminddb-rs", 1000, "rss_peak_bytes").is_err());
}

#[test]
fn unavailable_points_break_lines_instead_of_interpolating() {
    let (_, svg) = svg_line_chart(
        "gaps",
        &["1K".into(), "10K".into(), "100K".into()],
        &[("libmaxminddb-rs", vec![Some(1.0), None, Some(3.0)])],
        800,
        400,
        "MiB",
        "",
        true,
    );
    assert!(!svg.contains("<polyline"));
    assert_eq!(svg.matches("</title>").count(), 2);
}
