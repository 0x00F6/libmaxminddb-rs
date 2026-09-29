use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use crate::config::{BIN_DIR, BUILD_DIR, DEPS_DIR, IMPLEMENTATIONS, ROOT, WRITER_IMPLEMENTATIONS};

pub fn get_git_revision() -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&*ROOT)
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".into())
}

pub fn detect_versions() -> HashMap<String, String> {
    let mut versions = HashMap::new();
    versions.insert("libmaxminddb-rs".into(), "0.1.0".into());
    versions.insert("maxminddb-rust".into(), "0.32.0".into());
    versions.insert("geoip2-rs".into(), "0.1.8".into());
    let c_ver = std::env::var("LIBMAXMINDDB_BENCH_VERSION").unwrap_or_else(|_| "1.14.1".into());
    versions.insert("libmaxminddb".into(), c_ver);
    versions.insert("maxminddb-golang".into(), "v2.6.0".into());
    versions.insert("mmdbwriter".into(), "v1.2.0".into());

    let cargo_lock = ROOT.join("tools/rust-competitor-bench/Cargo.lock");
    if let Ok(text) = fs::read_to_string(&cargo_lock) {
        for name in &["maxminddb", "geoip2"] {
            for line in text.lines() {
                if line.starts_with(&format!("name = \"{name}\"")) {
                    // next lines have version
                }
            }
        }
    }

    let go_mod = ROOT.join("tools/go-bench/go.mod");
    if let Ok(text) = fs::read_to_string(&go_mod) {
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("github.com/oschwald/maxminddb-golang/v2 ") {
                if let Some(ver) = line.split_whitespace().nth(1) {
                    versions.insert("maxminddb-golang".into(), ver.to_string());
                }
            } else if line.starts_with("github.com/maxmind/mmdbwriter ") {
                if let Some(ver) = line.split_whitespace().nth(1) {
                    versions.insert("mmdbwriter".into(), ver.to_string());
                }
            }
        }
    }

    versions
}

pub fn detect_compilers() -> HashMap<String, String> {
    let mut compilers = HashMap::new();
    if let Ok(out) = Command::new("rustc").arg("--version").output() {
        if out.status.success() {
            compilers.insert(
                "rustc".into(),
                String::from_utf8_lossy(&out.stdout).trim().into(),
            );
        }
    }
    let cc_bin = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    if let Ok(out) = Command::new(&cc_bin).arg("--version").output() {
        if out.status.success() {
            let first_line = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            compilers.insert("c_compiler".into(), first_line);
        }
    }
    if let Ok(out) = Command::new("go").arg("version").output() {
        if out.status.success() {
            compilers.insert(
                "go".into(),
                String::from_utf8_lossy(&out.stdout).trim().into(),
            );
        }
    }
    compilers
}

pub fn cpu_model() -> String {
    if let Ok(content) = fs::read_to_string("/proc/cpuinfo") {
        for line in content.lines() {
            if line.starts_with("model name") {
                if let Some((_, model)) = line.split_once(':') {
                    return model.trim().to_string();
                }
            }
        }
    }
    "x86_64".into()
}

pub fn get_os_info() -> String {
    let os_type = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    if let Ok(release) = std::fs::read_to_string("/etc/os-release") {
        for line in release.lines() {
            if line.starts_with("PRETTY_NAME=") {
                if let Some((_, name)) = line.split_once('=') {
                    return format!("{os_type} ({arch}) — {}", name.trim_matches('"'));
                }
            }
        }
    }

    format!("{os_type} {arch}")
}

