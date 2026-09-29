use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::config::IMPLEMENTATIONS;
use crate::metrics::{HIGHER_IS_BETTER, LOWER_IS_BETTER, fmt_size_tag};

pub const NUMERIC_FIELDS: &[&str] = &[
    "mean_ns",
    "median_ns",
    "min_ns",
    "max_ns",
    "stddev_ns",
    "p50_ns",
    "p95_ns",
    "p99_ns",
    "throughput_ops_s",
    "allocations_per_op",
    "allocated_bytes_per_op",
    "rss_delta_bytes",
    "avg_rss_bytes",
    "wall_time_ns",
    "insert_time_ns",
    "build_time_ns",
    "database_bytes",
    "peak_rss_bytes",
    "batch_size",
    "ci_lower_ns",
    "ci_upper_ns",
    "change_pct",
    "change_ci_lower",
    "change_ci_upper",
    // memory_rss scenario (see MEMORY_PROTOCOL in config.rs)
    "rss_before_open_bytes",
    "rss_after_open_bytes",
    "rss_peak_bytes",
    "rss_after_bytes",
];

pub fn load_raw_results(path: &Path) -> Vec<Value> {
    if !path.exists() {
        return Vec::new();
    }
    let content = fs::read_to_string(path).unwrap_or_default();
    content
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() {
                return None;
            }
            serde_json::from_str::<Value>(l).ok()
        })
        .filter(|v| v.get("implementation").is_some())
        .collect()
}

pub fn group_key(row: &Value) -> String {
    let criterion_id = row
        .get("criterion_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        row.get("implementation")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        row.get("scenario").and_then(|v| v.as_str()).unwrap_or(""),
        row.get("family").and_then(|v| v.as_str()).unwrap_or(""),
        row.get("pattern").and_then(|v| v.as_str()).unwrap_or(""),
        row.get("workload_size")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        row.get("operation").and_then(|v| v.as_str()).unwrap_or(""),
        row.get("threads").and_then(|v| v.as_u64()).unwrap_or(1),
        row.get("measurement_protocol")
            .and_then(|v| v.as_str())
            .unwrap_or("legacy"),
        row.get("database_size")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        criterion_id,
        row.get("dataset_sha256")
            .and_then(Value::as_str)
            .unwrap_or(""),
        row.get("workload_sha256")
            .and_then(Value::as_str)
            .unwrap_or(""),
        row.get("pinned_cpu")
            .map(Value::to_string)
            .unwrap_or_default(),
        row.get("variant").and_then(Value::as_str).unwrap_or(""),
        row.get("opening_mode")
            .and_then(Value::as_str)
            .unwrap_or(""),
        row.get("rss_method").and_then(Value::as_str).unwrap_or(""),
        row.get("lookups").and_then(Value::as_u64).unwrap_or(0),
        row.get("warmup_ops").and_then(Value::as_u64).unwrap_or(0),
    )
}

fn median(mut vals: Vec<f64>) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = vals.len() / 2;
    if vals.len() % 2 == 1 {
        vals[mid]
    } else {
        (vals[mid - 1] + vals[mid]) / 2.0
    }
}

