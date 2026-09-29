//! Isolated reader RSS runs and strict selection of comparable report points.
use crate::{builder, config::*, runner::run_case, workloads::build_scaling_tool};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub const LOOKUPS: usize = 1_000_000;
pub const WARMUP: usize = 1_000;
pub const REPEATS: usize = 3;

fn sha256(path: &Path) -> Result<String, String> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("Cannot hash {}", path.display()));
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .map(String::from)
        .ok_or("Missing SHA-256".into())
}

fn unavailable(name: &str, size: usize, error: &str) -> Value {
    json!({"implementation": name, "scenario": "memory_rss", "database_size": size,
        "entries": size, "family": "ipv4", "pattern": "mixed_50_50", "opening_mode": "mmap",
        "measurement_protocol": MEMORY_PROTOCOL, "failed": true, "error": error})
}

/// Prepare all input files in separate processes before launching any measured reader.
pub fn run(
    binaries: &HashMap<String, PathBuf>,
    skip_competitors: bool,
    sizes: &[usize],
) -> Result<Vec<Value>, String> {
    let generator = build_scaling_tool();
    let mut prepared = Vec::new();
    for &size in sizes {
        let dir = DATA_DIR.join(MEMORY_PROTOCOL).join(size.to_string());
        let preparation = (|| -> Result<(String, String), String> {
            let generator = generator.as_ref().map_err(Clone::clone)?;
            let output = Command::new(generator)
                .arg("memory")
                .arg(&dir)
                .arg(size.to_string())
                .current_dir(&*ROOT)
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(format!(
                    "Fixture preparation failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let manifest: Value = serde_json::from_slice(
                &fs::read(dir.join("manifest.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            if manifest["protocol"] != MEMORY_PROTOCOL || manifest["entries"] != size {
                return Err("Invalid RSS fixture manifest".into());
            }
            Ok((
                sha256(&dir.join("database.mmdb"))?,
                sha256(&dir.join("memory-queries.bin"))?,
            ))
        })();
        println!(
            "RSS fixture {size}: {}",
            if preparation.is_ok() {
                "ready"
            } else {
                "unavailable"
            }
        );
        prepared.push((size, dir, preparation));
    }
    let revision = builder::get_git_revision();
    let versions = builder::detect_versions();
    let mut rows = Vec::new();
    for (size, dir, preparation) in prepared {
        let mut reference_checksum = None;
        for &name in MEMORY_IMPLEMENTATIONS {
            if skip_competitors && name != "libmaxminddb-rs" {
                rows.push(unavailable(name, size, "Competitor disabled for this run"));
                continue;
            }
            let inputs = preparation
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|hashes| {
                    binaries
                        .get(name)
                        .map(|binary| (binary, hashes))
                        .ok_or_else(|| "Harness unavailable (build failed or missing)".into())
                });
            let (binary, (database_hash, workload_hash)) = match inputs {
                Ok(inputs) => inputs,
                Err(error) => {
                    rows.push(unavailable(name, size, &error));
                    continue;
                }
            };
            for repeat in 0..REPEATS {
                let mut row = run_case(
                    binary,
                    name,
                    &revision,
                    &dir.join("database.mmdb"),
                    "memory_rss",
                    Some("ipv4"),
                    Some("random"),
                    LOOKUPS,
                    Some(1),
                    WARMUP,
                    1,
                    1,
                    repeat,
                    Some("memory_rss"),
                    Some(&dir),
                );
                row["database_size"] = json!(size);
                row["entries"] = json!(size);
                row["dataset_sha256"] = json!(database_hash);
                row["workload_sha256"] = json!(workload_hash);
                row["version"] = json!(versions.get(name));
                if row["failed"] != true {
                    let validation = validate(&row);
                    let checksum = row["checksum"].as_u64();
                    let mismatch = reference_checksum.is_some() && reference_checksum != checksum;
                    if let Err(error) = validation {
                        row["failed"] = json!(true);
                        row["error"] = json!(error);
                    } else if mismatch {
                        row["failed"] = json!(true);
                        row["error"] = json!("Different lookup results across libraries");
                    } else {
                        reference_checksum = checksum;
                    }
                }
                // A failed legacy/unsupported child must still identify the requested protocol.
                if row["failed"] == true {
                    row["measurement_protocol"] = json!(MEMORY_PROTOCOL);
                }
                println!(
                    "  RSS {name:18} / {size:7} / repeat {}: open {}, peak {}",
                    repeat + 1,
                    cell(&[row.clone()], name, size, "rss_after_open_bytes")
                        .map(|v| format!("{:.3} MiB", v / 1048576.0))
                        .unwrap_or_else(|e| format!("unavailable: {e}")),
                    cell(&[row.clone()], name, size, "rss_peak_bytes")
                        .map(|v| format!("{:.3} MiB", v / 1048576.0))
                        .unwrap_or_else(|e| format!("unavailable: {e}"))
                );
                rows.push(row);
            }
        }
    }
    Ok(rows)
}

fn validate(row: &Value) -> Result<(), String> {
    if row["measurement_protocol"] != MEMORY_PROTOCOL
        || row["opening_mode"] != "mmap"
        || row["rss_method"] != "linux-vmrss-vmhwm-reset-after-open"
        || row["lookups"] != LOOKUPS
        || row["warmup_ops"] != WARMUP
        || row["hits"] != LOOKUPS / 2
        || row["misses"] != LOOKUPS / 2
        || row["checksum"].as_u64().is_none()
        || row["pid"].as_u64().is_none()
    {
        return Err("Incompatible/incomplete RSS measurement or incorrect hit/miss counts".into());
    }
    for key in [
        "rss_before_open_bytes",
        "rss_after_open_bytes",
        "rss_after_bytes",
    ] {
        if row[key].as_u64().is_none_or(|value| value == 0) {
            return Err(format!("Unavailable {key}"));
        }
    }
    Ok(())
}

/// Never silently choose a historical/mmap-incompatible point or convert null to zero.
pub fn cell(rows: &[Value], name: &str, size: usize, metric: &str) -> Result<f64, String> {
    let all_rows = rows;
    let candidates: Vec<_> = rows
        .iter()
        .filter(|r| {
            r["scenario"] == "memory_rss"
                && r["implementation"] == name
                && r["database_size"] == size
        })
        .collect();
    let rows: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|r| r["measurement_protocol"] == MEMORY_PROTOCOL)
        .collect();
    if rows.is_empty() {
        return Err(if candidates.is_empty() {
            "Not measured"
        } else {
            "Incompatible measurement protocol"
        }
        .into());
    }
    if rows.len() != 1 {
        return Err("Multiple incompatible result groups".into());
    }
    let row = rows[0];
    if row["failed"] == true || row["unsupported"] == true {
        return Err(row["error"]
            .as_str()
            .unwrap_or("Failed or unsupported measurement")
            .into());
    }
    if row["opening_mode"] != "mmap" {
        return Err("Incompatible opening mode".into());
    }
    // A curve must not silently combine libraries measured on different inputs.
    for key in ["dataset_sha256", "workload_sha256"] {
        if row[key].as_str().is_none_or(str::is_empty) {
            return Err(format!("Missing {key} identity"));
        }
        if all_rows.iter().any(|peer| {
            peer["scenario"] == "memory_rss"
                && peer["database_size"] == size
                && peer["measurement_protocol"] == MEMORY_PROTOCOL
                && peer["failed"] != true
                && peer["unsupported"] != true
                && peer[key] != row[key]
        }) {
            return Err(format!("Incompatible {key} across libraries"));
        }
    }
    row[metric]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| {
            row["peak_unavailable_reason"]
                .as_str()
                .unwrap_or("Measurement unavailable")
                .into()
        })
}

