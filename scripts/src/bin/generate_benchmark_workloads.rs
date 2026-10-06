use bench_runner::config::{
    DATASET_PATH, DEFAULT_FAMILIES, DEFAULT_LOOKUP_PATTERNS, DEFAULT_LOOKUP_SIZES, WORKLOAD_DIR,
};
use bench_runner::runner::prepare_ipv6_absent_fixture;
use bench_runner::workloads::{prepare_dataset, prepare_workloads};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "🗄️ Preparing deterministic dataset: {}...",
        DATASET_PATH.display()
    );
    prepare_dataset(&DATASET_PATH)?;

    println!(
        "📥 Preparing workload addresses: {}...",
        WORKLOAD_DIR.display()
    );
    prepare_workloads(
        &WORKLOAD_DIR,
        DEFAULT_LOOKUP_SIZES,
        DEFAULT_LOOKUP_PATTERNS,
        DEFAULT_FAMILIES,
    )?;

    println!("📥 Preparing and verifying the million-route IPv6 absent workload...");
    prepare_ipv6_absent_fixture()?;

    println!("✅ Workloads prepared successfully");
    Ok(())
}
