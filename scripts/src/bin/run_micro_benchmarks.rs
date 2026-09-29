//! One fresh result directory per invocation, with native lookup and isolated RSS reports.
use std::fs;
use std::io::IsTerminal;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const NOISE_THRESHOLD_PCT: f64 = 3.0;

fn command_text(root: &Path, program: &str, args: &[&str]) -> String {
    match Command::new(program).args(args).current_dir(root).output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().into()
        }
        _ => "unavailable".into(),
    }
}

fn estimates(dir: &Path, rows: &mut Vec<Value>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().is_some_and(|n| n == "new") && path.join("estimates.json").exists() {
            let stats: Value = serde_json::from_slice(&fs::read(path.join("estimates.json"))?)?;
            let bench: Value = serde_json::from_slice(&fs::read(path.join("benchmark.json"))?)?;
            rows.push(json!({"name": bench["full_id"], "mean_ns": stats["mean"]["point_estimate"],
                "confidence_interval_ns": stats["mean"]["confidence_interval"], "throughput": bench["throughput"]}));
        } else {
            estimates(&path, rows)?;
        }
    }
    Ok(())
}

fn mib(value: &Value) -> Result<f64> {
    Ok(value.as_u64().ok_or("missing or invalid memory sample")? as f64 / 1_048_576.0)
}

// Criterion keeps the raw samples and estimates for each named baseline in a
// separate directory under every benchmark. Copy only those directories: the
// generated plots are not needed to compare a later run.
fn copy_criterion_baseline(source: &Path, destination: &Path, from: &str) -> Result<usize> {
    let mut copied = 0;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if entry.file_name() == from {
            let target = destination.join("baseline");
            fs::create_dir_all(&target)?;
            for file in fs::read_dir(&path)? {
                let file = file?;
                if file.file_type()?.is_file() {
                    fs::copy(file.path(), target.join(file.file_name()))?;
                }
            }
            copied += 1;
        } else {
            copied += copy_criterion_baseline(&path, &destination.join(entry.file_name()), from)?;
        }
    }
    Ok(copied)
}

fn comparable(baseline: &Value, current: &Value) -> bool {
    let previous = &baseline["environment"];
    let now = &current["environment"];
    [
        "rustc",
        "cpu",
        "architecture",
        "rustflags",
        "encoded_rustflags",
        "bench_cpu",
        "profile",
        "cargo_config",
    ]
    .iter()
    .all(|key| previous[*key] == now[*key])
        && baseline["fixtures"] == current["fixtures"]
}

fn change(current: f64, baseline: f64) -> (String, &'static str) {
    let percent = (current / baseline - 1.0) * 100.0;
    let boundary = NOISE_THRESHOLD_PCT / 100.0;
    let status = if current > baseline * (1.0 + boundary) {
        "🔴 Regression"
    } else if current < baseline * (1.0 - boundary) {
        "🟢 Improvement"
    } else {
        "Noise"
    };
    (format!("{percent:+.2}%"), status)
}

fn row_class(line: &str) -> Option<&'static str> {
    if !line.starts_with('|') {
        return None;
    }
    match line.rsplit('|').nth(1).map(str::trim) {
        Some("🔴 Regression") => Some("regression"),
        Some("🟢 Improvement") => Some("improvement"),
        _ => None,
    }
}

fn terminal_report(summary: &str, color: bool) -> String {
    if !color {
        return summary.to_owned();
    }
    let mut output = String::with_capacity(summary.len());
    for line in summary.split_inclusive('\n') {
        let code = match row_class(line) {
            Some("regression") => Some("\x1b[31m"),
            Some("improvement") => Some("\x1b[32m"),
            _ => None,
        };
        if let Some(code) = code {
            output.push_str(code);
            output.push_str(line.trim_end_matches('\n'));
            output.push_str("\x1b[0m");
            if line.ends_with('\n') {
                output.push('\n');
            }
        } else {
            output.push_str(line);
        }
    }
    output
}

fn html_report(summary: &str) -> String {
    let mut output = String::from(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Native microbenchmarks</title><style>body{background:#111827;color:#e5e7eb;font:14px ui-monospace,SFMono-Regular,Consolas,monospace;padding:24px}pre{overflow-x:auto;line-height:1.5}.regression{color:#ff6b6b}.improvement{color:#4ade80}</style><pre>",
    );
    for line in summary.split_inclusive('\n') {
        if let Some(class) = row_class(line) {
            output.push_str("<span class=\"");
            output.push_str(class);
            output.push_str("\">");
        }
        for character in line.chars() {
            match character {
                '&' => output.push_str("&amp;"),
                '<' => output.push_str("&lt;"),
                '>' => output.push_str("&gt;"),
                '"' => output.push_str("&quot;"),
                _ => output.push(character),
            }
        }
        if row_class(line).is_some() {
            output.push_str("</span>");
        }
    }
    output.push_str("</pre></html>\n");
    output
}

fn baseline_row<'a>(rows: &'a [Value], key: &str, value: &str) -> Option<&'a Value> {
    rows.iter().find(|row| row[key].as_str() == Some(value))
}