pub fn aggregate_results(rows: &[Value]) -> Vec<Value> {
    let mut groups: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for row in rows {
        groups.entry(group_key(row)).or_default().push(row);
    }

    let mut aggregated = Vec::new();
    for (_k, group) in groups {
        if group.is_empty() {
            continue;
        }
        let mut base = group[0].as_object().cloned().unwrap_or_default();

        for &field in NUMERIC_FIELDS {
            let vals: Vec<f64> = group
                .iter()
                .filter_map(|r| r.get(field).and_then(|v| v.as_f64()))
                .collect();
            if !vals.is_empty() {
                let med = median(vals);
                if med.fract() == 0.0 && med <= u64::MAX as f64 && med >= 0.0 {
                    base.insert(field.to_string(), Value::from(med as u64));
                } else {
                    base.insert(
                        field.to_string(),
                        serde_json::Number::from_f64(med)
                            .map(Value::Number)
                            .unwrap_or(Value::Null),
                    );
                }
            }
        }

        if base.get("scenario").and_then(Value::as_str) == Some("memory_rss") {
            // Never hide a failed repeat or an unavailable kernel high-water mark.
            if let Some(failed) = group
                .iter()
                .find(|r| r["failed"] == true || r["unsupported"] == true)
            {
                base.insert("failed".into(), Value::Bool(true));
                base.insert(
                    "error".into(),
                    Value::String(
                        failed["error"]
                            .as_str()
                            .unwrap_or("RSS repeat unavailable")
                            .into(),
                    ),
                );
                for key in [
                    "rss_before_open_bytes",
                    "rss_after_open_bytes",
                    "rss_peak_bytes",
                    "rss_after_bytes",
                ] {
                    base.remove(key);
                }
            } else if group.iter().any(|r| r["rss_peak_bytes"].as_u64().is_none()) {
                base.insert("rss_peak_bytes".into(), Value::Null);
                base.insert(
                    "peak_unavailable_reason".into(),
                    Value::String("At least one repeat has no kernel lookup peak".into()),
                );
            } else if let Some(peak) = group
                .iter()
                .filter_map(|r| r["rss_peak_bytes"].as_u64())
                .max()
            {
                base.insert("rss_peak_bytes".into(), Value::from(peak));
            }
        }

        let rates: Vec<f64> = group
            .iter()
            .filter_map(|r| r.get("throughput_ops_s").and_then(|v| v.as_f64()))
            .collect();
        if !rates.is_empty() {
            let min_r = rates.iter().copied().fold(f64::INFINITY, f64::min);
            let max_r = rates.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let sum_r: f64 = rates.iter().sum();
            let mean_r = sum_r / rates.len() as f64;
            base.insert(
                "throughput_run_min".to_string(),
                serde_json::Number::from_f64(min_r)
                    .map(Value::Number)
                    .unwrap_or(Value::Null),
            );
            base.insert(
                "throughput_run_max".to_string(),
                serde_json::Number::from_f64(max_r)
                    .map(Value::Number)
                    .unwrap_or(Value::Null),
            );
            base.insert(
                "throughput_run_mean".to_string(),
                serde_json::Number::from_f64(mean_r)
                    .map(Value::Number)
                    .unwrap_or(Value::Null),
            );
        }

        base.insert("run_count".to_string(), Value::from(group.len() as u64));
        aggregated.push(Value::Object(base));
    }

    aggregated.sort_by(|a, b| {
        let sc_a = a.get("scenario").and_then(|v| v.as_str()).unwrap_or("");
        let sc_b = b.get("scenario").and_then(|v| v.as_str()).unwrap_or("");
        let fam_a = a.get("family").and_then(|v| v.as_str()).unwrap_or("");
        let fam_b = b.get("family").and_then(|v| v.as_str()).unwrap_or("");
        let pat_a = a.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let pat_b = b.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let db_a = a.get("database_size").and_then(|v| v.as_u64()).unwrap_or(0);
        let db_b = b.get("database_size").and_then(|v| v.as_u64()).unwrap_or(0);
        let wl_a = a.get("workload_size").and_then(|v| v.as_u64()).unwrap_or(0);
        let wl_b = b.get("workload_size").and_then(|v| v.as_u64()).unwrap_or(0);
        let th_a = a.get("threads").and_then(|v| v.as_u64()).unwrap_or(1);
        let th_b = b.get("threads").and_then(|v| v.as_u64()).unwrap_or(1);

        let idx_a = IMPLEMENTATIONS
            .iter()
            .position(|&x| {
                x == a
                    .get("implementation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            })
            .unwrap_or(999);
        let idx_b = IMPLEMENTATIONS
            .iter()
            .position(|&x| {
                x == b
                    .get("implementation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            })
            .unwrap_or(999);

        (sc_a, fam_a, pat_a, db_a, wl_a, th_a, idx_a)
            .cmp(&(sc_b, fam_b, pat_b, db_b, wl_b, th_b, idx_b))
    });

    aggregated
}

pub fn rows_by_impl(rows: &[&Value]) -> BTreeMap<String, Value> {
    let mut map = BTreeMap::new();
    for r in rows {
        if let Some(impl_name) = r.get("implementation").and_then(|v| v.as_str()) {
            map.insert(impl_name.to_string(), (*r).clone());
        }
    }
    map
}

pub fn get_largest_lookup<'a>(rows: &'a [Value], family: &str, pattern: &str) -> Vec<&'a Value> {
    let candidates: Vec<&'a Value> = rows
        .iter()
        .filter(|r| {
            r.get("scenario").and_then(|v| v.as_str()) == Some("lookup")
                && r.get("family").and_then(|v| v.as_str()) == Some(family)
                && r.get("pattern").and_then(|v| v.as_str()) == Some(pattern)
        })
        .collect();

    if candidates.is_empty() {
        return Vec::new();
    }

    let max_size = candidates
        .iter()
        .map(|r| r.get("workload_size").and_then(|v| v.as_u64()).unwrap_or(0))
        .max()
        .unwrap_or(0);

    candidates
        .into_iter()
        .filter(|r| r.get("workload_size").and_then(|v| v.as_u64()).unwrap_or(0) == max_size)
        .collect()
}

pub fn get_concurrent_rows<'a>(rows: &'a [Value], family: &str) -> Vec<&'a Value> {
    rows.iter()
        .filter(|r| {
            let sc = r.get("scenario").and_then(|v| v.as_str()).unwrap_or("");
            (sc == "lookup_concurrent" || sc == "lookup")
                && r.get("family").and_then(|v| v.as_str()) == Some(family)
        })
        .collect()
}

/// One API variant in the random 1M-address concurrent lookup suite.
pub fn get_lookup_api_row<'a>(
    rows: &'a [Value],
    family: &str,
    threads: usize,
    operation: &str,
) -> Option<&'a Value> {
    rows.iter().find(|r| {
        r.get("implementation").and_then(Value::as_str) == Some("libmaxminddb-rs")
            && r.get("scenario")
                .and_then(Value::as_str)
                .is_some_and(|scenario| scenario.starts_with("lookup_concurrent"))
            && r.get("operation").and_then(Value::as_str) == Some(operation)
            && r.get("family").and_then(Value::as_str) == Some(family)
            && r.get("pattern").and_then(Value::as_str) == Some("random")
            && r.get("workload_size").and_then(Value::as_u64) == Some(1_000_000)
            && r.get("threads").and_then(Value::as_u64) == Some(threads as u64)
            && r.get("failed").and_then(Value::as_bool) != Some(true)
    })
}