/// Refresh only RSS rows; all other locally measured scenarios are preserved.
pub fn run_suite() -> Result<(), String> {
    let mut binaries = HashMap::new();
    fs::create_dir_all(&*BIN_DIR).map_err(|e| e.to_string())?;
    for &name in MEMORY_IMPLEMENTATIONS {
        let result = if name == "libmaxminddb" {
            builder::build_c_binary(name)
        } else {
            builder::build_rust_binary(name, "-C target-cpu=native -C opt-level=3")
        };
        match result {
            Ok(binary) => {
                binaries.insert(name.into(), binary);
            }
            Err(error) => eprintln!("{name} RSS unavailable: {error}"),
        }
    }
    let rows = run(&binaries, false, SCALING_DATABASE_SIZES)?;
    let previous = match fs::read_to_string(&*RESULTS_PATH) {
        Ok(previous) => previous,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.to_string()),
    };
    let mut updated = String::new();
    for line in previous.lines() {
        let row: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        if row["scenario"] != "memory_rss" {
            updated.push_str(line);
            updated.push('\n');
        }
    }
    for row in &rows {
        updated.push_str(&serde_json::to_string(row).map_err(|e| e.to_string())?);
        updated.push('\n');
    }
    fs::create_dir_all(RESULTS_PATH.parent().unwrap()).map_err(|e| e.to_string())?;
    let backup = RESULTS_PATH.with_extension("before-memory-rss-v2.jsonl");
    if !backup.exists() {
        fs::write(backup, &previous).map_err(|e| e.to_string())?;
    }
    let pending = RESULTS_PATH.with_extension(format!("memory-{}.jsonl", std::process::id()));
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)
        .map_err(|e| e.to_string())?;
    file.write_all(updated.as_bytes())
        .map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(pending, &*RESULTS_PATH).map_err(|e| e.to_string())?;
    Ok(())
}
