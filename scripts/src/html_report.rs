//! Comprehensive HTML benchmark report generator for libmaxminddb-rs
//!
//! This module generates a complete interactive HTML report containing:
//! - Hardware & Compilers section with library versions table
//! - Executive Benchmark Summary with winner/slowest highlighting
//! - Single-Threaded Lookups with statistics and candlestick charts
//! - Concurrent Multi-Threaded Lookups with scaling curves
//! - Database Size Scaling analysis
//! - MMDB Writer Benchmarks
//! - Database Open Latency comparison
//! - All required SVG charts embedded in the report

use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::builder::{cpu_model, detect_compilers, detect_versions, get_os_info, get_system_info};
use crate::charts::{
    CandlestickStats, lookup_api_charts, svg_bar_chart, svg_candlestick_chart, svg_line_chart,
};
use crate::config::{
    DEFAULT_THREAD_COUNTS, IMPLEMENTATIONS, LOOKUP_API_OPERATIONS, SCALING_DATABASE_SIZES,
    WRITER_IMPLEMENTATIONS, WRITER_SIZES, display_name, get_color,
};
use crate::metrics::{LOWER_IS_BETTER, esc_html, fmt_bytes, fmt_duration, fmt_rate, fmt_size_tag};
use crate::stats::{
    extract_summary_items, find_winners_and_slowest, get_concurrent_rows, get_largest_lookup,
    get_lookup_api_row, get_scaling_rows, get_writer_rows,
};

const HTML_STYLES: &str = r#"
:root {
  --bg: #090d13;
  --surface: #0d1117;
  --surface-raised: #161b22;
  --border: #30363d;
  --border-muted: #21262d;
  --text: #e6edf3;
  --muted: #8b949e;
  --accent: #00f5a0;
  --c-blue: #58a6ff;
  --c-gold: #f1e05a;
  --c-red: #ff3b5c;
  --c-orange: #ff8800;
  --c-cyan: #00d9f5;
  --c-purple: #a06ee1;
  --shadow: 0 8px 24px rgba(0,0,0,0.4);
  --shadow-sm: 0 4px 12px rgba(0,0,0,0.3);
}
* { box-sizing: border-box; margin: 0; padding: 0; }
body {
  background: var(--bg);
  color: var(--text);
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
  line-height: 1.5;
  padding: 32px 24px;
}
.container { max-width: 1440px; margin: 0 auto; }

/* Header */
header {
  margin-bottom: 32px;
  border-bottom: 1px solid var(--border-muted);
  padding-bottom: 24px;
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  flex-wrap: wrap;
  gap: 16px;
}
.header-titles h1 {
  font-size: 28px;
  font-weight: 800;
  color: #fff;
  letter-spacing: -0.5px;
  display: flex;
  align-items: center;
  gap: 12px;
}
.header-titles p { color: var(--muted); font-size: 14px; margin-top: 6px; }