pub fn get_scaling_rows<'a>(rows: &'a [Value]) -> Vec<&'a Value> {
    rows.iter()
        .filter(|r| {
            let sc = r.get("scenario").and_then(|v| v.as_str()).unwrap_or("");
            let label = r
                .get("label_scenario")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            sc == "db_size_scaling" || label == "db_size_scaling"
        })
        .collect()
}

pub fn get_writer_rows<'a>(rows: &'a [Value]) -> Vec<&'a Value> {
    rows.iter()
        .filter(|r| {
            let sc = r.get("scenario").and_then(|v| v.as_str()).unwrap_or("");
            sc == "writer_generation" || sc.contains("writer")
        })
        .collect()
}

/// Rows emitted by the per-size resident-memory harnesses
/// (scenario `memory_rss`, protocol `memory-rss-v1`).
pub fn get_memory_rows<'a>(rows: &'a [Value]) -> Vec<&'a Value> {
    rows.iter()
        .filter(|r| {
            let sc = r.get("scenario").and_then(|v| v.as_str()).unwrap_or("");
            match sc {
                "memory_rss" => true,
                // Some older aggregation paths retag sections rather than
                // scenarios; accept the label too.
                _ => r.get("label_scenario").and_then(|v| v.as_str()) == Some("memory_rss"),
            }
        })
        .collect()
}

pub struct SummaryItem {
    pub scenario: String,
    pub metric_key: String,
    pub metric_label: String,
    pub unit: String,
    pub direction: &'static str,
    pub rows: BTreeMap<String, Value>,
}

