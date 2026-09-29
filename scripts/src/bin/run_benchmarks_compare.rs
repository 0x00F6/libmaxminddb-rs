use std::collections::HashMap;
use std::env;
use std::time::Instant;

use bench_runner::builder::cpu_model;
use bench_runner::charts::generate_standalone_svg_charts;
use bench_runner::config::{
    CHARTS_DIR, DOCS_CHARTS_DIR, README_PATH, REPORT_PATH, RESULTS_CSV_PATH, RESULTS_JSON_PATH,
    RESULTS_PATH,
};
use bench_runner::console::print_console_summary;
use bench_runner::html_report::generate_html_report;
use bench_runner::readme::update_readme_benchmarks;
use bench_runner::runner::run_benchmarks_compare_suite;
use bench_runner::stats::{
    aggregate_results, export_results_csv, export_results_json, load_raw_results,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let skip_competitors = args
        .iter()
        .any(|a| a == "--skip-competitors" || a == "--quick");

    let t0 = Instant::now();

    // 1. Run comparative benchmarks
    let outcome = if args.iter().any(|a| a == "--memory-only") {
        bench_runner::memory::run_suite()
    } else if args.iter().any(|a| a == "--writer-only") {
        bench_runner::runner::run_writer_only_suite()
    } else if args.iter().any(|a| a == "--ipv6-absent-only") {
        bench_runner::runner::run_ipv6_absent_suite()
    } else if args.iter().any(|a| a == "--large-sizes-only") {
        bench_runner::runner::run_large_sizes_suite()
    } else {
        run_benchmarks_compare_suite(skip_competitors)
    };
    if let Err(e) = outcome {
        eprintln!("❌ Benchmark execution error: {e}");
        std::process::exit(1);
    }

    // 2. Load & aggregate results
    let raw = load_raw_results(&*RESULTS_PATH);
    let aggregated = aggregate_results(&raw);

    // 3. Print Console Summary Tables first
    print_console_summary(&aggregated, None);

    // 4. Print final steps in requested order
    println!("\n🧠 Aggregating recorded memory measurements and calculating statistics...");
    export_results_json(&aggregated, &*RESULTS_JSON_PATH)?;
    export_results_csv(&aggregated, &*RESULTS_CSV_PATH)?;

    println!(
        "📈 Generating standalone SVG benchmark charts in {}...",
        CHARTS_DIR.display()
    );
    let charts = generate_standalone_svg_charts(&aggregated, &*CHARTS_DIR);
    println!("   ✅ Generated {} standalone charts", charts.len());

    println!(
        "📈 Generating interactive HTML report at {}...",
        REPORT_PATH.display()
    );
    let mut env_data = HashMap::new();
    let cpu = cpu_model();
    env_data.insert("cpu".into(), cpu.clone());
    env_data.insert("timestamp_utc".into(), chrono_utc_now());
    generate_html_report(&aggregated, &raw, &env_data, &*REPORT_PATH, false)?;
    println!("   ✅ Report generated: {}", REPORT_PATH.display());

    println!(
        "📝 Updating benchmark tables and charts in {}...",
        README_PATH.display()
    );
    update_readme_benchmarks(&aggregated, &*README_PATH, &cpu);
    println!("   ✅ README.md updated successfully");

    println!(
        "📈 Refreshing SVG benchmark charts in {}...",
        DOCS_CHARTS_DIR.display()
    );
    let docs_charts = generate_standalone_svg_charts(&aggregated, &*DOCS_CHARTS_DIR);
    println!("   ✅ Refreshed {} SVG charts", docs_charts.len());

    let elapsed = t0.elapsed();
    println!(
        "\n🎉 Comparative benchmarks and reports completed in {:.1}s",
        elapsed.as_secs_f64()
    );

    let abs_report = REPORT_PATH
        .canonicalize()
        .unwrap_or_else(|_| REPORT_PATH.to_path_buf());
    let report_url = format!("file://{}", abs_report.display());
    println!(
        "\n🌐 HTML Report: \x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
        report_url,
        abs_report.display()
    );

    Ok(())
}

fn chrono_utc_now() -> String {
    unsafe {
        let mut t: libc::time_t = 0;
        libc::time(&mut t);
        let mut tm: libc::tm = std::mem::zeroed();
        libc::gmtime_r(&t, &mut tm);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}