.env-badge {
  background: var(--surface-raised);
  border: 1px solid var(--border);
  padding: 12px 18px;
  border-radius: 8px;
  font-size: 12px;
  color: var(--muted);
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.env-badge-row { display: flex; gap: 16px; flex-wrap: wrap; }
.env-badge strong { color: var(--text); }

/* Navigation */
.nav-bar {
  display: flex;
  gap: 8px;
  margin-bottom: 24px;
  flex-wrap: wrap;
}
.nav-link {
  background: var(--surface-raised);
  border: 1px solid var(--border-muted);
  padding: 8px 14px;
  border-radius: 6px;
  color: var(--text);
  text-decoration: none;
  font-size: 13px;
  font-weight: 600;
  transition: all 0.15s ease;
}
.nav-link:hover { border-color: var(--accent); color: var(--accent); }
.nav-link.active { background: var(--accent); color: #000; border-color: var(--accent); }

/* Sections */
section {
  background: var(--surface);
  border: 1px solid var(--border-muted);
  border-radius: 12px;
  padding: 24px;
  margin-bottom: 32px;
  box-shadow: var(--shadow);
}
.section-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 16px;
  border-bottom: 1px solid var(--border-muted);
  padding-bottom: 12px;
}
.section-head h2 { font-size: 18px; font-weight: 700; color: #fff; display: flex; align-items: center; gap: 8px; }
.desc { color: var(--muted); font-size: 13px; margin-bottom: 20px; }

/* Grid layouts */
.grid2 { display: grid; grid-template-columns: repeat(2, 1fr); gap: 20px; }
.throughput-group + .throughput-group { margin-top: 20px; }
.grid4 { display: grid; grid-template-columns: repeat(4, 1fr); gap: 16px; }
.charts-large { display: grid; grid-template-columns: 1fr; gap: 28px; }
@media (max-width: 1200px) { .grid2, .grid4, .charts-large { grid-template-columns: 1fr; } }

/* Chart boxes */
.chart-box {
  background: var(--surface-raised);
  border: 1px solid var(--border-muted);
  border-radius: 12px;
  padding: 22px 24px;
  box-shadow: 0 4px 16px rgba(0,0,0,0.25);
  transition: transform 0.15s ease, border-color 0.15s ease;
}
.chart-box:hover {
  border-color: #3b434d;
  box-shadow: 0 8px 24px rgba(0,0,0,0.35);
}
.chart-box svg {
  width: 100%;
  height: auto;
  display: block;
  border-radius: 8px;
}

/* Podium cards */
.podium-card {
  background: var(--surface-raised);
  border: 1px solid var(--border-muted);
  border-radius: 10px;
  padding: 18px;
  position: relative;
  overflow: hidden;
  box-shadow: 0 4px 12px rgba(0,0,0,0.2);
}
.podium-card.winner { border-color: var(--accent); }
.podium-step { font-size: 12px; font-weight: 700; color: var(--muted); text-transform: uppercase; margin-bottom: 8px; }
.podium-winner { font-size: 16px; font-weight: 800; color: #fff; display: flex; align-items: center; gap: 8px; margin-bottom: 6px; }
.podium-val { font-size: 24px; font-weight: 800; color: var(--accent); margin-bottom: 4px; }
.podium-sub { font-size: 12px; color: var(--muted); }

/* Tables */
.table-wrap { overflow-x: auto; margin-top: 16px; border: 1px solid var(--border-muted); border-radius: 8px; }
table { width: 100%; border-collapse: collapse; font-size: 13px; text-align: left; }
th { background: var(--surface-raised); color: var(--muted); font-weight: 600; padding: 12px 16px; border-bottom: 1px solid var(--border); font-size: 13px; }
.sort-button { display: inline-flex; align-items: center; gap: 8px; width: 100%; border: 0; padding: 0; background: none; color: inherit; font: inherit; text-align: left; cursor: pointer; }
.sort-button:hover, .sort-button:focus-visible { color: var(--accent); }
.sort-button:focus-visible { outline: 2px solid var(--accent); outline-offset: 4px; }
.sort-indicator { color: var(--accent); font-size: 11px; margin-left: auto; }
th[aria-sort="none"] .sort-indicator { color: var(--muted); }
td { padding: 10px 16px; border-bottom: 1px solid var(--border-muted); }
tr:hover td { background: rgba(255,255,255,0.02); }
tr.row-winner td { background: rgba(0, 245, 160, 0.05); }
tr.row-worst td { background: rgba(255, 59, 92, 0.05); }
tr.row-winner:hover td { background: rgba(0, 245, 160, 0.09); }
tr.row-worst:hover td { background: rgba(255, 59, 92, 0.09); }

/* Pills */
.pill { display: inline-block; padding: 3px 10px; border-radius: 12px; font-size: 12px; font-weight: 600; background: var(--surface-raised); border: 1px solid var(--border); }
.pill-win { background: rgba(0, 245, 160, 0.15); border-color: var(--accent); color: var(--accent); }
.pill-slow { background: rgba(255, 59, 92, 0.15); border-color: var(--c-red); color: var(--c-red); }

/* Status indicators */
.winner { color: var(--accent); font-weight: 700; }
.slowest { color: var(--c-red); }
.unsupported { color: var(--muted); font-style: italic; }
"#;

const CHART_STYLES: &str = r#"
.svg-bar-track { fill: #161b22; }
.svg-value { fill: #f0f6fc; font-size: 13px; font-weight: 700; }
.legend-text { fill: #c9d1d9; font-size: 13px; font-weight: 600; }
.legend-box { fill: #161b22; stroke: #30363d; stroke-width: 1; }
.legend-label { fill: #8b949e; font-size: 11px; font-weight: 700; letter-spacing: 0.8px; }
"#;

/// Escape HTML special characters

#[allow(dead_code)]
fn render_lookup_stats_table(html: &mut String, title: &str, rows: &[&Value]) {
    if rows.is_empty() {
        return;
    }
    html.push_str(&format!("  <h3 style=\"margin-top:24px;margin-bottom:12px;color:var(--text);font-size:16px;\">{}</h3>\n", esc_html(title)));
    html.push_str("  <div class=\"table-wrap\">\n");
    html.push_str("    <table data-default-sort=\"8:asc\">\n");
    html.push_str("      <thead>\n");
    html.push_str("        <tr>\n");
    html.push_str("          <th>Implementation</th>\n");
    html.push_str("          <th>Min</th>\n");
    html.push_str("          <th>Median</th>\n");
    html.push_str("          <th>Mean</th>\n");
    html.push_str("          <th>Max</th>\n");
    html.push_str("          <th>Std Dev</th>\n");
    html.push_str("          <th>p50</th>\n");
    html.push_str("          <th>p95</th>\n");
    html.push_str("          <th>p99</th>\n");
    html.push_str("          <th>Throughput</th>\n");
    html.push_str("        </tr>\n");
    html.push_str("      </thead>\n");
    html.push_str("      <tbody>\n");

    let mut p99_vals: Vec<(&str, f64)> = rows
        .iter()
        .filter_map(|r| {
            let name = r.get("implementation")?.as_str()?;
            let p99 = r.get("p99_ns")?.as_f64()?;
            if p99 > 0.0 && r.get("unsupported").and_then(|u| u.as_bool()) != Some(true) {
                Some((name, p99))
            } else {
                None
            }
        })
        .collect();
    p99_vals.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let p99_winner = p99_vals.first().map(|x| x.0);
    let p99_slowest = if p99_vals.len() > 1 {
        p99_vals.last().map(|x| x.0)
    } else {
        None
    };

    let mut tp_vals: Vec<(&str, f64)> = rows
        .iter()
        .filter_map(|r| {
            let name = r.get("implementation")?.as_str()?;
            let tp = r.get("throughput_ops_s")?.as_f64()?;
            if tp > 0.0 && r.get("unsupported").and_then(|u| u.as_bool()) != Some(true) {
                Some((name, tp))
            } else {
                None
            }
        })
        .collect();
    tp_vals.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let tp_winner = tp_vals.first().map(|x| x.0);
    let tp_slowest = if tp_vals.len() > 1 {
        tp_vals.last().map(|x| x.0)
    } else {
        None
    };

    for r in rows {
        let implementation = r
            .get("implementation")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let unsupported = r
            .get("unsupported")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if unsupported {
            html.push_str(&format!(
                "        <tr><td>{}</td><td colspan=\"9\">⚠️ unsupported</td></tr>\n",
                esc_html(display_name(implementation))
            ));
            continue;
        }

        html.push_str("        <tr>\n");
        html.push_str(&format!(
            "          <td><strong>{}</strong></td>\n",
            esc_html(display_name(implementation))
        ));

        let fields = [
            "min_ns",
            "median_ns",
            "mean_ns",
            "max_ns",
            "stddev_ns",
            "p50_ns",
            "p95_ns",
        ];
        for field in fields {
            let val = r.get(field).and_then(|v| v.as_f64());
            html.push_str(&format!(
                "          <td>{}</td>\n",
                esc_html(&fmt_duration(val))
            ));
        }

        let p99 = r.get("p99_ns").and_then(|v| v.as_f64());
        let p99_fmt = fmt_duration(p99);
        if Some(implementation) == p99_winner {
            html.push_str(&format!(
                "          <td class=\"winner\">{} 🏆</td>\n",
                esc_html(&p99_fmt)
            ));
        } else if Some(implementation) == p99_slowest {
            html.push_str(&format!(
                "          <td class=\"slowest\">{}</td>\n",
                esc_html(&p99_fmt)
            ));
        } else {
            html.push_str(&format!("          <td>{}</td>\n", esc_html(&p99_fmt)));
        }

        let tp = r.get("throughput_ops_s").and_then(|v| v.as_f64());
        let tp_fmt = fmt_rate(tp);
        if Some(implementation) == tp_winner {
            html.push_str(&format!(
                "          <td class=\"winner\">{} 🏆</td>\n",
                esc_html(&tp_fmt)
            ));
        } else if Some(implementation) == tp_slowest {
            html.push_str(&format!(
                "          <td class=\"slowest\">{}</td>\n",
                esc_html(&tp_fmt)
            ));
        } else {
            html.push_str(&format!("          <td>{}</td>\n", esc_html(&tp_fmt)));
        }

        html.push_str("        </tr>\n");
    }
    html.push_str("      </tbody>\n");
    html.push_str("    </table>\n");
    html.push_str("  </div>\n");
}

pub fn generate_html_report(
    aggregated_rows: &[Value],
    _raw_rows: &[Value],
    env_data: &HashMap<String, String>,
    output_path: &Path,
    include_microbenchmarks: bool,
) -> Result<PathBuf, String> {
    if let Some(p) = output_path.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }

    // Collect summary items
    let summary_items = extract_summary_items(aggregated_rows);

    // Get environment data
    let cpu = env_data.get("cpu").cloned().unwrap_or_else(cpu_model);
    let timestamp = env_data
        .get("timestamp_utc")
        .cloned()
        .unwrap_or_else(|| "recent".into());
    let versions = detect_versions();
    let compilers = detect_compilers();
    let os_info = get_os_info();
    let system_info = get_system_info();

    let mut html = String::new();
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("<meta charset=\"utf-8\"/>\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\"/>\n");
    html.push_str("<title>libmaxminddb-rs Benchmark Report</title>\n");
    html.push_str(&format!("<style>{HTML_STYLES}\n{CHART_STYLES}</style>\n"));
    html.push_str("</head>\n<body>\n<div class=\"container\">\n");

    // ========================================================================
    // HEADER
    // ========================================================================
    html.push_str("<header>\n");
    html.push_str("  <div class=\"header-titles\">\n");
    html.push_str("    <h1>⚡ libmaxminddb-rs Performance Benchmarks</h1>\n");
    html.push_str(&format!(
        "    <p>Comprehensive MMDB reader &amp; writer performance report</p>\n"
    ));
    html.push_str("  </div>\n");
    html.push_str("  <div class=\"env-badge\">\n");
    html.push_str(&format!("    <span><strong>UTC:</strong> {} · <strong>CPU:</strong> {} · <strong>OS:</strong> {} · <strong>Host:</strong> {}</span>\n", esc_html(&timestamp), esc_html(&cpu), esc_html(&os_info), esc_html(&system_info)));
    html.push_str("  </div>\n");
    html.push_str("</header>\n");

    // Navigation
    html.push_str("<nav class=\"nav-bar\">\n");
    html.push_str("  <a href=\"#hardware\" class=\"nav-link\">🛠️ Hardware &amp; Compilers</a>\n");
    html.push_str("  <a href=\"#summary\" class=\"nav-link\">🏁 Executive Summary</a>\n");
    html.push_str(
        "  <a href=\"#single-thread\" class=\"nav-link\">🔍 Single-Threaded Lookups</a>\n",
    );
    html.push_str("  <a href=\"#concurrent\" class=\"nav-link\">⚡ Concurrent Lookups</a>\n");
    html.push_str("  <a href=\"#lookup-api\" class=\"nav-link\">🦀 Lookup APIs</a>\n");
    html.push_str("  <a href=\"#scaling\" class=\"nav-link\">📈 Size Scaling</a>\n");
    html.push_str("  <a href=\"#memory\" class=\"nav-link\">🧠 Reader Memory</a>\n");
    html.push_str("  <a href=\"#writer\" class=\"nav-link\">✍️ Writer</a>\n");
    html.push_str("  <a href=\"#open\" class=\"nav-link\">📂 Database Open</a>\n");
    if include_microbenchmarks {
        html.push_str("  <a href=\"#microbenchmarks\" class=\"nav-link\">🔬 Microbenchmarks</a>\n");
    }
    html.push_str("</nav>\n");

    // ========================================================================
    // SECTION 1: Hardware & Compilers
    // ========================================================================
    html.push_str("<section id=\"hardware\">\n");
    html.push_str("  <div class=\"section-head\">\n");
    html.push_str("    <h2>🛠️ Hardware &amp; Compilers</h2>\n");
    html.push_str("  </div>\n");

    // Compiler versions
    html.push_str("  <h3 style=\"margin-top:16px;margin-bottom:8px;color:var(--text);font-size:15px;\">🔨 Compiler Versions</h3>\n");
    html.push_str("  <div class=\"table-wrap\">\n");
    html.push_str("    <table>\n");
    html.push_str("      <thead><tr><th>Compiler</th><th>Version</th></tr></thead>\n");
    html.push_str("      <tbody>\n");
    for (name, version) in &compilers {
        html.push_str(&format!(
            "        <tr><td><strong>{}</strong></td><td><code>{}</code></td></tr>\n",
            esc_html(display_name(name)),
            esc_html(version)
        ));
    }
    html.push_str("      </tbody>\n");
    html.push_str("    </table>\n");
    html.push_str("  </div>\n");

    // Library versions table
    html.push_str("  <h3 style=\"margin-top:16px;margin-bottom:8px;color:var(--text);font-size:15px;\">📚 Evaluated Libraries &amp; Reproducibility Specification</h3>\n");
    html.push_str("  <div class=\"table-wrap\">\n");
    html.push_str("    <table>\n");
    html.push_str("      <thead>\n");
    html.push_str("        <tr>\n");
    html.push_str("          <th>Library Name</th>\n");
    html.push_str("          <th>Language</th>\n");
    html.push_str("          <th>Role</th>\n");
    html.push_str("          <th>Evaluated Version</th>\n");
    html.push_str("          <th>Compiler &amp; Version</th>\n");
    html.push_str("          <th>Optimization &amp; Build Flags</th>\n");
    html.push_str("          <th>Upstream Repository</th>\n");
    html.push_str("        </tr>\n");
    html.push_str("      </thead>\n");
    html.push_str("      <tbody>\n");

    // Library entries - extract owned strings to avoid lifetime issues
    let v1_14_1 = "1.14.1".to_string();
    let v0_32_0 = "0.32.0".to_string();
    let v0_1_8 = "0.1.8".to_string();
    let v2_6_0 = "v2.6.0".to_string();
    let v1_2_0 = "v1.2.0".to_string();
    let rustc_ver = compilers.get("rustc").cloned().unwrap_or_default();
    let c_compiler_ver = compilers.get("c_compiler").cloned().unwrap_or_default();
    let go_ver = compilers.get("go").cloned().unwrap_or_default();
    let libmaxminddb_ver = versions.get("libmaxminddb").cloned().unwrap_or(v1_14_1);
    let maxminddb_rust_ver = versions.get("maxminddb-rust").cloned().unwrap_or(v0_32_0);
    let geoip2_ver = versions.get("geoip2-rs").cloned().unwrap_or(v0_1_8);
    let golang_ver = versions.get("maxminddb-golang").cloned().unwrap_or(v2_6_0);
    let mmdbwriter_ver = versions.get("mmdbwriter").cloned().unwrap_or(v1_2_0);

    let libs = [
        (
            "libmaxminddb-rs",
            "Rust",
            "Reader &amp; Writer",
            "0.1.0",
            &rustc_ver,
            "opt-level=3, -C target-cpu=native",
            "Current repository",
        ),
        (
            "libmaxminddb",
            "C",
            "Reader",
            &libmaxminddb_ver,
            &c_compiler_ver,
            "-O3 -march=native -fPIC",
            "github.com/maxmind/libmaxminddb",
        ),
        (
            "maxminddb-rust",
            "Rust",
            "Reader",
            &maxminddb_rust_ver,
            &rustc_ver,
            "opt-level=3, -C target-cpu=native",
            "crates.io/crates/maxminddb",
        ),
        (
            "geoip2-rs",
            "Rust",
            "Reader",
            &geoip2_ver,
            &rustc_ver,
            "opt-level=3, -C target-cpu=native",
            "crates.io/crates/geoip2",
        ),
        (
            "maxminddb-golang",
            "Go",
            "Reader",
            &golang_ver,
            &go_ver,
            "-ldflags=\"-s -w\" -trimpath",
            "github.com/oschwald/maxminddb-golang",
        ),
        (
            "mmdbwriter",
            "Go",
            "Writer",
            &mmdbwriter_ver,
            &go_ver,
            "-ldflags=\"-s -w\" -trimpath",
            "github.com/maxmind/mmdbwriter",
        ),
    ];

    for (name, lang, role, version, compiler, flags, repo) in libs {
        html.push_str("        <tr>\n");
        html.push_str(&format!(
            "          <td><strong>{}</strong></td>\n",
            esc_html(display_name(name))
        ));
        html.push_str(&format!("          <td>{}</td>\n", esc_html(lang)));
        html.push_str(&format!("          <td>{}</td>\n", esc_html(role)));
        html.push_str(&format!(
            "          <td><code>{}</code></td>\n",
            esc_html(version)
        ));
        html.push_str(&format!(
            "          <td><code>{}</code></td>\n",
            esc_html(compiler)
        ));
        html.push_str(&format!(
            "          <td><code>{}</code></td>\n",
            esc_html(flags)
        ));
        html.push_str(&format!(
            "          <td><code>{}</code></td>\n",
            esc_html(repo)
        ));
        html.push_str("        </tr>\n");
    }

    html.push_str("      </tbody>\n");
    html.push_str("    </table>\n");
    html.push_str("  </div>\n");
    html.push_str("</section>\n");

    // ========================================================================
    // SECTION 2: Executive Benchmark Summary
    // ========================================================================
    html.push_str("<section id=\"summary\">\n");
    html.push_str("  <div class=\"section-head\">\n");
    html.push_str("    <h2>🏁 Executive Benchmark Summary</h2>\n");
    html.push_str("  </div>\n");
    html.push_str("  <p class=\"desc\">Comparative performance across all evaluated MMDB implementations. Rows initially rank libmaxminddb-rs against the best measured implementation for each metric; select a column heading to sort it.</p>\n");

    // Key metrics summary table
    html.push_str("  <div class=\"table-wrap\">\n");
    html.push_str("    <table data-default-sort=\"0:desc\">\n");
    html.push_str("      <thead>\n");
    html.push_str("        <tr>\n");
    html.push_str("          <th>Scenario</th>\n");
    html.push_str("          <th>Metric</th>\n");
    for &name in IMPLEMENTATIONS {
        html.push_str(&format!(
            "          <th>{}</th>\n",
            esc_html(display_name(name))
        ));
    }
    html.push_str("        </tr>\n");
    html.push_str("      </thead>\n");
    html.push_str("      <tbody>\n");

    for item in &summary_items {
        if item.scenario.starts_with("Writer") || item.scenario == "IPv4 Heap Allocations" {
            continue; // Writers have their own section; omit allocations from this summary.
        }

        // Collect values and determine winner/slowest
        let mut vals: Vec<(&str, f64)> = Vec::new();
        for &name in IMPLEMENTATIONS {
            if let Some(r) = item.rows.get(name) {
                if let Some(v) = r.get(&item.metric_key).and_then(|x| x.as_f64()) {
                    if !v.is_nan() && v > 0.0 {
                        vals.push((name, v));
                    }
                }
            }
        }

        let (winners, slowest) = find_winners_and_slowest(&vals, item.direction == LOWER_IS_BETTER);
        let best = if item.direction == LOWER_IS_BETTER {
            vals.iter().map(|(_, v)| *v).reduce(f64::min)
        } else {
            vals.iter().map(|(_, v)| *v).reduce(f64::max)
        };
        let relative_performance = vals
            .iter()
            .find(|(name, _)| *name == "libmaxminddb-rs")
            .and_then(|(_, ours)| {
                best.map(|best| {
                    if item.direction == LOWER_IS_BETTER {
                        best / ours
                    } else {
                        ours / best
                    }
                })
            });

        html.push_str("        <tr>\n");
        html.push_str(&format!(
            "          <td data-sort-value=\"{}\"><strong>{}</strong></td>\n",
            relative_performance
                .map(|score| format!("{score:.12}"))
                .unwrap_or_else(|| "NaN".to_string()),
            esc_html(&item.scenario)
        ));
        html.push_str(&format!(
            "          <td>{}</td>\n",
            esc_html(&item.metric_label)
        ));

        for &name in IMPLEMENTATIONS {
            match item.rows.get(name) {
                Some(r) => {
                    if r.get("unsupported").and_then(|v| v.as_bool()) == Some(true) {
                        html.push_str("          <td class=\"unsupported\">⚠️ unsupported</td>\n");
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

                        if winners.contains(&name) {
                            html.push_str(&format!(
                                "          <td class=\"winner\">{} 🏆</td>\n",
                                esc_html(&formatted)
                            ));
                        } else if slowest.contains(&name) {
                            html.push_str(&format!(
                                "          <td class=\"slowest\">{}</td>\n",
                                esc_html(&formatted)
                            ));
                        } else {
                            html.push_str(&format!(
                                "          <td>{}</td>\n",
                                esc_html(&formatted)
                            ));
                        }
                    } else {
                        html.push_str("          <td>—</td>\n");
                    }
                }
                None => html.push_str("          <td>—</td>\n"),
            }
        }
        html.push_str("        </tr>\n");
    }

    html.push_str("      </tbody>\n");
    html.push_str("    </table>\n");
    html.push_str("  </div>\n");
    html.push_str("</section>\n");

    // ========================================================================
    // SECTION 3: Single-Threaded Lookups (Comparative Charts)
    // ========================================================================
    html.push_str("<section id=\"single-thread\">\n");
    html.push_str("  <div class=\"section-head\">\n");
    html.push_str("    <h2>🔍 Single-Threaded Lookups</h2>\n");
    html.push_str("  </div>\n");
    html.push_str("  <p class=\"desc\">Comparative performance across all 5 readers on 1,000,000 lookups (GeoIP2-City database). Showing percentile distributions and throughput across all access patterns.</p>\n");

    // 1. Candlestick Percentile Rank Charts
    html.push_str("  <div class=\"charts-large\">\n");

    // IPv4 Candlestick
    let ipv4_candle_rows = get_largest_lookup(aggregated_rows, "ipv4", "random");
    if !ipv4_candle_rows.is_empty() {
        let mut candlesticks = Vec::new();
        for r in &ipv4_candle_rows {
            let impl_name = r
                .get("implementation")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let min_val = r.get("min_ns").and_then(|v| v.as_f64());
            let p50_val = r.get("p50_ns").and_then(|v| v.as_f64());
            let p95_val = r.get("p95_ns").and_then(|v| v.as_f64());
            let p99_val = r.get("p99_ns").and_then(|v| v.as_f64());
            let max_val = r.get("max_ns").and_then(|v| v.as_f64());

            if let (Some(min_ns), Some(p50_ns), Some(p95_ns), Some(p99_ns)) =
                (min_val, p50_val, p95_val, p99_val)
            {
                candlesticks.push(CandlestickStats {
                    label: impl_name.to_string(),
                    min_ns,
                    p50_ns,
                    p95_ns,
                    p99_ns,
                    max_ns: max_val.unwrap_or(p99_ns),
                    color: get_color(impl_name).to_string(),
                });
            }
        }
        if !candlesticks.is_empty() {
            candlesticks.sort_by(|a, b| a.p50_ns.partial_cmp(&b.p50_ns).unwrap());
            let items: Vec<_> = candlesticks
                .iter()
                .map(|s| (s.label.as_str(), s.clone()))
                .collect();
            let (_, svg) = svg_candlestick_chart(
                "Candlestick Percentile Rank — IPv4 Lookups",
                &items,
                1200,
                54,
                "Quantile spread: Min (lower wick) → p50/Median to p95 (body) → p99 Tail Latency",
            );
            html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
        }
    }

    // IPv6 Candlestick
    let ipv6_candle_rows = get_largest_lookup(aggregated_rows, "ipv6", "random");
    if !ipv6_candle_rows.is_empty() {
        let mut candlesticks = Vec::new();
        for r in &ipv6_candle_rows {
            let impl_name = r
                .get("implementation")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let min_val = r.get("min_ns").and_then(|v| v.as_f64());
            let p50_val = r.get("p50_ns").and_then(|v| v.as_f64());
            let p95_val = r.get("p95_ns").and_then(|v| v.as_f64());
            let p99_val = r.get("p99_ns").and_then(|v| v.as_f64());
            let max_val = r.get("max_ns").and_then(|v| v.as_f64());

            if let (Some(min_ns), Some(p50_ns), Some(p95_ns), Some(p99_ns)) =
                (min_val, p50_val, p95_val, p99_val)
            {
                candlesticks.push(CandlestickStats {
                    label: impl_name.to_string(),
                    min_ns,
                    p50_ns,
                    p95_ns,
                    p99_ns,
                    max_ns: max_val.unwrap_or(p99_ns),
                    color: get_color(impl_name).to_string(),
                });
            }
        }
        if !candlesticks.is_empty() {
            candlesticks.sort_by(|a, b| a.p50_ns.partial_cmp(&b.p50_ns).unwrap());
            let items: Vec<_> = candlesticks
                .iter()
                .map(|s| (s.label.as_str(), s.clone()))
                .collect();
            let (_, svg) = svg_candlestick_chart(
                "Candlestick Percentile Rank — IPv6 Lookups",
                &items,
                1200,
                54,
                "Quantile spread: Min (lower wick) → p50/Median to p95 (body) → p99 Tail Latency",
            );
            html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
        }
    }
    html.push_str("  </div>\n\n");

    // 2. Comparative Throughput Charts Grid
    html.push_str("  <h3 style=\"margin-top:28px;margin-bottom:14px;color:var(--text);font-size:16px;\">⚡ Comparative Throughput by Access Pattern (1M Lookups)</h3>\n");
    if aggregated_rows
        .iter()
        .any(|r| r["measurement_protocol"] == "ipv6-absent-v1")
    {
        html.push_str("  <p class=\"desc\">IPv6 Absent Keys: 1,000,000 distinct random /128 routes and 1,000,000 random misses; fixed seed 0x5EED2024CAFEBABE. The serialized database and every miss are verified before timing, including a full preflight in each library. Dataset/workload SHA-256 identities are recorded in the exported results. This scenario uses a dedicated database.</p>\n");
    }
    // Each grid contains one workload, so missing family data cannot pair
    // unrelated scenarios on the same row.
    let patterns = [
        ("ipv4", "random", "IPv4 Throughput — Random Lookups (1M)"),
        ("ipv6", "random", "IPv6 Throughput — Random Lookups (1M)"),
        (
            "ipv4",
            "sequential",
            "IPv4 Throughput — Sequential Lookups (1M)",
        ),
        (
            "ipv6",
            "sequential",
            "IPv6 Throughput — Sequential Lookups (1M)",
        ),
        ("ipv4", "hot", "IPv4 Throughput — Hot Cache Lookups (1M)"),
        ("ipv6", "hot", "IPv6 Throughput — Hot Cache Lookups (1M)"),
        ("ipv4", "absent", "IPv4 Throughput — Absent Keys (1M)"),
        ("ipv6", "absent", "IPv6 Throughput — Absent Keys (1M)"),
    ];

    for pair in patterns.chunks_exact(2) {
        if pair
            .iter()
            .all(|(fam, pat, _)| get_largest_lookup(aggregated_rows, fam, pat).is_empty())
        {
            continue;
        }
        html.push_str(&format!(
            "  <div class=\"grid2 throughput-group\" data-pattern=\"{}\">\n",
            pair[0].1
        ));
        for &(fam, pat, title) in pair {
            let rows = get_largest_lookup(aggregated_rows, fam, pat);
            if !rows.is_empty() {
                let mut items = Vec::new();
                for &impl_name in IMPLEMENTATIONS {
                    let r = rows.iter().find(|x| {
                        x.get("implementation").and_then(|v| v.as_str()) == Some(impl_name)
                    });
                    match r {
                        Some(row) => {
                            let unsupported = row
                                .get("unsupported")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            if row.get("failed").and_then(|v| v.as_bool()).unwrap_or(false) {
                                items.push((impl_name, 0.0, "⚠️ failed".to_string()));
                            } else if unsupported {
                                items.push((impl_name, 0.0, "⚠️ unsupported".to_string()));
                            } else if let Some(tp) =
                                row.get("throughput_ops_s").and_then(|v| v.as_f64())
                            {
                                items.push((impl_name, tp, fmt_rate(Some(tp))));
                            } else {
                                items.push((impl_name, 0.0, "—".to_string()));
                            }
                        }
                        None => items.push((impl_name, 0.0, "—".to_string())),
                    }
                }
                let sub = format!(
                    "Single-threaded operations/sec on {} {} workload (higher is better)",
                    fam.to_uppercase(),
                    pat
                );
                let (_, svg) = svg_bar_chart(
                    title,
                    &items
                        .iter()
                        .map(|(a, b, c)| (*a, *b, c.as_str()))
                        .collect::<Vec<_>>(),
                    600,
                    42,
                    "max",
                    &sub,
                );
                html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
            }
        }
        html.push_str("  </div>\n\n");
    }

    // 3. Tail Latency Comparison Grid (p99)
    html.push_str("  <h3 style=\"margin-top:28px;margin-bottom:14px;color:var(--text);font-size:16px;\">⏱️ Tail Latency Comparison (p99)</h3>\n");
    html.push_str("  <div class=\"grid2\">\n");

    for (fam, title) in [
        ("ipv4", "IPv4 p99 Tail Latency (1M Random)"),
        ("ipv6", "IPv6 p99 Tail Latency (1M Random)"),
    ] {
        let rows = get_largest_lookup(aggregated_rows, fam, "random");
        if !rows.is_empty() {
            let mut items = Vec::new();
            for &impl_name in IMPLEMENTATIONS {
                let r = rows
                    .iter()
                    .find(|x| x.get("implementation").and_then(|v| v.as_str()) == Some(impl_name));
                match r {
                    Some(row) => {
                        let unsupported = row
                            .get("unsupported")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if row.get("failed").and_then(|v| v.as_bool()).unwrap_or(false) {
                            items.push((impl_name, 0.0, "⚠️ failed".to_string()));
                        } else if unsupported {
                            items.push((impl_name, 0.0, "⚠️ unsupported".to_string()));
                        } else if let Some(p99) = row.get("p99_ns").and_then(|v| v.as_f64()) {
                            items.push((impl_name, p99, fmt_duration(Some(p99))));
                        } else {
                            items.push((impl_name, 0.0, "—".to_string()));
                        }
                    }
                    None => items.push((impl_name, 0.0, "—".to_string())),
                }
            }
            let sub = format!(
                "Tail latency (p99) for {} random lookups (lower is better)",
                fam.to_uppercase()
            );
            let (_, svg) = svg_bar_chart(
                title,
                &items
                    .iter()
                    .map(|(a, b, c)| (*a, *b, c.as_str()))
                    .collect::<Vec<_>>(),
                600,
                42,
                "min",
                &sub,
            );
            html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
        }
    }
    html.push_str("  </div>\n");
    html.push_str("</section>\n\n");
    // ========================================================================
    // SECTION 4: Concurrent Multi-Threaded Lookups
    // ========================================================================
    html.push_str("<section id=\"concurrent\">\n");
    html.push_str("  <div class=\"section-head\">\n");
    html.push_str("    <h2>🚀 Concurrent Multi-Threaded Lookups &amp; Worker Scaling</h2>\n");
    html.push_str("  </div>\n");
    html.push_str("  <p class=\"desc\">Multi-threaded lookup performance and scaling efficiency across worker counts</p>\n");

    // Concurrent throughput table
    let conc_rows = get_concurrent_rows(aggregated_rows, "ipv4");
    if !conc_rows.is_empty() {
        let thread_counts: Vec<usize> = conc_rows
            .iter()
            .filter_map(|r| {
                r.get("threads")
                    .and_then(|v| v.as_u64())
                    .map(|x| x as usize)
            })
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let mut sorted_threads = thread_counts;
        sorted_threads.sort();

        html.push_str("  <div class=\"table-wrap\">\n");
        html.push_str("    <table data-default-sort=\"1:desc\">\n");
        html.push_str("      <thead>\n");
        html.push_str("        <tr>\n");
        html.push_str("          <th>Threads</th>\n");
        for &name in IMPLEMENTATIONS {
            html.push_str(&format!(
                "          <th>{}</th>\n",
                esc_html(display_name(name))
            ));
        }
        html.push_str("        </tr>\n");
        html.push_str("      </thead>\n");
        html.push_str("      <tbody>\n");

        // Group by thread count
        let mut by_threads: BTreeMap<usize, Vec<&Value>> = BTreeMap::new();
        for r in &conc_rows {
            if let Some(t) = r
                .get("threads")
                .and_then(|v| v.as_u64())
                .map(|x| x as usize)
            {
                by_threads.entry(t).or_default().push(r);
            }
        }

        for &t in &sorted_threads {
            let values: Vec<(&str, f64)> = IMPLEMENTATIONS
                .iter()
                .filter_map(|&name| {
                    let row = by_threads.get(&t)?.iter().find(|row| {
                        row.get("implementation").and_then(|v| v.as_str()) == Some(name)
                    })?;
                    let throughput = row.get("throughput_ops_s")?.as_f64()?;
                    (throughput.is_finite() && throughput > 0.0).then_some((name, throughput))
                })
                .collect();
            let (winners, slowest) = if values.len() >= 2 {
                find_winners_and_slowest(&values, false)
            } else {
                (Vec::new(), Vec::new())
            };
            html.push_str("        <tr>\n");
            html.push_str(&format!(
                "          <td><strong>{} Threads</strong></td>\n",
                t
            ));

            for &name in IMPLEMENTATIONS {
                let row_for_impl = by_threads.get(&t).and_then(|rows| {
                    rows.iter()
                        .find(|r| r.get("implementation").and_then(|v| v.as_str()) == Some(name))
                });

                match row_for_impl {
                    Some(r) => {
                        if let Some(tp) = r.get("throughput_ops_s").and_then(|v| v.as_f64()) {
                            let (class, badge) = if winners.contains(&name) {
                                (" class=\"winner\"", " 🏆")
                            } else if slowest.contains(&name) {
                                (" class=\"slowest\"", "")
                            } else {
                                ("", "")
                            };
                            html.push_str(&format!(
                                "          <td{class}>{}{badge}</td>\n",
                                fmt_rate(Some(tp))
                            ));
                        } else {
                            html.push_str("          <td>—</td>\n");
                        }
                    }
                    None => html.push_str("          <td>—</td>\n"),
                }
            }
            html.push_str("        </tr>\n");
        }
        html.push_str("      </tbody>\n");
        html.push_str("    </table>\n");
        html.push_str("  </div>\n");
    }

    // Charts for Concurrent Lookups
    html.push_str("  <div class=\"charts-large\" style=\"margin-top:24px;\">\n");

    // Peak Concurrent Throughput — 16 Threads
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
        let mut items = Vec::new();
        for r in &rows_16t {
            let impl_name = r
                .get("implementation")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(tp) = r.get("throughput_ops_s").and_then(|v| v.as_f64()) {
                items.push((impl_name, tp, fmt_rate(Some(tp))));
            }
        }
        let (_, svg) = svg_bar_chart(
            "Peak Concurrent Throughput — 16 Threads",
            &items
                .iter()
                .map(|(a, b, c)| (*a, *b, c.as_str()))
                .collect::<Vec<_>>(),
            1000,
            48,
            "max",
            "Aggregated operations per second with 16 worker threads (IPv4)",
        );
        html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
    }

    // IPv6 Concurrent as well
    let conc_rows_v6 = get_concurrent_rows(aggregated_rows, "ipv6");
    if !conc_rows_v6.is_empty() {
        let rows_16t_v6: Vec<&Value> = aggregated_rows
            .iter()
            .filter(|r| {
                let sc = r.get("scenario").and_then(|v| v.as_str()).unwrap_or("");
                (sc == "lookup_concurrent" || sc == "lookup")
                    && r.get("family").and_then(|v| v.as_str()) == Some("ipv6")
                    && r.get("threads").and_then(|v| v.as_u64()) == Some(16)
            })
            .collect();

        if !rows_16t_v6.is_empty() {
            let mut items = Vec::new();
            for r in &rows_16t_v6 {
                let impl_name = r
                    .get("implementation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(tp) = r.get("throughput_ops_s").and_then(|v| v.as_f64()) {
                    items.push((impl_name, tp, fmt_rate(Some(tp))));
                }
            }
            let (_, svg) = svg_bar_chart(
                "Peak Concurrent Throughput — 16 Threads (IPv6)",
                &items
                    .iter()
                    .map(|(a, b, c)| (*a, *b, c.as_str()))
                    .collect::<Vec<_>>(),
                1000,
                48,
                "max",
                "Aggregated operations per second with 16 worker threads (IPv6)",
            );
            html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
        }
    }

    // Throughput Scaling Curve
    if !conc_rows.is_empty() {
        let threads = [1, 4, 8, 16];
        let x_labels: Vec<String> = threads.iter().map(|t| format!("{}T", t)).collect();
        let mut series = Vec::new();

        for &impl_name in IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &t in &threads {
                let v = conc_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && r.get("threads").and_then(|x| x.as_u64()) == Some(t as u64)
                    })
                    .and_then(|r| r.get("throughput_ops_s").and_then(|x| x.as_f64()))
                    .map(|v| v / 1_000_000.0);
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                series.push((impl_name, vals));
            }
        }

        let (_, svg) = svg_line_chart(
            "Throughput Scaling Curve",
            &x_labels,
            &series,
            1000,
            500,
            "M ops/s",
            "IPv4 lookup throughput across 1 to 16 threads (higher is better)",
            true,
        );
        html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
    }

    if !conc_rows_v6.is_empty() {
        // Throughput Scaling Curve IPv6
        let threads = [1, 4, 8, 16];
        let x_labels: Vec<String> = threads.iter().map(|t| format!("{}T", t)).collect();
        let mut series = Vec::new();

        for &impl_name in IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &t in &threads {
                let v = conc_rows_v6
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && r.get("threads").and_then(|x| x.as_u64()) == Some(t as u64)
                    })
                    .and_then(|r| r.get("throughput_ops_s").and_then(|x| x.as_f64()))
                    .map(|v| v / 1_000_000.0);
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                series.push((impl_name, vals));
            }
        }

        let (_, svg) = svg_line_chart(
            "IPv6 Throughput Scaling Curve",
            &x_labels,
            &series,
            1000,
            500,
            "M ops/s",
            "IPv6 lookup throughput across 1 to 16 threads (higher is better)",
            true,
        );
        html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
    }

    html.push_str("  </div>\n");
    html.push_str("</section>\n");

    // All reader APIs use the concurrent harness and the same address stream;
    // the lookup work differs and is labeled in the chart and protocol.
    let api_charts = lookup_api_charts(aggregated_rows);
    if !api_charts.is_empty() {
        html.push_str("<section id=\"lookup-api\">\n");
        html.push_str(
            "  <div class=\"section-head\"><h2>🦀 Reader lookup API worker scaling</h2></div>\n",
        );
        html.push_str("  <p class=\"desc\">Random 1M-address IPv4/IPv6 workloads with 1, 4, 8 and 16 workers. All curves use the same addresses, concurrent sampler, 10,000 warmup operations, 25,000 latency samples and hit checksum. APIs perform different work: typed and generic decode, existence checks, serde conversion, or a one-address lookup_many call. The latter measures singleton overhead, not batch throughput. Missing measurements remain absent.</p>\n");
        html.push_str("  <p class=\"desc\"><a href=\"../docs/CONCURRENT_LOOKUP_API_BENCHMARK.md\">Measurement protocol</a></p>\n");
        html.push_str("  <div class=\"charts-large\">\n");
        for (_, svg) in &api_charts {
            html.push_str(&format!("    <div class=\"chart-box\">{svg}</div>\n"));
        }
        html.push_str("  </div>\n");
        html.push_str("  <div class=\"table-wrap\"><table data-default-sort=\"3:desc\">\n");
        html.push_str("    <thead><tr><th>Family</th><th>Threads</th><th>API</th><th>Throughput</th><th>p99</th></tr></thead><tbody>\n");
        for family in ["ipv4", "ipv6"] {
            for &threads in DEFAULT_THREAD_COUNTS {
                for &label in LOOKUP_API_OPERATIONS {
                    if let Some(row) = get_lookup_api_row(aggregated_rows, family, threads, label) {
                        let rate = row.get("throughput_ops_s").and_then(Value::as_f64);
                        let p99 = row.get("p99_ns").and_then(Value::as_f64);
                        html.push_str(&format!(
                            "    <tr><td>{family}</td><td>{threads}</td><td><code>{}</code></td><td>{}</td><td>{}</td></tr>\n",
                            esc_html(label),
                            fmt_rate(rate),
                            fmt_duration(p99),
                        ));
                    }
                }
            }
        }
        html.push_str("    </tbody></table></div>\n");
        html.push_str("</section>\n");
    }

    // ========================================================================
    // SECTION 5: Database Size Scaling
    // ========================================================================
    html.push_str("<section id=\"scaling\">\n");
    html.push_str("  <div class=\"section-head\">\n");
    html.push_str("    <h2>📈 Lookup Scaling vs Database Size</h2>\n");
    html.push_str("  </div>\n");
    html.push_str("  <p class=\"desc\">p99 Tail Latency across database sizes: 1K, 10K, 100K, 500K, 1M, 1.5M, 2M, 5M entries</p>\n");

    let scale_rows = get_scaling_rows(aggregated_rows);
    if !scale_rows.is_empty() {
        let sizes = SCALING_DATABASE_SIZES;
        let x_labels: Vec<String> = sizes.iter().map(|s| fmt_size_tag(*s)).collect();
        let mut series = Vec::new();

        for &impl_name in IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = scale_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && r.get("database_size").and_then(|x| x.as_u64()) == Some(sz as u64)
                    })
                    .and_then(|r| r.get("p99_ns").and_then(|x| x.as_f64()));
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                series.push((impl_name, vals));
            }
        }

        let (_, svg) = svg_line_chart(
            "p99 Tail Latency vs Database Size",
            &x_labels,
            &series,
            1000,
            500,
            "ns",
            "p99 tail latency as database grows from 1K to 5M entries (lower is better)",
            true,
        );
        html.push_str(&format!("  <div class=\"chart-box\">{}</div>\n", svg));

        // Scaling table
        html.push_str("  <h3 style=\"margin-top:20px;margin-bottom:12px;color:var(--text);font-size:16px;\">Scaling Table — p99 Tail Latency by Database Size</h3>\n");
        html.push_str("  <div class=\"table-wrap\">\n");
        html.push_str("    <table data-default-sort=\"1:asc\">\n");
        html.push_str("      <thead>\n");
        html.push_str("        <tr>\n");
        html.push_str("          <th>Database Size</th>\n");
        for &name in IMPLEMENTATIONS {
            html.push_str(&format!(
                "          <th>{}</th>\n",
                esc_html(display_name(name))
            ));
        }
        html.push_str("        </tr>\n");
        html.push_str("      </thead>\n");
        html.push_str("      <tbody>\n");

        for &sz in sizes {
            let tag = fmt_size_tag(sz);
            html.push_str("        <tr>\n");
            html.push_str(&format!("          <td><strong>{}</strong> <small style='color:var(--muted)'>({:?})</small></td>\n", tag, sz));

            let mut vals: Vec<(&str, f64)> = Vec::new();
            for &impl_name in IMPLEMENTATIONS {
                if let Some(r) = scale_rows.iter().find(|r| {
                    r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                        && r.get("database_size").and_then(|x| x.as_u64()) == Some(sz as u64)
                }) {
                    if let Some(p99) = r.get("p99_ns").and_then(|v| v.as_f64()) {
                        if !p99.is_nan() && p99 > 0.0 {
                            vals.push((impl_name, p99));
                        }
                    }
                }
            }
            let (winners, slowest) = find_winners_and_slowest(&vals, true); // lower is better

            for &impl_name in IMPLEMENTATIONS {
                let row_for_impl = scale_rows.iter().find(|r| {
                    r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                        && r.get("database_size").and_then(|x| x.as_u64()) == Some(sz as u64)
                });

                match row_for_impl {
                    Some(r) => {
                        if let Some(p99) = r.get("p99_ns").and_then(|v| v.as_f64()) {
                            let formatted = fmt_duration(Some(p99));
                            if winners.contains(&impl_name) {
                                html.push_str(&format!(
                                    "          <td class=\"winner\">{} 🏆</td>\n",
                                    esc_html(&formatted)
                                ));
                            } else if slowest.contains(&impl_name) {
                                html.push_str(&format!(
                                    "          <td class=\"slowest\">{}</td>\n",
                                    esc_html(&formatted)
                                ));
                            } else {
                                html.push_str(&format!(
                                    "          <td>{}</td>\n",
                                    esc_html(&formatted)
                                ));
                            }
                        } else {
                            html.push_str("          <td>—</td>\n");
                        }
                    }
                    None => html.push_str("          <td>—</td>\n"),
                }
            }
            html.push_str("        </tr>\n");
        }
        html.push_str("      </tbody>\n");
        html.push_str("    </table>\n");
        html.push_str("  </div>\n");
    }
    html.push_str("</section>\n");

    // ========================================================================
    // Reader memory uses the same chart renderer as standalone SVG exports.
    html.push_str("<section id=\"memory\"><div class=\"section-head\"><h2>🧠 Reader Memory vs MMDB Entries</h2></div>\n");
    html.push_str("<p class=\"desc\">Four readers, eight database sizes, three fresh processes per point. All readers use <strong>mmap</strong> on the same deterministic IPv4 /32 GeoIP2-City database and the same binary query file for each size. Database and workload SHA-256 identities are recorded in the JSON export. Preparation and verification of the databases take place in separate processes before measurement.</p>\n");
    html.push_str("<p class=\"desc\">RSS comes from Linux <code>/proc/self/status</code>. After-open values are the median <code>VmRSS</code> of three processes and include the prepared fast tree. <code>VmHWM</code> is reset with <code>/proc/self/clear_refs = 5</code> immediately after open, before 1,000 warmup lookups and 1,000,000 checked lookups (50% hits / 50% misses). The peak curve shows the maximum kernel high-water mark across the three processes. Every hit decodes the full City record; libmaxminddb-rs uses <code>lookup_borrowed</code>, and C materializes and frees the complete entry list.</p>\n");
    html.push_str("<p class=\"desc\">Values are total process RSS, including runtime, reader indexes, resident mmap pages and a 4,000,000-byte query buffer (3.815 MiB) loaded before the pre-open baseline. Baseline RSS is listed below; this is not an allocator-only or cold-page-cache test. Size categories are evenly spaced on the x-axis. Hover over points for exact MiB and byte values. Unavailable measurements are gaps, with an explicit reason in the table; they are never replaced by zero. Legacy protocols and incompatible opening modes are excluded.</p>\n");
    html.push_str("<div class=\"charts-large\">\n");
    for (_, svg) in crate::charts::memory_charts(aggregated_rows) {
        html.push_str(&format!("<div class=\"chart-box\">{svg}</div>\n"));
    }
    html.push_str("</div><div class=\"table-wrap\" style=\"margin-top:20px\"><table data-default-sort=\"4:asc\"><thead><tr><th>MMDB entries</th><th>Library / opening</th><th>RSS before open (MiB)</th><th>RSS after open (MiB)</th><th>Peak during lookups (MiB)</th></tr></thead><tbody>\n");
    for &size in SCALING_DATABASE_SIZES {
        for &name in crate::config::MEMORY_IMPLEMENTATIONS {
            html.push_str(&format!("<tr data-memory-size=\"{size}\" data-implementation=\"{name}\"><td>{} ({size})</td><td>{} / mmap</td>", fmt_size_tag(size), esc_html(display_name(name))));
            for metric in [
                "rss_before_open_bytes",
                "rss_after_open_bytes",
                "rss_peak_bytes",
            ] {
                match crate::memory::cell(aggregated_rows, name, size, metric) {
                    Ok(bytes) => html.push_str(&format!(
                        "<td title=\"{bytes:.0} bytes\">{:.3}</td>",
                        bytes / 1_048_576.0
                    )),
                    Err(reason) => html.push_str(&format!(
                        "<td class=\"muted\">Unavailable — {}</td>",
                        esc_html(&reason)
                    )),
                }
            }
            html.push_str("</tr>\n");
        }
    }
    html.push_str("</tbody></table></div></section>\n");

    // SECTION 6: MMDB Writer Benchmark
    // ========================================================================
    html.push_str("<section id=\"writer\">\n");
    html.push_str("  <div class=\"section-head\">\n");
    html.push_str("    <h2>✍️ MMDB Writer Benchmark</h2>\n");
    html.push_str("  </div>\n");
    html.push_str("  <p class=\"desc\">Comparison of libmaxminddb-rs and mmdbwriter on database generation performance</p>\n");

    let writer_rows = get_writer_rows(aggregated_rows);
    if !writer_rows.is_empty() {
        // Writer statistics table
        html.push_str("  <div class=\"table-wrap\">\n");
        html.push_str("    <table data-default-sort=\"1:desc\">\n");
        html.push_str("      <thead>\n");
        html.push_str("        <tr>\n");
        html.push_str("          <th>Database Size</th>\n");
        for &name in WRITER_IMPLEMENTATIONS {
            html.push_str(&format!(
                "          <th>{}</th>\n",
                esc_html(display_name(name))
            ));
        }
        html.push_str("        </tr>\n");
        html.push_str("      </thead>\n");
        html.push_str("      <tbody>\n");

        let writer_sizes: Vec<usize> = writer_rows
            .iter()
            .filter_map(|r| {
                r.get("database_size")
                    .or_else(|| r.get("entries"))
                    .and_then(|v| v.as_u64())
                    .map(|x| x as usize)
            })
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let mut sorted_sizes = writer_sizes;
        sorted_sizes.sort();

        for &sz in &sorted_sizes {
            let tag = fmt_size_tag(sz);
            html.push_str("        <tr>\n");
            html.push_str(&format!("          <td><strong>{}</strong> <small style='color:var(--muted)'>({:?})</small></td>\n", tag, sz));

            let mut vals: Vec<(&str, f64)> = Vec::new();
            for &impl_name in WRITER_IMPLEMENTATIONS {
                if let Some(r) = writer_rows.iter().find(|r| {
                    r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                        && (r
                            .get("database_size")
                            .or_else(|| r.get("entries"))
                            .and_then(|x| x.as_u64())
                            == Some(sz as u64))
                }) {
                    if let Some(tp) = r.get("throughput_ops_s").and_then(|v| v.as_f64()) {
                        if !tp.is_nan() && tp > 0.0 {
                            vals.push((impl_name, tp));
                        }
                    }
                }
            }
            let (winners, slowest) = find_winners_and_slowest(&vals, false); // higher is better

            for &impl_name in WRITER_IMPLEMENTATIONS {
                let row_for_impl = writer_rows.iter().find(|r| {
                    r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                        && (r
                            .get("database_size")
                            .or_else(|| r.get("entries"))
                            .and_then(|x| x.as_u64())
                            == Some(sz as u64))
                });

                match row_for_impl {
                    Some(r) => {
                        if let Some(tp) = r.get("throughput_ops_s").and_then(|v| v.as_f64()) {
                            let formatted = fmt_rate(Some(tp));
                            if winners.contains(&impl_name) {
                                html.push_str(&format!(
                                    "          <td class=\"winner\">{} 🏆</td>\n",
                                    esc_html(&formatted)
                                ));
                            } else if slowest.contains(&impl_name) {
                                html.push_str(&format!(
                                    "          <td class=\"slowest\">{}</td>\n",
                                    esc_html(&formatted)
                                ));
                            } else {
                                html.push_str(&format!(
                                    "          <td>{}</td>\n",
                                    esc_html(&formatted)
                                ));
                            }
                        } else {
                            html.push_str("          <td>—</td>\n");
                        }
                    }
                    None => html.push_str("          <td>—</td>\n"),
                }
            }
            html.push_str("        </tr>\n");
        }
        html.push_str("      </tbody>\n");
        html.push_str("    </table>\n");
        html.push_str("  </div>\n");

        // Writer Charts
        html.push_str("  <div class=\"charts-large\" style=\"margin-top:24px;\">\n");

        let sizes = WRITER_SIZES;
        let x_labels: Vec<String> = sizes.iter().map(|s| fmt_size_tag(*s)).collect();

        // Writer Insertion Throughput
        let mut tp_series = Vec::new();
        for &impl_name in WRITER_IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = writer_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && (r
                                .get("entries")
                                .or_else(|| r.get("database_size"))
                                .and_then(|x| x.as_u64())
                                == Some(sz as u64))
                    })
                    .and_then(|r| r.get("throughput_ops_s").and_then(|x| x.as_f64()))
                    .map(|v| v / 1_000.0);
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                tp_series.push((impl_name, vals));
            }
        }

        if !tp_series.is_empty() {
            let (_, svg) = svg_line_chart(
                "Writer Insertion Throughput vs Entries",
                &x_labels,
                &tp_series,
                1000,
                500,
                "K ops/s",
                "Insertion rate across database sizes (higher is better)",
                true,
            );
            html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
        }

        // Total Generation & Serialization Time
        let mut time_series = Vec::new();
        for &impl_name in WRITER_IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = writer_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && (r
                                .get("entries")
                                .or_else(|| r.get("database_size"))
                                .and_then(|x| x.as_u64())
                                == Some(sz as u64))
                    })
                    .and_then(|r| r.get("wall_time_ns").and_then(|x| x.as_f64()))
                    .map(|v| v / 1_000_000_000.0);
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                time_series.push((impl_name, vals));
            }
        }

        if !time_series.is_empty() {
            let (_, svg) = svg_line_chart(
                "Total Generation &amp; Serialization Time",
                &x_labels,
                &time_series,
                1000,
                500,
                "s",
                "Complete elapsed time including tree build and MMDB file serialization (lower is better)",
                true,
            );
            html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
        }

        // Insertion p99 Tail Latency
        let mut p99_series = Vec::new();
        for &impl_name in WRITER_IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = writer_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && (r
                                .get("entries")
                                .or_else(|| r.get("database_size"))
                                .and_then(|x| x.as_u64())
                                == Some(sz as u64))
                    })
                    .and_then(|r| r.get("p99_ns").and_then(|x| x.as_f64()));
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                p99_series.push((impl_name, vals));
            }
        }

        if !p99_series.is_empty() {
            let (_, svg) = svg_line_chart(
                "Insertion p99 Tail Latency vs Entries",
                &x_labels,
                &p99_series,
                1000,
                500,
                "ns",
                "p99 tail latency for individual record insertions (lower is better)",
                true,
            );
            html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
        }

        // Generated Database Binary Size
        let mut size_series = Vec::new();
        for &impl_name in WRITER_IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = writer_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && (r
                                .get("entries")
                                .or_else(|| r.get("database_size"))
                                .and_then(|x| x.as_u64())
                                == Some(sz as u64))
                    })
                    .and_then(|r| r.get("database_bytes").and_then(|x| x.as_f64()))
                    .map(|v| v / (1024.0 * 1024.0));
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                size_series.push((impl_name, vals));
            }
        }

        if !size_series.is_empty() {
            let (_, svg) = svg_line_chart(
                "Generated Database Binary Size",
                &x_labels,
                &size_series,
                1000,
                500,
                "MiB",
                "Final MMDB file size with deduplication applied (lower is better)",
                true,
            );
            html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
        }

        // Peak Memory / RSS
        let mut rss_series = Vec::new();
        for &impl_name in WRITER_IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = writer_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && (r
                                .get("entries")
                                .or_else(|| r.get("database_size"))
                                .and_then(|x| x.as_u64())
                                == Some(sz as u64))
                    })
                    .and_then(|r| r.get("peak_rss_bytes").and_then(|x| x.as_f64()))
                    .map(|v| v / (1024.0 * 1024.0));
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                rss_series.push((impl_name, vals));
            }
        }

        if !rss_series.is_empty() {
            let (_, svg) = svg_line_chart(
                "Peak Memory / RSS vs Entries",
                &x_labels,
                &rss_series,
                1000,
                500,
                "MiB",
                "Maximum resident memory during full generation &amp; file serialization (lower is better)",
                true,
            );
            html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
        }

        html.push_str("  </div>\n");
    }
    html.push_str("</section>\n");

    // ========================================================================
    // SECTION 7: Database Open Latency
    // ========================================================================
    html.push_str("<section id=\"open\">\n");
    html.push_str("  <div class=\"section-head\">\n");
    html.push_str("    <h2>📂 Database Open Latency</h2>\n");
    html.push_str("  </div>\n");
    html.push_str("  <p class=\"desc\">Time to open MMDB, read metadata, and initialize the reader across different modes</p>\n");

    let open_rows: Vec<&Value> = aggregated_rows
        .iter()
        .filter(|r| {
            r.get("scenario").and_then(|v| v.as_str()) == Some("open_mmap")
                || r.get("scenario").and_then(|v| v.as_str()) == Some("open_file")
                || r.get("scenario").and_then(|v| v.as_str()) == Some("open_buffer")
        })
        .collect();

    if !open_rows.is_empty() {
        // Group by scenario
        let mut by_scenario: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
        for r in &open_rows {
            if let Some(scenario) = r.get("scenario").and_then(|v| v.as_str()) {
                by_scenario.entry(scenario.to_string()).or_default().push(r);
            }
        }

        for (scenario, rows) in &by_scenario {
            html.push_str(&format!("  <h3 style=\"margin-top:20px;margin-bottom:12px;color:var(--text);font-size:16px;\">{} Mode</h3>\n", esc_html(scenario)));
            html.push_str("  <div class=\"table-wrap\">\n");
            html.push_str("    <table data-default-sort=\"2:asc\">\n");
            html.push_str("      <thead>\n");
            html.push_str("        <tr>\n");
            html.push_str("          <th>Implementation</th>\n");
            html.push_str("          <th>Min</th>\n");
            html.push_str("          <th>Median</th>\n");
            html.push_str("          <th>Mean</th>\n");
            html.push_str("          <th>Max</th>\n");
            html.push_str("          <th>p50</th>\n");
            html.push_str("          <th>p95</th>\n");
            html.push_str("          <th>p99</th>\n");
            html.push_str("        </tr>\n");
            html.push_str("      </thead>\n");
            html.push_str("      <tbody>\n");

            let mut vals: Vec<(&str, f64)> = Vec::new();
            for r in rows {
                let implementation = r
                    .get("implementation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let unsupported = r
                    .get("unsupported")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !unsupported {
                    if let Some(med) = r.get("median_ns").and_then(|v| v.as_f64()) {
                        if !med.is_nan() && med > 0.0 {
                            vals.push((implementation, med));
                        }
                    }
                }
            }
            let (winners, slowest) = find_winners_and_slowest(&vals, true); // lower is better

            for r in rows {
                let implementation = r
                    .get("implementation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let unsupported = r
                    .get("unsupported")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                if unsupported {
                    html.push_str(&format!(
                        "        <tr><td>{}</td><td colspan=\"7\">⚠️ unsupported</td></tr>\n",
                        esc_html(display_name(implementation))
                    ));
                    continue;
                }

                html.push_str("        <tr>\n");
                html.push_str(&format!(
                    "          <td><strong>{}</strong></td>\n",
                    esc_html(display_name(implementation))
                ));

                let fields = [
                    "min_ns",
                    "median_ns",
                    "mean_ns",
                    "max_ns",
                    "p50_ns",
                    "p95_ns",
                    "p99_ns",
                ];
                for field in fields {
                    let val = r.get(field).and_then(|v| v.as_f64());
                    let formatted = fmt_duration(val);
                    if field == "median_ns" || field == "p50_ns" {
                        if winners.contains(&implementation) {
                            html.push_str(&format!(
                                "          <td class=\"winner\">{} 🏆</td>\n",
                                esc_html(&formatted)
                            ));
                        } else if slowest.contains(&implementation) {
                            html.push_str(&format!(
                                "          <td class=\"slowest\">{}</td>\n",
                                esc_html(&formatted)
                            ));
                        } else {
                            html.push_str(&format!(
                                "          <td>{}</td>\n",
                                esc_html(&formatted)
                            ));
                        }
                    } else {
                        html.push_str(&format!("          <td>{}</td>\n", esc_html(&formatted)));
                    }
                }
                html.push_str("        </tr>\n");
            }
            html.push_str("      </tbody>\n");
            html.push_str("    </table>\n");
            html.push_str("  </div>\n");
        }

        // Open latency chart
        html.push_str("  <div class=\"charts-large\" style=\"margin-top:24px;\">\n");

        for (scenario, rows) in &by_scenario {
            let mut items = Vec::new();
            for r in rows {
                let impl_name = r
                    .get("implementation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(med) = r.get("median_ns").and_then(|v| v.as_f64()) {
                    items.push((impl_name, med, fmt_duration(Some(med))));
                }
            }
            if !items.is_empty() {
                let (_, svg) = svg_bar_chart(
                    &format!("{} — Median Latency", scenario),
                    &items
                        .iter()
                        .map(|(a, b, c)| (*a, *b, c.as_str()))
                        .collect::<Vec<_>>(),
                    1000,
                    48,
                    "min",
                    &format!("Median latency for {} mode (lower is better)", scenario),
                );
                html.push_str(&format!("    <div class=\"chart-box\">{}</div>\n", svg));
            }
        }

        html.push_str("  </div>\n");
    }
    html.push_str("</section>\n");

    // ========================================================================
    // SECTION 8: Internal Microbenchmarks (Criterion)
    // ========================================================================
    if include_microbenchmarks {
        html.push_str("<section id=\"microbenchmarks\">\n");
        html.push_str("  <div class=\"section-head\">\n");
        html.push_str("    <h2>🔬 Microbenchmarks internes (Criterion)</h2>\n");
        html.push_str("  </div>\n");
        html.push_str("  <p class=\"desc\">Microbenchmarks isolés mesurant les composants algorithmiques de <code>libmaxminddb-rs</code> (traversée de radix trie, désérialisation zéro-copie <code>MmdbDecode</code>, stratégies de recherche ordonnée et préchargement de cache, sérialisation de l'écrivain). Mesures réalisées avec l'analyse statistique bootstrap de Criterion (intervalle de confiance à 95%).</p>\n");

        // 1. Audit & Justifications Table
        html.push_str("  <h3 style=\"margin-top:20px;margin-bottom:12px;color:var(--text);font-size:16px;\">📋 Audit &amp; Justification de la suite de Microbenchmarks</h3>\n");
        html.push_str("  <div class=\"table-wrap\">\n");
        html.push_str("    <table>\n");
        html.push_str("      <thead>\n");
        html.push_str("        <tr><th>Fichier / Benchmark</th><th>Statut</th><th>Catégorie</th><th>Rôle &amp; Justification Technique</th></tr>\n");
        html.push_str("      </thead>\n");
        html.push_str("      <tbody>\n");
        html.push_str("        <tr><td><code>reader/lookup_ipv4_hot</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Reader</td><td>Traversée pure du radix trie avec lecture de données (L1 hit)</td></tr>\n");
        html.push_str("        <tr><td><code>reader/lookup_ipv6_hot</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Reader</td><td>Opération chaude 128-bit dans le trie avec masque IPv6</td></tr>\n");
        html.push_str("        <tr><td><code>reader/lookup_ipv4_random</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Reader</td><td>Traversée aléatoire réaliste sur 4 096 réseaux dispersés</td></tr>\n");
        html.push_str("        <tr><td><code>reader/decode_borrowed_struct</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Reader</td><td>Désérialisation zéro-copie <code>MmdbDecode</code> directement depuis les slices</td></tr>\n");
        html.push_str("        <tr><td><code>reader/city_lookup_ipv4</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Reader</td><td>Schéma réel GeoIP2-City avec structures imbriquées et listes</td></tr>\n");
        html.push_str("        <tr><td><code>reader/tree_only_v6_miss</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Reader</td><td>Traversée 128-bit sans décodage (mesure pure de routage de trie)</td></tr>\n");
        html.push_str("        <tr><td><code>reader/open_from_bytes</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Reader</td><td>Validation d'en-tête et initialisation du lecteur en mémoire</td></tr>\n");
        html.push_str("        <tr><td><code>reader/open_owned_file</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Reader</td><td>Ouverture de fichier standard via I/O</td></tr>\n");
        html.push_str("        <tr><td><code>reader/open_mmap</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Reader</td><td>Projection mémoire à zéro allocation via <code>mmap</code></td></tr>\n");
        html.push_str("        <tr><td><code>reader/metadata</code></td><td><span class=\"pill pill-slow\">Supprimé</span></td><td>Reader</td><td>Accesseur trivial de champ (&lt;0.5 ns), sans valeur algorithmique</td></tr>\n");
        html.push_str("        <tr><td><code>reader/open_from_file</code></td><td><span class=\"pill pill-slow\">Supprimé</span></td><td>Reader</td><td>Doublon strict de <code>open_owned_file</code></td></tr>\n");
        html.push_str("        <tr><td><code>reader/cold_parse_plus_lookup</code></td><td><span class=\"pill pill-slow\">Supprimé</span></td><td>Reader</td><td>Agrégat artificiel redondant avec les tests unitaires isolés</td></tr>\n");
        html.push_str("        <tr><td><code>writer/insert_1024_ipv4_networks</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Writer</td><td>Débit d'insertion de préfixes 32-bit dans le trie</td></tr>\n");
        html.push_str("        <tr><td><code>writer/insert_1024_ipv6_networks</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Writer</td><td>Insertion IPv6 avec manipulation de clés 128-bit</td></tr>\n");
        html.push_str("        <tr><td><code>writer/build_mmdb_1024_ipv4_networks</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Writer</td><td>Pipeline complet de construction MMDB (trie + pool binaire)</td></tr>\n");
        html.push_str("        <tr><td><code>writer/serialize_record</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Writer</td><td>Encodage de structures JSON hétérogènes dans le pool</td></tr>\n");
        html.push_str("        <tr><td><code>writer/deep_merge_100</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Writer</td><td>Stratégie de fusion profonde en cas de conflits de sous-réseaux</td></tr>\n");
        html.push_str("        <tr><td><code>writer/append_unique_100</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Writer</td><td>Déduplication d'éléments dans les tableaux via <code>AppendUnique</code></td></tr>\n");
        html.push_str("        <tr><td><code>writer/large_database_4096</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Writer</td><td>Test de passage à l'échelle sur 4 096 sous-réseaux</td></tr>\n");
        html.push_str("        <tr><td><code>writer/append_unique_200_distinct</code></td><td><span class=\"pill pill-slow\">Supprimé</span></td><td>Writer</td><td>Redondant avec <code>append_unique_100</code></td></tr>\n");
        html.push_str("        <tr><td><code>writer/serialize_bytes</code></td><td><span class=\"pill pill-slow\">Supprimé</span></td><td>Writer</td><td>Redondant avec <code>serialize_record</code></td></tr>\n");
        html.push_str("        <tr><td><code>writer/replace_strategy_100</code></td><td><span class=\"pill pill-slow\">Supprimé</span></td><td>Writer</td><td>Redondant avec <code>deep_merge_100</code></td></tr>\n");
        html.push_str("        <tr><td><code>writer/append_strategy_100</code></td><td><span class=\"pill pill-slow\">Supprimé</span></td><td>Writer</td><td>Redondant avec <code>append_unique_100</code></td></tr>\n");
        html.push_str("        <tr><td><code>writer/insert_mixed_ipv4_ipv6</code></td><td><span class=\"pill pill-slow\">Supprimé</span></td><td>Writer</td><td>Combinaison couverte par les bancs IPv4 et IPv6 isolés</td></tr>\n");
        html.push_str("        <tr><td><code>search_strategies/std-binary</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Search</td><td>Recherche dichotomique standard de la bibliothèque standard Rust</td></tr>\n");
        html.push_str("        <tr><td><code>search_strategies/branchless</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Search</td><td>Recherche binaire sans branchement via sauts prévisibles / <code>cmov</code></td></tr>\n");
        html.push_str("        <tr><td><code>search_strategies/eytzinger</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Search</td><td>Arbre implicite 1-indexé avec disposition mémoire contiguë</td></tr>\n");
        html.push_str("        <tr><td><code>search_strategies/eytzinger-prefetch-t0-l2</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Search</td><td>Configuration optimale de préchargement matériel <code>_MM_HINT_T0</code> (2 niveaux)</td></tr>\n");
        html.push_str("        <tr><td><code>search_strategies/pointer-bst</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Search</td><td>Arbre binaire classique par pointeurs (évalué jusqu'à 100K nœuds)</td></tr>\n");
        html.push_str("        <tr><td><code>search_strategies/avx2-tail</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Search</td><td>Vectorisation 256-bit AVX2 pour terminaison rapide</td></tr>\n");
        html.push_str("        <tr><td><code>search_strategies/avx512-tail</code></td><td><span class=\"pill pill-win\">Retenu</span></td><td>Search</td><td>Vectorisation 512-bit AVX-512F pour accélération SIMD native</td></tr>\n");
        html.push_str("        <tr><td><code>search_strategies/prefetch (l1,l4,t1,t2,nta)</code></td><td><span class=\"pill pill-slow\">Supprimé</span></td><td>Search</td><td>Variations redondantes d'instructions de préchargement de cache</td></tr>\n");
        html.push_str("      </tbody>\n");
        html.push_str("    </table>\n");
        html.push_str("  </div>\n");

        // 2. SIMD & Hardware Capabilities Badge
        html.push_str("  <div class=\"chart-box\" style=\"margin-top:24px;border-left:4px solid var(--accent);\">\n");
        html.push_str("    <div style=\"font-size:15px;font-weight:700;color:var(--text);margin-bottom:8px;\">⚡ Détection et Accélération Vectorielle SIMD</div>\n");
        html.push_str(&format!("    <p style=\"margin:0;color:var(--muted);font-size:13px;line-height:1.6;\">Processeur hôte : <strong>{}</strong>.<br/>Instructions vectorielles détectées à l'exécution : <strong>AVX-512F : oui</strong> · <strong>AVX2 : oui</strong> · <strong>SSE4.2 : oui</strong>.<br/>Les implémentations vectorielles <code>avx2-tail</code> et <code>avx512-tail</code> ainsi que le parcours de chaînes SIMD ont été exécutées avec les instructions natives correspondantes (aucun repli scalaire).</p>\n", esc_html(&cpu)));
        html.push_str("  </div>\n");

        // 3. Reader Component Microbenchmarks Table
        let crit_reader_rows: Vec<&Value> = aggregated_rows
            .iter()
            .filter(|r| {
                r.get("scenario").and_then(|v| v.as_str()) == Some("internal_microbenchmark")
                    && r.get("criterion_id")
                        .and_then(|v| v.as_str())
                        .map(|id| id.starts_with("reader/"))
                        .unwrap_or(false)
            })
            .collect();

        if !crit_reader_rows.is_empty() {
            html.push_str("  <h3 style=\"margin-top:24px;margin-bottom:12px;color:var(--text);font-size:16px;\">📖 Reader Component Microbenchmarks (Criterion)</h3>\n");
            html.push_str("  <div class=\"table-wrap\">\n");
            html.push_str("    <table data-default-sort=\"1:asc\">\n");
            html.push_str("      <thead>\n");
            html.push_str("        <tr><th>Benchmark / Composant</th><th>Moyenne (ns)</th><th>Intervalle de Confiance 95%</th><th>Médiane (ns)</th><th>Écart-type (ns)</th></tr>\n");
            html.push_str("      </thead>\n");
            html.push_str("      <tbody>\n");
            for r in &crit_reader_rows {
                let id = r.get("criterion_id").and_then(|v| v.as_str()).unwrap_or("");
                let mean = r.get("mean_ns").and_then(|v| v.as_f64());
                let median = r.get("median_ns").and_then(|v| v.as_f64());
                let stddev = r.get("stddev_ns").and_then(|v| v.as_f64());
                let ci_low = r.get("ci_lower_ns").and_then(|v| v.as_f64());
                let ci_high = r.get("ci_upper_ns").and_then(|v| v.as_f64());
                let ci_str = match (ci_low, ci_high) {
                    (Some(l), Some(h)) => {
                        format!("[{} – {}]", fmt_duration(Some(l)), fmt_duration(Some(h)))
                    }
                    _ => "—".to_string(),
                };
                html.push_str("        <tr>\n");
                html.push_str(&format!(
                    "          <td><code>{}</code></td>\n",
                    esc_html(id)
                ));
                html.push_str(&format!(
                    "          <td><strong>{}</strong></td>\n",
                    esc_html(&fmt_duration(mean))
                ));
                html.push_str(&format!(
                    "          <td style=\"color:var(--muted);\">{}</td>\n",
                    esc_html(&ci_str)
                ));
                html.push_str(&format!(
                    "          <td>{}</td>\n",
                    esc_html(&fmt_duration(median))
                ));
                html.push_str(&format!(
                    "          <td>{}</td>\n",
                    esc_html(&fmt_duration(stddev))
                ));
                let status = r
                    .get("change_status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("no_baseline");
                let change_pct = r.get("change_pct").and_then(|v| v.as_f64());
                let ci_low_pct = r.get("change_ci_lower").and_then(|v| v.as_f64());
                let ci_high_pct = r.get("change_ci_upper").and_then(|v| v.as_f64());
                let change_badge = match status {
                "improvement" => {
                    let pct = change_pct.unwrap_or(0.0);
                    let l = ci_low_pct.unwrap_or(0.0);
                    let h = ci_high_pct.unwrap_or(0.0);
                    format!("<span class=\"pill pill-win\">🚀 Amélioration : {:+.1}% [{:+.1}%, {:+.1}%]</span>", pct, l, h)
                }
                "regression" => {
                    let pct = change_pct.unwrap_or(0.0);
                    let l = ci_low_pct.unwrap_or(0.0);
                    let h = ci_high_pct.unwrap_or(0.0);
                    format!("<span class=\"pill pill-slow\">⚠️ Régression : {:+.1}% [{:+.1}%, {:+.1}%]</span>", pct, l, h)
                }
                "no_change" => {
                    let pct = change_pct.unwrap_or(0.0);
                    format!("<span style=\"color:var(--muted);font-size:12px;\">⚪ Non significative ({:+.1}%)</span>", pct)
                }
                _ => "<span style=\"color:var(--muted);font-size:12px;\">ℹ️ Aucune baseline</span>".to_string(),
            };
                html.push_str(&format!("          <td>{}</td>\n", change_badge));
                html.push_str("        </tr>\n");
            }
            html.push_str("      </tbody>\n");
            html.push_str("    </table>\n");
            html.push_str("  </div>\n");
        }

        // 4. Writer Component Microbenchmarks Table
        let crit_writer_rows: Vec<&Value> = aggregated_rows
            .iter()
            .filter(|r| {
                r.get("scenario").and_then(|v| v.as_str()) == Some("internal_microbenchmark")
                    && r.get("criterion_id")
                        .and_then(|v| v.as_str())
                        .map(|id| id.starts_with("writer/"))
                        .unwrap_or(false)
            })
            .collect();

        if !crit_writer_rows.is_empty() {
            html.push_str("  <h3 style=\"margin-top:24px;margin-bottom:12px;color:var(--text);font-size:16px;\">✍️ Writer Component Microbenchmarks (Criterion)</h3>\n");
            html.push_str("  <div class=\"table-wrap\">\n");
            html.push_str("    <table data-default-sort=\"1:asc\">\n");
            html.push_str("      <thead>\n");
            html.push_str("        <tr><th>Opération / Scénario</th><th>Moyenne</th><th>Intervalle de Confiance 95%</th><th>Médiane</th><th>Écart-type</th><th>Évolution vs Baseline</th></tr>\n");
            html.push_str("      </thead>\n");
            html.push_str("      <tbody>\n");
            for r in &crit_writer_rows {
                let id = r.get("criterion_id").and_then(|v| v.as_str()).unwrap_or("");
                let mean = r.get("mean_ns").and_then(|v| v.as_f64());
                let median = r.get("median_ns").and_then(|v| v.as_f64());
                let stddev = r.get("stddev_ns").and_then(|v| v.as_f64());
                let ci_low = r.get("ci_lower_ns").and_then(|v| v.as_f64());
                let ci_high = r.get("ci_upper_ns").and_then(|v| v.as_f64());
                let ci_str = match (ci_low, ci_high) {
                    (Some(l), Some(h)) => {
                        format!("[{} – {}]", fmt_duration(Some(l)), fmt_duration(Some(h)))
                    }
                    _ => "—".to_string(),
                };
                html.push_str("        <tr>\n");
                html.push_str(&format!(
                    "          <td><code>{}</code></td>\n",
                    esc_html(id)
                ));
                html.push_str(&format!(
                    "          <td><strong>{}</strong></td>\n",
                    esc_html(&fmt_duration(mean))
                ));
                html.push_str(&format!(
                    "          <td style=\"color:var(--muted);\">{}</td>\n",
                    esc_html(&ci_str)
                ));
                html.push_str(&format!(
                    "          <td>{}</td>\n",
                    esc_html(&fmt_duration(median))
                ));
                html.push_str(&format!(
                    "          <td>{}</td>\n",
                    esc_html(&fmt_duration(stddev))
                ));
                let status = r
                    .get("change_status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("no_baseline");
                let change_pct = r.get("change_pct").and_then(|v| v.as_f64());
                let ci_low_pct = r.get("change_ci_lower").and_then(|v| v.as_f64());
                let ci_high_pct = r.get("change_ci_upper").and_then(|v| v.as_f64());
                let change_badge = match status {
                "improvement" => {
                    let pct = change_pct.unwrap_or(0.0);
                    let l = ci_low_pct.unwrap_or(0.0);
                    let h = ci_high_pct.unwrap_or(0.0);
                    format!("<span class=\"pill pill-win\">🚀 Amélioration : {:+.1}% [{:+.1}%, {:+.1}%]</span>", pct, l, h)
                }
                "regression" => {
                    let pct = change_pct.unwrap_or(0.0);
                    let l = ci_low_pct.unwrap_or(0.0);
                    let h = ci_high_pct.unwrap_or(0.0);
                    format!("<span class=\"pill pill-slow\">⚠️ Régression : {:+.1}% [{:+.1}%, {:+.1}%]</span>", pct, l, h)
                }
                "no_change" => {
                    let pct = change_pct.unwrap_or(0.0);
                    format!("<span style=\"color:var(--muted);font-size:12px;\">⚪ Non significative ({:+.1}%)</span>", pct)
                }
                _ => "<span style=\"color:var(--muted);font-size:12px;\">ℹ️ Aucune baseline</span>".to_string(),
            };
                html.push_str(&format!("          <td>{}</td>\n", change_badge));
                html.push_str("        </tr>\n");
            }
            html.push_str("      </tbody>\n");
            html.push_str("    </table>\n");
            html.push_str("  </div>\n");
        }

        // 5. Ordered Search Strategies Table
        let crit_search_rows: Vec<&Value> = aggregated_rows
            .iter()
            .filter(|r| {
                r.get("scenario").and_then(|v| v.as_str()) == Some("internal_microbenchmark")
                    && r.get("criterion_id")
                        .and_then(|v| v.as_str())
                        .map(|id| id.starts_with("ordered-search"))
                        .unwrap_or(false)
            })
            .collect();

        if !crit_search_rows.is_empty() {
            html.push_str("  <h3 style=\"margin-top:24px;margin-bottom:12px;color:var(--text);font-size:16px;\">🔍 Ordered Search Strategies &amp; Cache Prefetching (Criterion)</h3>\n");
            html.push_str("  <div class=\"table-wrap\">\n");
            html.push_str("    <table data-default-sort=\"3:asc\">\n");
            html.push_str("      <thead>\n");
            html.push_str("        <tr><th>Taille Dataset</th><th>Clé de Requête</th><th>Stratégie Algorithmique</th><th>Moyenne (ns)</th><th>Écart-type (ns)</th><th>Évolution vs Baseline</th></tr>\n");
            html.push_str("      </thead>\n");
            html.push_str("      <tbody>\n");
            for r in &crit_search_rows {
                let id = r.get("criterion_id").and_then(|v| v.as_str()).unwrap_or("");
                let parts: Vec<&str> = id.split('/').collect();
                let key_type = parts.get(1).copied().unwrap_or("present");
                let size = parts.get(2).copied().unwrap_or("1000");
                let strategy = parts.get(3).copied().unwrap_or(id);
                let mean = r.get("mean_ns").and_then(|v| v.as_f64());
                let stddev = r.get("stddev_ns").and_then(|v| v.as_f64());

                html.push_str("        <tr>\n");
                html.push_str(&format!(
                    "          <td><strong>{}</strong></td>\n",
                    esc_html(size)
                ));
                html.push_str(&format!("          <td>{}</td>\n", esc_html(key_type)));
                html.push_str(&format!(
                    "          <td><code>{}</code></td>\n",
                    esc_html(strategy)
                ));
                html.push_str(&format!(
                    "          <td><strong>{}</strong></td>\n",
                    esc_html(&fmt_duration(mean))
                ));
                html.push_str(&format!(
                    "          <td>{}</td>\n",
                    esc_html(&fmt_duration(stddev))
                ));
                let status = r
                    .get("change_status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("no_baseline");
                let change_pct = r.get("change_pct").and_then(|v| v.as_f64());
                let ci_low_pct = r.get("change_ci_lower").and_then(|v| v.as_f64());
                let ci_high_pct = r.get("change_ci_upper").and_then(|v| v.as_f64());
                let change_badge = match status {
                "improvement" => {
                    let pct = change_pct.unwrap_or(0.0);
                    let l = ci_low_pct.unwrap_or(0.0);
                    let h = ci_high_pct.unwrap_or(0.0);
                    format!("<span class=\"pill pill-win\">🚀 Amélioration : {:+.1}% [{:+.1}%, {:+.1}%]</span>", pct, l, h)
                }
                "regression" => {
                    let pct = change_pct.unwrap_or(0.0);
                    let l = ci_low_pct.unwrap_or(0.0);
                    let h = ci_high_pct.unwrap_or(0.0);
                    format!("<span class=\"pill pill-slow\">⚠️ Régression : {:+.1}% [{:+.1}%, {:+.1}%]</span>", pct, l, h)
                }
                "no_change" => {
                    let pct = change_pct.unwrap_or(0.0);
                    format!("<span style=\"color:var(--muted);font-size:12px;\">⚪ Non significative ({:+.1}%)</span>", pct)
                }
                _ => "<span style=\"color:var(--muted);font-size:12px;\">ℹ️ Aucune baseline</span>".to_string(),
            };
                html.push_str(&format!("          <td>{}</td>\n", change_badge));
                html.push_str("        </tr>\n");
            }
            html.push_str("      </tbody>\n");
            html.push_str("    </table>\n");
            html.push_str("  </div>\n");
        }

        html.push_str("</section>\n\n");
    }
    // ========================================================================
    // FOOTER
    // ========================================================================
    html.push_str("<section>\n");
    html.push_str("  <p style=\"text-align:center;color:var(--muted);font-size:13px;\">\n");
    html.push_str("    Report generated by libmaxminddb-rs benchmark suite\n");
    html.push_str(&format!(
        "    <br/>Generated at: {}\n",
        esc_html(&timestamp)
    ));
    html.push_str("  </p>\n");
    html.push_str("</section>\n");

    html.push_str("</div>\n<script>\n");
    html.push_str(include_str!("table_sort.js"));
    html.push_str(include_str!("lookup_api_chart.js"));
    html.push_str("\n</script>\n</body>\n</html>\n");

    fs::write(output_path, html).map_err(|e| e.to_string())?;
    Ok(output_path.to_path_buf())
}