pub fn extract_summary_items(aggregated_rows: &[Value]) -> Vec<SummaryItem> {
    let mut items = Vec::new();

    // 1. IPv4 Random Lookup
    let rows_ipv4 = get_largest_lookup(aggregated_rows, "ipv4", "random");
    if !rows_ipv4.is_empty() {
        items.push(SummaryItem {
            scenario: "IPv4 Random Lookup (1M)".into(),
            metric_key: "p99_ns".into(),
            metric_label: "p99 Tail Latency".into(),
            unit: "ns".into(),
            direction: LOWER_IS_BETTER,
            rows: rows_by_impl(&rows_ipv4),
        });
        items.push(SummaryItem {
            scenario: "IPv4 Random Throughput (1M)".into(),
            metric_key: "throughput_ops_s".into(),
            metric_label: "Peak Throughput".into(),
            unit: "ops/s".into(),
            direction: HIGHER_IS_BETTER,
            rows: rows_by_impl(&rows_ipv4),
        });
    }

    // 2. IPv6 Random Lookup
    let rows_ipv6 = get_largest_lookup(aggregated_rows, "ipv6", "random");
    if !rows_ipv6.is_empty() {
        items.push(SummaryItem {
            scenario: "IPv6 Random Lookup (1M)".into(),
            metric_key: "p99_ns".into(),
            metric_label: "p99 Tail Latency".into(),
            unit: "ns".into(),
            direction: LOWER_IS_BETTER,
            rows: rows_by_impl(&rows_ipv6),
        });
        items.push(SummaryItem {
            scenario: "IPv6 Random Throughput (1M)".into(),
            metric_key: "throughput_ops_s".into(),
            metric_label: "Peak Throughput".into(),
            unit: "ops/s".into(),
            direction: HIGHER_IS_BETTER,
            rows: rows_by_impl(&rows_ipv6),
        });
    }

    // 3. Multi-threaded Throughput (16T IPv4)
    let rows_16t: Vec<&Value> = aggregated_rows
        .iter()
        .filter(|r| {
            let sc = r.get("scenario").and_then(|v| v.as_str()).unwrap_or("");
            (sc == "lookup_concurrent" || sc == "lookup")
                && r.get("family").and_then(|v| v.as_str()) == Some("ipv4")
                && r.get("threads").and_then(|v| v.as_u64()) == Some(16)
        })
        .collect();
    if !rows_16t.is_empty() {
        items.push(SummaryItem {
            scenario: "16-Thread Concurrent IPv4".into(),
            metric_key: "throughput_ops_s".into(),
            metric_label: "Concurrent Throughput".into(),
            unit: "ops/s".into(),
            direction: HIGHER_IS_BETTER,
            rows: rows_by_impl(&rows_16t),
        });
    }

    // 4. Memory Allocations (Allocations per Op)
    if !rows_ipv4.is_empty() {
        items.push(SummaryItem {
            scenario: "IPv4 Heap Allocations".into(),
            metric_key: "allocations_per_op".into(),
            metric_label: "Allocations / Op".into(),
            unit: "allocs".into(),
            direction: LOWER_IS_BETTER,
            rows: rows_by_impl(&rows_ipv4),
        });
    }

    // 5. Open mmap latency
    let rows_mmap: Vec<&Value> = aggregated_rows
        .iter()
        .filter(|r| r.get("scenario").and_then(|v| v.as_str()) == Some("open_mmap"))
        .collect();
    if !rows_mmap.is_empty() {
        items.push(SummaryItem {
            scenario: "Database Open (mmap)".into(),
            metric_key: "median_ns".into(),
            metric_label: "Median Latency".into(),
            unit: "µs".into(),
            direction: LOWER_IS_BETTER,
            rows: rows_by_impl(&rows_mmap),
        });
    }

    // 6. Writer Generation Summary
    let writer_rows = get_writer_rows(aggregated_rows);
    if !writer_rows.is_empty() {
        let max_size = writer_rows
            .iter()
            .map(|r| {
                r.get("entries")
                    .or_else(|| r.get("database_size"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0);
        let largest_writer: Vec<&Value> = writer_rows
            .iter()
            .copied()
            .filter(|r| {
                r.get("entries")
                    .or_else(|| r.get("database_size"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    == max_size
            })
            .collect();
        if !largest_writer.is_empty() {
            let tag = fmt_size_tag(max_size as usize);
            items.push(SummaryItem {
                scenario: format!("Writer Generation ({tag})"),
                metric_key: "throughput_ops_s".into(),
                metric_label: "Insert Throughput".into(),
                unit: "ops/s".into(),
                direction: HIGHER_IS_BETTER,
                rows: rows_by_impl(&largest_writer),
            });
            items.push(SummaryItem {
                scenario: format!("Writer Total Time ({tag})"),
                metric_key: "wall_time_ns".into(),
                metric_label: "Total Duration".into(),
                unit: "s".into(),
                direction: LOWER_IS_BETTER,
                rows: rows_by_impl(&largest_writer),
            });
            items.push(SummaryItem {
                scenario: format!("Writer Peak RSS ({tag})"),
                metric_key: "peak_rss_bytes".into(),
                metric_label: "Peak Memory (RSS)".into(),
                unit: "MB".into(),
                direction: LOWER_IS_BETTER,
                rows: rows_by_impl(&largest_writer),
            });
        }
    }

    items
}

pub fn export_results_json(aggregated: &[Value], path: &Path) -> Result<(), String> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let s = serde_json::to_string_pretty(aggregated).map_err(|e| e.to_string())?;
    fs::write(path, s).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn export_results_csv(aggregated: &[Value], path: &Path) -> Result<(), String> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let columns: Vec<String> = vec![
        "implementation".into(),
        "scenario".into(),
        "family".into(),
        "pattern".into(),
        "workload_size".into(),
        "database_size".into(),
        "threads".into(),
        "throughput_ops_s".into(),
        "mean_ns".into(),
        "median_ns".into(),
        "p50_ns".into(),
        "p95_ns".into(),
        "p99_ns".into(),
        "min_ns".into(),
        "max_ns".into(),
        "stddev_ns".into(),
        "allocations_per_op".into(),
        "allocated_bytes_per_op".into(),
        "rss_delta_bytes".into(),
        "peak_rss_bytes".into(),
        "rss_before_open_bytes".into(),
        "rss_after_open_bytes".into(),
        "rss_peak_bytes".into(),
        "rss_after_bytes".into(),
        "lookups".into(),
        "opening_mode".into(),
        "wall_time_ns".into(),
        "insert_time_ns".into(),
        "build_time_ns".into(),
        "database_bytes".into(),
        "run_count".into(),
    ];

    let mut w = String::new();
    w.push_str(&columns.join(","));
    w.push('\n');

    for row in aggregated {
        let mut line = Vec::new();
        for col in &columns {
            let val = match row.get(col) {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                Some(Value::Bool(b)) => b.to_string(),
                _ => String::new(),
            };
            line.push(val);
        }
        w.push_str(&line.join(","));
        w.push('\n');
    }

    fs::write(path, w).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn find_winners_and_slowest<'a>(
    vals: &[(&'a str, f64)],
    lower_is_better: bool,
) -> (Vec<&'a str>, Vec<&'a str>) {
    if vals.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let best_val = if lower_is_better {
        vals.iter().map(|x| x.1).fold(f64::INFINITY, f64::min)
    } else {
        vals.iter().map(|x| x.1).fold(f64::NEG_INFINITY, f64::max)
    };
    let worst_val = if lower_is_better {
        vals.iter().map(|x| x.1).fold(f64::NEG_INFINITY, f64::max)
    } else {
        vals.iter().map(|x| x.1).fold(f64::INFINITY, f64::min)
    };

    // If all values are identical (or within epsilon), everyone is best, and no one is slowest!
    if (best_val - worst_val).abs() < 1e-9 {
        let winners = vals.iter().map(|x| x.0).collect();
        return (winners, Vec::new());
    }

    let winners: Vec<&'a str> = vals
        .iter()
        .filter(|x| (x.1 - best_val).abs() < 1e-9)
        .map(|x| x.0)
        .collect();
    let slowest: Vec<&'a str> = vals
        .iter()
        .filter(|x| (x.1 - worst_val).abs() < 1e-9)
        .map(|x| x.0)
        .collect();
    (winners, slowest)
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    #[test]
    fn comparison_variants_and_affinity_are_not_averaged_together() {
        let base = serde_json::json!({
            "implementation": "libmaxminddb-rs", "scenario": "lookup",
            "family": "ipv6", "pattern": "absent", "workload_size": 1_000_000,
            "operation": "lookup_borrowed", "threads": 1,
            "measurement_protocol": "reader-replay-affinity-v1",
            "pinned_cpu": 2, "variant": "before", "throughput_ops_s": 1_000_000.0,
        });
        let mut after = base.clone();
        after["variant"] = Value::from("after");
        let mut unpinned = after.clone();
        unpinned["pinned_cpu"] = Value::Null;
        assert_eq!(
            aggregate_results(&[base.clone(), base, after, unpinned]).len(),
            3
        );
    }
}
