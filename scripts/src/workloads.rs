use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::ROOT;

pub fn run_command(cmd: &[&str], cwd: Option<&Path>) -> Result<(), String> {
    let mut command = Command::new(cmd[0]);
    if cmd.len() > 1 {
        command.args(&cmd[1..]);
    }
    if let Some(c) = cwd {
        command.current_dir(c);
    }
    let output = command
        .output()
        .map_err(|e| format!("Failed to exec {:?}: {e}", cmd))?;
    if !output.status.success() {
        return Err(format!(
            "Command {:?} failed with status {}:\n{}",
            cmd,
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

pub fn prepare_dataset(dataset: &Path) -> Result<(), String> {
    if let Some(p) = dataset.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let regen = std::env::var("BENCH_REGENERATE_DATA").as_deref() == Ok("1");
    if dataset.exists() && !regen {
        return Ok(());
    }

    let manifest = ROOT.join("tools/gen-bench-data/Cargo.toml");
    let cmd = [
        "cargo",
        "run",
        "--quiet",
        "--release",
        "--manifest-path",
        &manifest.to_string_lossy(),
        "--",
        &dataset.to_string_lossy(),
    ];
    run_command(&cmd, Some(&*ROOT))?;
    Ok(())
}

pub fn prepare_workloads(
    workload_dir: &Path,
    sizes: &[usize],
    patterns: &[&str],
    families: &[&str],
) -> Result<(), String> {
    fs::create_dir_all(workload_dir).map_err(|e| e.to_string())?;
    let manifest_json = workload_dir.join("workloads.json");
    let max_size = sizes.iter().copied().max().unwrap_or(1_000_000);
    let regen = std::env::var("BENCH_REGENERATE_DATA").as_deref() == Ok("1");

    if manifest_json.exists() && !regen {
        let mut valid = true;
        for fam in families {
            for pat in patterns {
                let fp = workload_dir.join(format!("{fam}-{pat}.txt"));
                if !fp.exists() {
                    valid = false;
                    break;
                }
            }
            if !valid {
                break;
            }
        }
        if valid {
            return Ok(());
        }
    }

    let patterns_csv = patterns.join(",");
    let families_csv = families.join(",");
    let max_size_str = max_size.to_string();

    let binary_path = ROOT.join("tools/gen-bench-workloads/target/release/gen-bench-workloads");
    if binary_path.exists() {
        let cmd = [
            &binary_path.to_string_lossy(),
            "--workload-dir",
            &workload_dir.to_string_lossy(),
            "--count",
            &max_size_str,
            "--patterns",
            &patterns_csv,
            "--families",
            &families_csv,
        ];
        run_command(&cmd, Some(&*ROOT))?;
    } else {
        let manifest_path = ROOT.join("tools/gen-bench-workloads/Cargo.toml");
        let cmd = [
            "cargo",
            "run",
            "--quiet",
            "--release",
            "--manifest-path",
            &manifest_path.to_string_lossy(),
            "--",
            "--workload-dir",
            &workload_dir.to_string_lossy(),
            "--count",
            &max_size_str,
            "--patterns",
            &patterns_csv,
            "--families",
            &families_csv,
        ];
        run_command(&cmd, Some(&*ROOT))?;
    }

    Ok(())
}

pub fn build_scaling_tool() -> Result<PathBuf, String> {
    let gen_tool = ROOT.join("tools/gen-scaling-bench-data/target/release/gen-scaling-bench-data");
    // Cargo verifies source freshness even when the generator binary already exists.
    let manifest = ROOT.join("tools/gen-scaling-bench-data/Cargo.toml");
    let cmd = [
        "cargo",
        "build",
        "--quiet",
        "--release",
        "--manifest-path",
        &manifest.to_string_lossy(),
    ];
    run_command(&cmd, Some(&*ROOT))?;

    if !gen_tool.exists() {
        return Err(format!("Scaling tool not found at {}", gen_tool.display()));
    }
    Ok(gen_tool)
}

pub fn generate_scaling_workload(
    gen_tool: &Path,
    workload_file: &Path,
    family: &str,
    count: usize,
    seed: &str,
) -> Result<(), String> {
    if let Some(p) = workload_file.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let regen = std::env::var("BENCH_REGENERATE_DATA").as_deref() == Ok("1");
    if workload_file.exists() && !regen {
        return Ok(());
    }

    let ip_ver = if family.contains("6") { "6" } else { "4" };
    let count_str = count.to_string();
    let cmd = [
        &gen_tool.to_string_lossy(),
        "workload",
        &workload_file.to_string_lossy(),
        ip_ver,
        &count_str,
        seed,
    ];
    run_command(&cmd, Some(&*ROOT))?;
    Ok(())
}

pub fn generate_scaling_db(
    gen_tool: &Path,
    db_path: &Path,
    family: &str,
    size: usize,
    seed: &str,
    batch_size: usize,
) -> Result<(), String> {
    if let Some(p) = db_path.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let regen = std::env::var("BENCH_REGENERATE_DATA").as_deref() == Ok("1");
    if db_path.exists() && !regen {
        return Ok(());
    }

    let ip_ver = if family.contains("6") { "6" } else { "4" };
    let size_str = size.to_string();
    let batch_str = batch_size.to_string();
    let cmd = [
        &gen_tool.to_string_lossy(),
        "db",
        &db_path.to_string_lossy(),
        &size_str,
        ip_ver,
        seed,
        &batch_str,
    ];
    run_command(&cmd, Some(&*ROOT))?;
    Ok(())
}
