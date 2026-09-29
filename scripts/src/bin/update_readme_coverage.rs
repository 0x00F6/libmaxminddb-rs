//! Refresh README coverage figures from cargo-llvm-cov's workspace JSON summary.

use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bench_runner::config::ROOT;
use serde::Deserialize;

#[derive(Deserialize)]
struct Coverage {
    data: Vec<Dataset>,
}

#[derive(Deserialize)]
struct Dataset {
    files: Vec<CoverageFile>,
    totals: Metrics,
}

#[derive(Deserialize)]
struct CoverageFile {
    filename: PathBuf,
    summary: Metrics,
}

#[derive(Deserialize)]
struct Metrics {
    lines: Metric,
    regions: Metric,
    functions: Metric,
}

#[derive(Deserialize)]
struct Metric {
    count: u64,
    covered: u64,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn percentage(metric: &Metric) -> Result<String, io::Error> {
    if metric.count == 0 || metric.covered > metric.count {
        return Err(invalid("invalid coverage counts"));
    }
    Ok(format!(
        "{:.2}%",
        metric.covered as f64 / metric.count as f64 * 100.0
    ))
}

fn cell(metric: &Metric) -> Result<String, io::Error> {
    Ok(format!(
        "{} ({}/{})",
        percentage(metric)?,
        metric.covered,
        metric.count
    ))
}

fn replace_marked(readme: &str, name: &str, body: &str) -> Result<String, io::Error> {
    let start = format!("<!-- {name}:start -->");
    let end = format!("<!-- {name}:end -->");
    if readme.matches(&start).count() != 1 || readme.matches(&end).count() != 1 {
        return Err(invalid(format!("expected one {name} marker pair")));
    }
    let (before, rest) = readme
        .split_once(&start)
        .ok_or_else(|| invalid("missing start marker"))?;
    let (_, after) = rest
        .split_once(&end)
        .ok_or_else(|| invalid("missing end marker"))?;
    Ok(format!("{before}{start}\n{body}\n{end}{after}"))
}

fn replace_badge(readme: &str, prefix: &str, replacement: &str) -> Result<String, io::Error> {
    if readme.matches(prefix).count() != 1 {
        return Err(invalid(format!("expected one badge with prefix {prefix}")));
    }
    let (before, rest) = readme
        .split_once(prefix)
        .ok_or_else(|| invalid("missing badge"))?;
    let (_, after) = rest
        .split_once('>')
        .ok_or_else(|| invalid("unterminated badge"))?;
    Ok(format!("{before}{replacement}{after}"))
}

fn count_tests(test_list: &str) -> Result<usize, io::Error> {
    let count = test_list
        .lines()
        .filter(|line| line.trim_end().ends_with(": test"))
        .count();
    if count == 0 {
        return Err(invalid("test listing contains no tests"));
    }
    Ok(count)
}

fn terminal_table(rows: &[(String, Metrics)], totals: &Metrics) -> Result<String, io::Error> {
    let mut cells = Vec::with_capacity(rows.len() + 2);
    cells.push([
        "File".to_owned(),
        "Lines".to_owned(),
        "Regions".to_owned(),
        "Functions".to_owned(),
    ]);
    for (name, metrics) in rows {
        cells.push([
            name.clone(),
            cell(&metrics.lines)?,
            cell(&metrics.regions)?,
            cell(&metrics.functions)?,
        ]);
    }
    cells.push([
        "Total".to_owned(),
        cell(&totals.lines)?,
        cell(&totals.regions)?,
        cell(&totals.functions)?,
    ]);

    let widths = std::array::from_fn::<_, 4, _>(|column| {
        cells.iter().map(|row| row[column].len()).max().unwrap_or(0)
    });
    let mut output = String::from("\nCode coverage by file\n");
    for (index, row) in cells.iter().enumerate() {
        if index == 1 || index == cells.len() - 1 {
            output.push_str(&format!(
                "{:-<a$}-+-{:-<b$}-+-{:-<c$}-+-{:-<d$}\n",
                "",
                "",
                "",
                "",
                a = widths[0],
                b = widths[1],
                c = widths[2],
                d = widths[3]
            ));
        }
        output.push_str(&format!(
            "{:<a$} | {:>b$} | {:>c$} | {:>d$}\n",
            row[0],
            row[1],
            row[2],
            row[3],
            a = widths[0],
            b = widths[1],
            c = widths[2],
            d = widths[3]
        ));
    }
    Ok(output)
}

fn render(
    readme: &str,
    coverage: Coverage,
    root: &Path,
    test_count: usize,
) -> Result<(String, String, String), io::Error> {
    let [dataset] = <[Dataset; 1]>::try_from(coverage.data)
        .map_err(|_| invalid("expected one workspace coverage dataset"))?;
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for file in dataset.files {
        let name = file
            .filename
            .strip_prefix(root)
            .map_err(|_| {
                invalid(format!(
                    "coverage file is outside repository: {:?}",
                    file.filename
                ))
            })?
            .to_str()
            .ok_or_else(|| invalid("non-UTF-8 coverage filename"))?
            .replace(std::path::MAIN_SEPARATOR, "/");
        if !seen.insert(name.clone()) {
            return Err(invalid(format!("duplicate coverage file: {name}")));
        }
        rows.push((name, file.summary));
    }
    if !seen.contains("derive/src/lib.rs") {
        return Err(invalid("workspace coverage must include derive/src/lib.rs"));
    }
    rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let terminal = terminal_table(&rows, &dataset.totals)?;

    let mut table = String::from(
        "<details>\n<summary>Code coverage by file (line, region, and function coverage)</summary>\n\n\
         | File | Lines | Regions | Functions |\n| --- | ---: | ---: | ---: |\n",
    );
    for (name, metrics) in &rows {
        table.push_str(&format!(
            "| [`{name}`]({name}) | {} | {} | {} |\n",
            cell(&metrics.lines)?,
            cell(&metrics.regions)?,
            cell(&metrics.functions)?
        ));
    }
    table.push_str(&format!(
        "| **Total** | **{}** | **{}** | **{}** |\n\n</details>",
        cell(&dataset.totals.lines)?,
        cell(&dataset.totals.regions)?,
        cell(&dataset.totals.functions)?
    ));

    let readme = replace_marked(readme, "coverage-table", &table)?;
    let line_pct = percentage(&dataset.totals.lines)?;
    let region_pct = percentage(&dataset.totals.regions)?;
    let readme = replace_marked(
        &readme,
        "coverage-summary",
        &format!(
            "The latest local coverage run measured **{line_pct} overall line coverage** and **{region_pct} region coverage**."
        ),
    )?;
    let coverage_prefix = "<img src=\"https://img.shields.io/badge/code%20coverage-";
    let coverage_badge = format!(
        "{coverage_prefix}{}-brightgreen\" alt=\"{line_pct} line coverage\">",
        line_pct.replace('%', "%25")
    );
    let readme = replace_badge(&readme, coverage_prefix, &coverage_badge)?;
    let test_prefix = "<img src=\"https://img.shields.io/badge/tests-";
    let test_badge = format!(
        "{test_prefix}{test_count}%20passing-brightgreen\" alt=\"{test_count} workspace tests passing\">"
    );
    let readme = replace_badge(&readme, test_prefix, &test_badge)?;
    Ok((readme, terminal, line_pct))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let summary = args
        .next()
        .ok_or_else(|| invalid("usage: update_readme_coverage COVERAGE_JSON TEST_LIST"))?;
    let test_list = args
        .next()
        .ok_or_else(|| invalid("usage: update_readme_coverage COVERAGE_JSON TEST_LIST"))?;
    if args.next().is_some() {
        return Err(invalid("usage: update_readme_coverage COVERAGE_JSON TEST_LIST").into());
    }
    let coverage: Coverage = serde_json::from_slice(&fs::read(summary)?)?;
    let test_count = count_tests(&fs::read_to_string(test_list)?)?;
    let readme_path = ROOT.join("README.md");
    let current = fs::read_to_string(&readme_path)?;
    let (updated, table, line_pct) = render(&current, coverage, &ROOT, test_count)?;
    if updated != current {
        fs::write(readme_path, updated)?;
    }
    println!(
        "README.md updated: Code Coverage & Tests section, tests badge ({test_count}), code coverage badge ({line_pct})."
    );
    print!("{table}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_table_summary_and_badge() {
        let readme = "<a href=\"#code-coverage--tests\"><img src=\"https://img.shields.io/badge/tests-old-brightgreen\" alt=\"old\"></a>\n\
            <a href=\"#code-coverage--tests\"><img src=\"https://img.shields.io/badge/code%20coverage-old-brightgreen\" alt=\"old\"></a>\n\
            <!-- coverage-summary:start -->\nold\n<!-- coverage-summary:end -->\n\
            <!-- coverage-table:start -->\nold\n<!-- coverage-table:end -->\n";
        let coverage: Coverage = serde_json::from_str(
            r#"{"data":[{"files":[{"filename":"/repo/derive/src/lib.rs","summary":{
                "lines":{"count":4,"covered":3},"regions":{"count":2,"covered":2},
                "functions":{"count":1,"covered":1}}}],"totals":{
                "lines":{"count":4,"covered":3},"regions":{"count":2,"covered":2},
                "functions":{"count":1,"covered":1}}}]}"#,
        )
        .unwrap();
        let (result, table, line_pct) = render(readme, coverage, Path::new("/repo"), 12).unwrap();
        assert!(result.contains("[`derive/src/lib.rs`](derive/src/lib.rs) | 75.00% (3/4)"));
        assert!(result.contains("**75.00% overall line coverage**"));
        assert!(result.contains("code%20coverage-75.00%25-brightgreen"));
        assert!(result.contains("tests-12%20passing-brightgreen"));
        assert_eq!(
            result.matches("<a href=\"#code-coverage--tests\">").count(),
            2
        );
        assert!(result.contains("alt=\"12 workspace tests passing\"></a>"));
        assert!(result.contains("alt=\"75.00% line coverage\"></a>"));
        assert_eq!(line_pct, "75.00%");
        assert!(table.contains("derive/src/lib.rs"));
        assert!(table.contains("75.00% (3/4)"));
        assert!(table.contains("Total"));
    }

    #[test]
    fn rejects_missing_markers() {
        assert!(replace_marked("", "coverage-table", "table").is_err());
    }

    #[test]
    fn counts_only_unit_and_integration_tests() {
        assert_eq!(
            count_tests("first: test\nsecond: test\n2 tests, 0 benchmarks\n").unwrap(),
            2
        );
        assert!(count_tests("0 tests, 0 benchmarks\n").is_err());
    }
}
