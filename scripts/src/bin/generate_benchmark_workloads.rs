use bench_runner::config::{
    DATASET_PATH, DEFAULT_FAMILIES, DEFAULT_LOOKUP_PATTERNS, DEFAULT_LOOKUP_SIZES, WORKLOAD_DIR,
};
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

    println!("✅ Workloads prepared successfully");
    Ok(())
}