pub fn get_system_info() -> String {
    let mut info = Vec::new();

    // CPU info
    if let Ok(cpus) = std::fs::read_to_string("/proc/cpuinfo") {
        let mut core_set = std::collections::HashSet::new();
        let mut current_phys = 0_u32;
        let mut logical = 0;
        let mut cpu_cores = 0;

        for line in cpus.lines() {
            if line.starts_with("processor") {
                logical += 1;
            } else if line.starts_with("physical id") {
                if let Some((_, id)) = line.split_once(':') {
                    current_phys = id.trim().parse::<u32>().unwrap_or(0);
                }
            } else if line.starts_with("core id") {
                if let Some((_, id)) = line.split_once(':') {
                    let core_id = id.trim().parse::<u32>().unwrap_or(0);
                    core_set.insert((current_phys, core_id));
                }
            } else if line.starts_with("cpu cores") {
                if let Some((_, c)) = line.split_once(':') {
                    let c = c.trim().parse::<u32>().unwrap_or(0);
                    if c > cpu_cores {
                        cpu_cores = c;
                    }
                }
            }
        }
        let physical = if !core_set.is_empty() {
            core_set.len()
        } else if cpu_cores > 0 {
            cpu_cores as usize
        } else {
            logical
        };
        info.push(format!(
            "{} cores ({} physical, {} logical)",
            logical, physical, logical
        ));

        // Memory with unit
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                if line.starts_with("MemTotal:") {
                    if let Some((_, rest)) = line.split_once(':') {
                        let parts: Vec<&str> = rest.trim().split_whitespace().collect();
                        if !parts.is_empty() {
                            let kb = parts[0].parse::<u64>().unwrap_or(0);
                            let gib = kb as f64 / (1024.0 * 1024.0);
                            info.push(format!("{:.1} GiB RAM", gib));
                        }
                    }
                }
            }
        }
    }

    // Fallback
    if info.is_empty() {
        info.push(format!(
            "{} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }

    info.join(" — ")
}

pub fn build_rust_binary(name: &str, rustflags: &str) -> Result<PathBuf, String> {
    let feature = if name == "libmaxminddb-rs" {
        "ours"
    } else {
        name
    };
    let target_dir = BUILD_DIR.join(name);
    let destination = BIN_DIR.join(name);

    // Let Cargo validate source/dependency freshness for every Rust implementation.

    let mut cmd = Command::new("cargo");
    cmd.args([
        "build",
        "--release",
        "--locked",
        "--manifest-path",
        &ROOT
            .join("tools/rust-competitor-bench/Cargo.toml")
            .to_string_lossy(),
        "--no-default-features",
        "--features",
        feature,
    ]);
    cmd.current_dir(&*ROOT);
    cmd.env("RUSTFLAGS", rustflags);
    cmd.env("CARGO_TARGET_DIR", &target_dir);

    let output = cmd
        .output()
        .map_err(|e| format!("cargo build failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Build failed for {name}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let source = target_dir.join("release/rust-competitor-bench");
    if !source.exists() {
        return Err(format!("Binary not found at {}", source.display()));
    }

    fs::create_dir_all(&*BIN_DIR).map_err(|e| e.to_string())?;
    fs::copy(&source, &destination).map_err(|e| e.to_string())?;
    let _ = fs::set_permissions(&destination, fs::Permissions::from_mode(0o755));
    Ok(destination)
}

pub fn build_rust_writer_binary(name: &str, rustflags: &str) -> Result<PathBuf, String> {
    let target_dir = BUILD_DIR.join(name);
    let destination = BIN_DIR.join(name);

    if std::env::var("BENCH_SKIP_REBUILD").as_deref() == Ok("1") && destination.exists() {
        return Ok(destination);
    }

    let mut cmd = Command::new("cargo");
    cmd.args([
        "build",
        "--release",
        "--manifest-path",
        &ROOT
            .join("tools/rust-writer-bench/Cargo.toml")
            .to_string_lossy(),
    ]);
    cmd.current_dir(&*ROOT);
    if !rustflags.is_empty() {
        cmd.env("RUSTFLAGS", rustflags);
    }
    cmd.env("CARGO_TARGET_DIR", &target_dir);

    let output = cmd
        .output()
        .map_err(|e| format!("cargo build failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Build failed for {name}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let source = target_dir.join("release/rust-writer-bench");
    if !source.exists() {
        return Err(format!("Binary not found at {}", source.display()));
    }

    fs::create_dir_all(&*BIN_DIR).map_err(|e| e.to_string())?;
    fs::copy(&source, &destination).map_err(|e| e.to_string())?;
    let _ = fs::set_permissions(&destination, fs::Permissions::from_mode(0o755));
    Ok(destination)
}

pub fn build_c_binary(name: &str) -> Result<PathBuf, String> {
    let destination = BIN_DIR.join(name);

    let c_source = ROOT.join("tools/libmaxminddb-bench/bench.c");
    let c_version = std::env::var("LIBMAXMINDDB_BENCH_VERSION").unwrap_or_else(|_| "1.14.1".into());
    let c_dir = DEPS_DIR.join(format!("libmaxminddb-{c_version}"));
    let install_dir = c_dir.join("install");

    // Ensure C library is built
    if !install_dir.join("lib/libmaxminddb.a").exists() {
        fs::create_dir_all(&*DEPS_DIR).map_err(|e| e.to_string())?;
        let tar_path = DEPS_DIR.join(format!("libmaxminddb-{c_version}.tar.gz"));
        if !tar_path.exists() {
            let url = format!(
                "https://github.com/maxmind/libmaxminddb/releases/download/{c_version}/libmaxminddb-{c_version}.tar.gz"
            );
            let status = Command::new("curl")
                .args(["-fsSL", "-o", &tar_path.to_string_lossy(), &url])
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("Failed to download libmaxminddb from {url}"));
            }
        }
        let status = Command::new("tar")
            .args([
                "-xzf",
                &tar_path.to_string_lossy(),
                "-C",
                &DEPS_DIR.to_string_lossy(),
            ])
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("Failed to unpack libmaxminddb".into());
        }

        let configure_status = Command::new("./configure")
            .args([
                "--prefix",
                &install_dir.to_string_lossy(),
                "--enable-static",
                "--disable-shared",
                "CFLAGS=-O3 -march=native -fPIC",
            ])
            .current_dir(&c_dir)
            .status()
            .map_err(|e| e.to_string())?;
        if !configure_status.success() {
            return Err("Failed to configure libmaxminddb".into());
        }

        let make_status = Command::new("make")
            .args(["-j", "install"])
            .current_dir(&c_dir)
            .status()
            .map_err(|e| e.to_string())?;
        if !make_status.success() {
            return Err("Failed to build/install libmaxminddb".into());
        }
    }

    fs::create_dir_all(&*BIN_DIR).map_err(|e| e.to_string())?;
    let cc_bin = std::env::var("CC").unwrap_or_else(|_| "gcc".into());
    let compile_status = Command::new(&cc_bin)
        .args([
            "-O3",
            "-march=native",
            "-I",
            &install_dir.join("include").to_string_lossy(),
            &c_source.to_string_lossy(),
            &install_dir.join("lib/libmaxminddb.a").to_string_lossy(),
            "-o",
            &destination.to_string_lossy(),
            "-lpthread",
            "-lm",
            "-Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc",
        ])
        .status()
        .map_err(|e| e.to_string())?;

    if !compile_status.success() {
        return Err(format!("Failed to compile C harness {name}"));
    }
    let _ = fs::set_permissions(&destination, fs::Permissions::from_mode(0o755));
    Ok(destination)
}

pub fn build_go_binary(name: &str, pkg_rel_path: &str) -> Result<PathBuf, String> {
    let destination = BIN_DIR.join(name);

    let go_bench_dir = ROOT.join("tools/go-bench");
    fs::create_dir_all(&*BIN_DIR).map_err(|e| e.to_string())?;

    let mut cmd = Command::new("go");
    cmd.args([
        "build",
        "-ldflags=-s -w",
        "-trimpath",
        "-o",
        &destination.to_string_lossy(),
        pkg_rel_path,
    ]);
    cmd.current_dir(&go_bench_dir);

    let output = cmd.output().map_err(|e| format!("go build failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Failed to build Go binary {name}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let _ = fs::set_permissions(&destination, fs::Permissions::from_mode(0o755));
    Ok(destination)
}

pub fn build_all_binaries() -> Result<HashMap<String, PathBuf>, String> {
    let mut binaries = HashMap::new();
    let rustflags = "-C target-cpu=native -C opt-level=3";

    for &name in IMPLEMENTATIONS {
        match name {
            "libmaxminddb-rs" | "maxminddb-rust" | "geoip2-rs" => {
                let bin = build_rust_binary(name, rustflags)?;
                binaries.insert(name.to_string(), bin);
            }
            "libmaxminddb" => {
                let bin = build_c_binary(name)?;
                binaries.insert(name.to_string(), bin);
            }
            "maxminddb-golang" => {
                let bin = build_go_binary(name, "./cmd/reader")?;
                binaries.insert(name.to_string(), bin);
            }
            _ => {}
        }
    }

    binaries.extend(build_writer_binaries()?);

    Ok(binaries)
}

pub fn build_writer_binaries() -> Result<HashMap<String, PathBuf>, String> {
    let mut binaries = HashMap::new();
    let rustflags = "-C target-cpu=native -C opt-level=3";
    for &name in WRITER_IMPLEMENTATIONS {
        match name {
            "libmaxminddb-rs" => {
                let bin = build_rust_writer_binary("libmaxminddb-rs-writer", rustflags)?;
                binaries.insert("libmaxminddb-rs-writer".to_string(), bin);
            }
            "mmdbwriter" => {
                let bin = build_go_binary(name, "./cmd/writer")?;
                binaries.insert(name.to_string(), bin);
            }
            _ => {}
        }
    }

    Ok(binaries)
}
