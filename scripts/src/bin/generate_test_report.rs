use std::fs;

use serde_json::json;

use bench_runner::config::{ROOT, TARGET};
use bench_runner::metrics::esc_html;

#[derive(Debug, Clone)]
struct TestCase {
    name: String,
    status: String,
}

#[derive(Debug, Clone)]
struct TestSuite {
    name: String,
    cases: Vec<TestCase>,
}

fn parse_cargo_test_log(content: &str) -> Vec<TestSuite> {
    let mut suites = Vec::new();
    let mut current_suite = TestSuite {
        name: "libmaxminddb-rs".into(),
        cases: Vec::new(),
    };

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("test ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 && parts[parts.len() - 2] == "..." {
                let name = parts[1].to_string();
                let status = match parts[parts.len() - 1] {
                    "ok" => "passed",
                    "FAILED" => "failed",
                    "ignored" => "skipped",
                    _ => "unknown",
                };
                current_suite.cases.push(TestCase {
                    name,
                    status: status.into(),
                });
            }
        } else if line.starts_with("PASS ") || line.starts_with("FAIL ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 5 {
                let status = if parts[0] == "PASS" {
                    "passed"
                } else {
                    "failed"
                };
                let test_name = parts[parts.len() - 1].to_string();
                current_suite.cases.push(TestCase {
                    name: test_name,
                    status: status.into(),
                });
            }
        } else if line.starts_with("Running ") {
            if !current_suite.cases.is_empty() {
                suites.push(current_suite);
            }
            let name = line.replace("Running ", "").trim().to_string();
            current_suite = TestSuite {
                name,
                cases: Vec::new(),
            };
        }
    }

    if !current_suite.cases.is_empty() {
        suites.push(current_suite);
    }

    suites
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let log_path = TARGET.join("test_output.log");
    let _xml_path = TARGET.join("test_results.xml");
    let report_dir = ROOT.join("test-report");
    let report_path = report_dir.join("index.html");
    let json_path = TARGET.join("test-results.json");

    let suites = if log_path.exists() {
        let text = fs::read_to_string(&log_path)?;
        parse_cargo_test_log(&text)
    } else {
        Vec::new()
    };

    let mut total_passed = 0;
    let mut total_failed = 0;
    let mut total_skipped = 0;
    for s in &suites {
        for c in &s.cases {
            match c.status.as_str() {
                "passed" => total_passed += 1,
                "failed" => total_failed += 1,
                "skipped" => total_skipped += 1,
                _ => {}
            }
        }
    }
    let total_tests = total_passed + total_failed + total_skipped;

    println!(
        "🧪 Test Report Summary: {total_passed} passed, {total_failed} failed, {total_skipped} skipped ({total_tests} total)"
    );

    // Write JSON
    fs::create_dir_all(&*TARGET)?;
    let json_data = json!({
        "total": total_tests,
        "passed": total_passed,
        "failed": total_failed,
        "skipped": total_skipped,
        "suites": suites.iter().map(|s| {
            json!({
                "name": s.name,
                "tests": s.cases.len(),
                "cases": s.cases.iter().map(|c| {
                    json!({
                        "name": c.name,
                        "status": c.status,
                    })
                }).collect::<Vec<_>>()
            })
        }).collect::<Vec<_>>()
    });
    fs::write(&json_path, serde_json::to_string_pretty(&json_data)?)?;

    // Write HTML
    fs::create_dir_all(&report_dir)?;
    let mut html = String::new();
    html.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Test Report</title>");
    html.push_str(
        "<style>body{background:#0d1117;color:#c9d1d9;font-family:sans-serif;padding:24px;}",
    );
    html.push_str("h1{color:#58a6ff;} table{width:100%;border-collapse:collapse;} th,td{padding:8px;border:1px solid #30363d;}");
    html.push_str(
        ".passed{color:#3fb950;} .failed{color:#f85149;font-weight:bold;} .skipped{color:#8b949e;}",
    );
    html.push_str("</style></head><body>");
    html.push_str(&format!(
        "<h1>🧪 Test Report: {total_passed}/{total_tests} Passed</h1>"
    ));
    html.push_str(
        "<table><thead><tr><th>Suite</th><th>Test Name</th><th>Status</th></tr></thead><tbody>",
    );
    for s in &suites {
        for c in &s.cases {
            html.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td class=\"{}\">{}</td></tr>",
                esc_html(&s.name),
                esc_html(&c.name),
                c.status,
                c.status.to_uppercase()
            ));
        }
    }
    html.push_str("</tbody></table></body></html>");
    fs::write(&report_path, html)?;

    println!("✅ Test report written to {}", report_path.display());
    Ok(())
}
