use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

fn parse_csv(path: &Path) -> Result<Vec<HashMap<String, String>>, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut lines = text.lines();
    let header_line = lines.next().ok_or_else(|| "Empty CSV".to_string())?;
    let headers: Vec<&str> = header_line.split(',').map(|s| s.trim()).collect();

    let mut rows = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        let mut row = HashMap::new();
        for (i, &h) in headers.iter().enumerate() {
            if i < cols.len() {
                row.insert(h.to_string(), cols[i].to_string());
            }
        }
        rows.push(row);
    }
    Ok(rows)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        println!("Usage: compare_benchmarks <baseline.csv> <current.csv> [threshold_pct]");
        return Ok(());
    }

    let baseline_path = Path::new(&args[1]);
    let current_path = Path::new(&args[2]);
    let threshold: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(5.0);

    let baseline_rows = parse_csv(baseline_path)?;
    let current_rows = parse_csv(current_path)?;

    println!(
        "📊 Comparing {} vs {} (threshold: {}%)...",
        baseline_path.display(),
        current_path.display(),
        threshold
    );

    let mut baseline_map = HashMap::new();
    for r in &baseline_rows {
        let key = format!(
            "{}:{}:{}:{}",
            r.get("implementation").unwrap_or(&"".into()),
            r.get("scenario").unwrap_or(&"".into()),
            r.get("family").unwrap_or(&"".into()),
            r.get("workload_size").unwrap_or(&"".into())
        );
        baseline_map.insert(key, r);
    }

    let mut regressions = 0;
    let mut improvements = 0;

    for r in &current_rows {
        let key = format!(
            "{}:{}:{}:{}",
            r.get("implementation").unwrap_or(&"".into()),
            r.get("scenario").unwrap_or(&"".into()),
            r.get("family").unwrap_or(&"".into()),
            r.get("workload_size").unwrap_or(&"".into())
        );

        if let Some(base) = baseline_map.get(&key) {
            // Compare p99_ns
            if let (Some(cur_p99_str), Some(base_p99_str)) = (r.get("p99_ns"), base.get("p99_ns")) {
                if let (Ok(cur), Ok(base)) =
                    (cur_p99_str.parse::<f64>(), base_p99_str.parse::<f64>())
                {
                    if base > 0.0 {
                        let pct = ((cur - base) / base) * 100.0;
                        if pct > threshold {
                            println!(
                                "   ⚠️ REGRESSION in {key} p99_ns: {base:.1} -> {cur:.1} (+{pct:.1}%)"
                            );
                            regressions += 1;
                        } else if pct < -threshold {
                            println!(
                                "   ✅ IMPROVEMENT in {key} p99_ns: {base:.1} -> {cur:.1} ({pct:.1}%)"
                            );
                            improvements += 1;
                        }
                    }
                }
            }
        }
    }

    println!("\nComparison summary: {regressions} regressions, {improvements} improvements");
    Ok(())
}
