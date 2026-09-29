use bench_runner::builder::cpu_model;
use bench_runner::charts::generate_standalone_svg_charts;
use bench_runner::config::{
    CHARTS_DIR, DOCS_CHARTS_DIR, README_PATH, REPORT_PATH, RESULTS_CSV_PATH, RESULTS_JSON_PATH,
    RESULTS_PATH,
};
use bench_runner::console::print_console_summary;
use bench_runner::html_report::generate_html_report;
use bench_runner::readme::update_readme_benchmarks;
use bench_runner::stats::{
    aggregate_results, export_results_csv, export_results_json, load_raw_results,
};
use std::collections::HashMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "📈 Generating benchmark report from {}...",
        RESULTS_PATH.display()
    );
    let raw = load_raw_results(&*RESULTS_PATH);
    if raw.is_empty() {
        eprintln!(
            "⚠️ No benchmark results found in {}",
            RESULTS_PATH.display()
        );
        return Ok(());
    }

    let aggregated = aggregate_results(&raw);
    export_results_json(&aggregated, &*RESULTS_JSON_PATH)?;
    export_results_csv(&aggregated, &*RESULTS_CSV_PATH)?;

    println!("📈 Generating standalone SVG charts...");
    let charts = generate_standalone_svg_charts(&aggregated, &*CHARTS_DIR);
    println!("   ✅ Generated {} standalone charts", charts.len());
    generate_standalone_svg_charts(&aggregated, &*DOCS_CHARTS_DIR);

    println!("📈 Generating HTML report...");
    let mut env_data = HashMap::new();
    let cpu = cpu_model();
    env_data.insert("cpu".into(), cpu.clone());
    generate_html_report(&aggregated, &raw, &env_data, &*REPORT_PATH, true)?;
    println!("   ✅ Report: {}", REPORT_PATH.display());

    update_readme_benchmarks(&aggregated, &*README_PATH, &cpu);
    println!("   ✅ Updated README.md");

    print_console_summary(&aggregated, None);
    Ok(())
}