fn append_markdown_row(
    table: &mut String,
    cells: &[&str],
    columns: &[(&str, bool)],
    widths: &[usize],
) {
    table.push('|');
    for (index, cell) in cells.iter().enumerate() {
        let padding = widths[index] - UnicodeWidthStr::width(*cell);
        table.push(' ');
        if columns[index].1 {
            table.push_str(&" ".repeat(padding));
        }
        table.push_str(cell);
        if !columns[index].1 {
            table.push_str(&" ".repeat(padding));
        }
        table.push_str(" |");
    }
    table.push('\n');
}

fn markdown_table(columns: &[(&str, bool)], rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(index, (heading, _))| {
            rows.iter()
                .map(|row| UnicodeWidthStr::width(row[index].as_str()))
                .chain(std::iter::once(UnicodeWidthStr::width(*heading)))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut table = String::new();
    append_markdown_row(
        &mut table,
        &columns
            .iter()
            .map(|(heading, _)| *heading)
            .collect::<Vec<_>>(),
        columns,
        &widths,
    );
    table.push('|');
    for (index, (_, right_aligned)) in columns.iter().enumerate() {
        table.push_str(&"-".repeat(widths[index] + 1));
        table.push(if *right_aligned { ':' } else { '-' });
        table.push('|');
    }
    table.push('\n');
    for row in rows {
        append_markdown_row(
            &mut table,
            &row.iter().map(String::as_str).collect::<Vec<_>>(),
            columns,
            &widths,
        );
    }
    table
}

fn report(performance: &[Value], memory: &[Value], baseline: Option<&Value>) -> Result<String> {
    let mut text = String::from(
        "# Native microbenchmarks\n\nLatency is the mean per Criterion iteration (95% bootstrap CI in summary.json).\nBatch benchmarks report a whole batch, not a single lookup.\n\n",
    );
    let mut performance_rows = Vec::with_capacity(performance.len());
    for row in performance {
        let name = row["name"].as_str().ok_or("missing benchmark name")?;
        let mean = row["mean_ns"].as_f64().ok_or("missing estimate")?;
        let previous = baseline
            .and_then(|b| b["performance"].as_array())
            .and_then(|rows| baseline_row(rows, "name", name))
            .and_then(|row| row["mean_ns"].as_f64());
        let (delta, status) = previous
            .filter(|value| *value > 0.0)
            .map(|value| change(mean, value))
            .unwrap_or_else(|| ("—".into(), "—"));
        performance_rows.push(vec![
            name.to_owned(),
            format!("{mean:.2}"),
            previous.map_or_else(|| "—".into(), |value| format!("{value:.2}")),
            delta,
            status.into(),
        ]);
    }
    text.push_str(&markdown_table(
        &[
            ("Scenario", false),
            ("Mean (ns/iteration)", true),
            ("Baseline (ns/iteration)", true),
            ("Change", true),
            ("Result", false),
        ],
        &performance_rows,
    ));
    if baseline.is_none() {
        text.push_str("\nBaseline created by this run; comparisons begin with the next run.\n");
    } else {
        text.push_str("\nA change within ±3% is treated as noise. Lower latency is better.\n");
    }
    text.push_str("\nMemory: 1,000,000 routes per family, 2,000,000 borrowed lookups (50% hits). All sizes in MiB. Each row is a fresh exec; fixture generation is excluded.\n\n");
    let mut memory_rows = Vec::with_capacity(memory.len());
    for m in memory {
        let before = mib(&m["before_open"]["rss_bytes"])?;
        let after = mib(&m["after_lookups"]["rss_bytes"])?;
        let family = m["family"].as_str().ok_or("missing memory family")?;
        let opening = m["opening"].as_str().ok_or("missing opening mode")?;
        let peak = mib(&m["after_lookups"]["peak_rss_bytes"])?;
        let previous = baseline
            .and_then(|b| b["memory"].as_array())
            .and_then(|rows| {
                rows.iter().find(|row| {
                    row["family"].as_str() == Some(family)
                        && row["opening"].as_str() == Some(opening)
                })
            })
            .map(|row| mib(&row["after_lookups"]["peak_rss_bytes"]))
            .transpose()?;
        let (delta, status) = previous
            .filter(|value| *value > 0.0)
            .map(|value| change(peak, value))
            .unwrap_or_else(|| ("—".into(), "—"));
        memory_rows.push(vec![
            family.to_owned(),
            opening.to_owned(),
            format!("{:.2}", mib(&m["database_bytes"])?),
            format!("{:.2}", mib(&m["auxiliary_capacity_bytes"])?),
            format!("{:.2}", mib(&m["before_auxiliary"]["rss_bytes"])?),
            format!("{before:.2}"),
            format!("{:.2}", mib(&m["after_open"]["rss_bytes"])?),
            format!("{after:.2}"),
            format!("{peak:.2}"),
            format!("{:.2}", after - before),
            previous.map_or_else(|| "—".into(), |value| format!("{value:.2}")),
            delta,
            status.into(),
        ]);
    }
    text.push_str(&markdown_table(
        &[
            ("Family", false),
            ("Open mode", false),
            ("MMDB", true),
            ("Auxiliary buffers¹", true),
            ("RSS before aux", true),
            ("RSS before open", true),
            ("RSS after open", true),
            ("RSS after lookups", true),
            ("Peak RSS²", true),
            ("Reader RSS delta³", true),
            ("Baseline peak", true),
            ("Peak change", true),
            ("Result", false),
        ],
        &memory_rows,
    ));
    if baseline.is_some() {
        text.push_str(
            "\nPeak RSS changes within ±3% are treated as noise. Lower memory usage is better.\n",
        );
    }
    text.push_str("\n¹ Exact capacity of the two query Vecs; RSS includes these buffers, the runtime and the reader.\n² Linux VmHWM: process high-water RSS, including temporary allocations during open; not cumulative allocation traffic or system page cache.\n³ RSS after lookups minus RSS before open: an estimate of resident reader/index/file pages, not an exact allocator measurement.\n\nOwned = Reader::open (file copied into memory); mmap = Reader::open_mmap (file-backed pages faulted on demand). Both prepare the fast tree during open. RssAnon/RssFile snapshots and worker PIDs are in memory.json. These are warm lookup measurements, not cold-disk benchmarks.\n");
    Ok(text)
}

fn run() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("missing project root")?;
    let cargo = std::env::args().nth(1).unwrap_or_else(|| "cargo".into());
    let base = root.join("target/micro-benchmarks");
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let run = base
        .join("runs")
        .join(format!("{stamp}-{}", std::process::id()));
    fs::create_dir_all(&run)?;
    let baseline_dir = root.join("benchmarks/baseline");
    let baseline = if baseline_dir.exists() {
        Some(serde_json::from_slice::<Value>(&fs::read(
            baseline_dir.join("summary.json"),
        )?)?)
    } else {
        None
    };
    if baseline.is_some() {
        let copied = copy_criterion_baseline(
            &baseline_dir.join("criterion"),
            &run.join("criterion"),
            "baseline",
        )?;
        if copied == 0 {
            return Err("Criterion baseline contains no benchmark samples".into());
        }
        println!(
            "Comparing against {} ({copied} Criterion baselines)",
            baseline_dir.display()
        );
    } else {
        println!("Creating baseline at {}", baseline_dir.display());
    }
    let cpu = fs::read_to_string("/proc/cpuinfo")
        .unwrap_or_default()
        .lines()
        .find_map(|line| {
            line.strip_prefix("model name")
                .and_then(|s| s.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        });
    let environment = json!({
        "rustc": command_text(root, "rustc", &["-Vv"]), "revision": command_text(root, "git", &["rev-parse", "HEAD"]),
        "working_tree": command_text(root, "git", &["status", "--short"]), "cpu": cpu,
        "os": command_text(root, "uname", &["-a"]), "architecture": std::env::consts::ARCH,
        "rustflags": std::env::var("RUSTFLAGS").ok(), "encoded_rustflags": std::env::var("CARGO_ENCODED_RUSTFLAGS").ok(),
        "bench_cpu": std::env::var("BENCH_CPU").ok(), "profile": "Cargo bench: opt-level=3; debug=true; default features",
        "cargo_config": ([root.join(".cargo/config.toml"), root.join(".cargo/config")].iter().filter_map(|p| fs::read_to_string(p).ok()).collect::<Vec<_>>()),
        "started_unix_ns": stamp.to_string(), "command": "cargo bench --bench reader --bench writer --bench search_strategies --bench million_reader"
    });
    fs::write(
        run.join("environment.json"),
        serde_json::to_vec_pretty(&environment)?,
    )?;
    println!("Microbenchmark results: {}", run.display());
    let mut command = if let Ok(cpu) = std::env::var("BENCH_CPU") {
        let mut command = Command::new("taskset");
        command.args(["-c", &cpu, &cargo]);
        command
    } else {
        Command::new(&cargo)
    };
    command.current_dir(root).args([
        "bench",
        "--bench",
        "reader",
        "--bench",
        "writer",
        "--bench",
        "search_strategies",
        "--bench",
        "million_reader",
    ]);
    if baseline.is_some() {
        command.args([
            "--",
            "--baseline-lenient",
            "baseline",
            "--noise-threshold",
            "0.03",
        ]);
    }
    let status = command
        .env("CRITERION_HOME", run.join("criterion"))
        .env("MMDB_MICRO_RUN_DIR", &run)
        .env("MMDB_MICRO_FIXTURE_DIR", base.join("fixtures"))
        .status()?;
    if !status.success() {
        return Err(format!(
            "microbenchmarks failed: {status}; partial artifacts at {}",
            run.display()
        )
        .into());
    }
    let mut performance = Vec::new();
    estimates(&run.join("criterion"), &mut performance)?;
    performance.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    if performance.is_empty() {
        return Err("no Criterion measurements produced".into());
    }
    let memory: Vec<Value> = serde_json::from_slice(&fs::read(run.join("memory.json"))?)?;
    if memory.len() != 4 {
        return Err("expected four isolated memory scenarios".into());
    }
    let mut fixtures = Vec::new();
    for family in ["ipv4", "ipv6"] {
        let dir = base.join("fixtures").join(format!("{family}-1000000"));
        let manifest: Value = serde_json::from_slice(&fs::read(dir.join("manifest.json"))?)?;
        let hashes: Vec<_> = ["database.mmdb", "hits.bin", "misses.bin"].iter().map(|name| {
            json!({"file": name, "sha256": command_text(root, "sha256sum", &[dir.join(name).to_str().unwrap()])})
        }).collect();
        fixtures.push(json!({"manifest": manifest, "hashes": hashes}));
    }
    let results = json!({"environment": environment,
        "fixtures": fixtures, "performance": performance, "memory": memory});
    if let Some(previous) = &baseline
        && !comparable(previous, &results)
    {
        return Err(format!(
            "baseline environment or fixtures differ; inspect {} and create a new baseline only after validating the change",
            baseline_dir.display()
        )
        .into());
    }
    fs::write(
        run.join("summary.json"),
        serde_json::to_vec_pretty(&results)?,
    )?;
    let summary = report(&performance, &memory, baseline.as_ref())?;
    fs::write(run.join("summary.md"), &summary)?;
    fs::write(run.join("summary.html"), html_report(&summary))?;
    if baseline.is_none() {
        let stage = root.join("benchmarks").join(format!(".baseline-{stamp}"));
        fs::create_dir(&stage)?;
        let copied =
            copy_criterion_baseline(&run.join("criterion"), &stage.join("criterion"), "base")?;
        if copied != performance.len() {
            return Err(format!(
                "Criterion produced {copied} baseline samples for {} scenarios",
                performance.len()
            )
            .into());
        }
        fs::write(
            stage.join("summary.json"),
            serde_json::to_vec_pretty(&results)?,
        )?;
        fs::rename(stage, &baseline_dir)?;
        println!(
            "Saved {copied} Criterion baselines to {}",
            baseline_dir.display()
        );
    }
    fs::write(base.join("latest.txt"), run.display().to_string())?;
    let color = std::io::stdout().is_terminal();
    println!("\n{}", terminal_report(&summary, color));
    println!("Markdown report: {}", run.join("summary.md").display());
    println!(
        "Colored HTML report: {}",
        run.join("summary.html").display()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("micro-benchmark: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_iterations_without_dividing_batch_latency() {
        let result = report(
            &[json!({"name":"batch_256", "mean_ns": 25600.0})],
            &[],
            None,
        )
        .unwrap();
        let row = result
            .lines()
            .find(|line| line.contains("batch_256"))
            .unwrap();
        let cells: Vec<_> = row.split('|').map(str::trim).collect();
        assert_eq!(cells[1..3], ["batch_256", "25600.00"]);
        assert!(result.contains("whole batch"));
    }

    #[test]
    fn markdown_columns_are_visually_aligned() {
        let table = markdown_table(
            &[("Scenario", false), ("Mean (ns/iteration)", true)],
            &[
                vec!["short".into(), "7.00".into()],
                vec!["longer_name".into(), "12345.67".into()],
            ],
        );
        let widths: Vec<_> = table.lines().map(UnicodeWidthStr::width).collect();
        assert!(widths.iter().all(|width| *width == widths[0]));
        assert!(table.contains("| short       |                7.00 |"));
    }

    #[test]
    fn failed_estimate_is_not_a_zero_latency() {
        assert!(report(&[json!({"name":"broken"})], &[], None).is_err());
    }

    #[test]
    fn comparison_marks_only_changes_outside_three_percent() {
        assert_eq!(change(96.0, 100.0).1, "🟢 Improvement");
        assert_eq!(change(104.0, 100.0).1, "🔴 Regression");
        assert_eq!(change(103.0, 100.0).1, "Noise");
        assert_eq!(change(97.0, 100.0).1, "Noise");
    }

    #[test]
    fn full_result_rows_are_colored_in_terminal_and_html() {
        let summary = "| slow | 🔴 Regression |\n| fast | 🟢 Improvement |\n| same | Noise |\n";
        let terminal = terminal_report(summary, true);
        assert!(terminal.contains("\x1b[31m| slow | 🔴 Regression |\x1b[0m\n"));
        assert!(terminal.contains("\x1b[32m| fast | 🟢 Improvement |\x1b[0m\n"));
        assert!(terminal.contains("| same | Noise |\n"));
        assert_eq!(terminal_report(summary, false), summary);
        let html = html_report(summary);
        assert!(html.contains("<span class=\"regression\">| slow | 🔴 Regression |\n</span>"));
        assert!(html.contains("<span class=\"improvement\">| fast | 🟢 Improvement |\n</span>"));
        assert!(html.contains("| same | Noise |\n"));
    }

    #[test]
    fn report_compares_latency_and_peak_rss() {
        let baseline = json!({
            "performance": [{"name":"lookup", "mean_ns":100.0}],
            "memory": [{"family":"ipv4", "opening":"mmap", "after_lookups":{"peak_rss_bytes":104857600}}]
        });
        let memory = json!({"family":"ipv4", "opening":"mmap", "database_bytes":1048576,
            "auxiliary_capacity_bytes":1048576, "before_auxiliary":{"rss_bytes":1048576},
            "before_open":{"rss_bytes":1048576}, "after_open":{"rss_bytes":1048576},
            "after_lookups":{"rss_bytes":1048576,"peak_rss_bytes":110100480}});
        let result = report(
            &[json!({"name":"lookup", "mean_ns": 95.0})],
            &[memory],
            Some(&baseline),
        )
        .unwrap();
        assert!(result.contains("🟢 Improvement"));
        assert!(result.contains("🔴 Regression"));
        assert!(result.contains("+5.00%"));
    }

    #[test]
    fn criterion_samples_are_copied_under_the_named_baseline() {
        let root = std::env::temp_dir().join(format!(
            "mmdb-micro-baseline-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source/reader/lookup/base");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("sample.json"), b"sample").unwrap();
        let destination = root.join("destination");
        assert_eq!(
            copy_criterion_baseline(&root.join("source"), &destination, "base").unwrap(),
            1
        );
        assert_eq!(
            fs::read(destination.join("reader/lookup/baseline/sample.json")).unwrap(),
            b"sample"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn comparison_rejects_different_hardware_or_fixtures() {
        let baseline = json!({"environment":{"rustc":"rustc", "cpu":"A", "architecture":"x86_64",
            "rustflags":null, "encoded_rustflags":null, "bench_cpu":null,
            "profile":"bench", "cargo_config":[]}, "fixtures":[{"hash":"one"}]});
        assert!(comparable(&baseline, &baseline));
        let mut different_cpu = baseline.clone();
        different_cpu["environment"]["cpu"] = json!("B");
        assert!(!comparable(&baseline, &different_cpu));
        let mut different_fixture = baseline.clone();
        different_fixture["fixtures"][0]["hash"] = json!("two");
        assert!(!comparable(&baseline, &different_fixture));
    }

    #[test]
    fn missing_rss_is_not_reported_as_zero() {
        assert!(mib(&Value::Null).is_err());
        assert!(mib(&json!(-1)).is_err());
        assert_eq!(mib(&json!(1_048_576)).unwrap(), 1.0);
    }
}
