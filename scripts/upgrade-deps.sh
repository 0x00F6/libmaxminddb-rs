#!/usr/bin/env bash
# Usage: bash scripts/upgrade-deps.sh [--help]
# Upgrade dependencies in every tracked Cargo manifest, run tests, and restore
# the original manifests and lockfiles if an upgrade or test fails.
set -euo pipefail

cd "$(dirname "$0")/.."
CARGO="${CARGO:-cargo}"

if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
    BOLD=$'\033[1m'
    CYAN=$'\033[36m'
    GREEN=$'\033[32m'
    RED=$'\033[31m'
    RESET=$'\033[0m'
else
    BOLD=''
    CYAN=''
    GREEN=''
    RED=''
    RESET=''
fi

info() { printf '%sℹ️  %s%s\n' "$CYAN" "$*" "$RESET"; }
step() { printf '\n%s%s🚀 %s%s\n' "$BOLD" "$CYAN" "$*" "$RESET"; }
success() { printf '%s%s✅ %s%s\n' "$BOLD" "$GREEN" "$*" "$RESET"; }
error() { printf '%s%s❌ %s%s\n' "$BOLD" "$RED" "$*" "$RESET" >&2; }

show_help() {
    printf '%s%s📦 Usage:%s make upgrade-deps\n' "$BOLD" "$CYAN" "$RESET"
    printf '   bash scripts/upgrade-deps.sh [--help]\n\n'
    printf 'Upgrade declared and locked dependencies in every tracked Cargo project.\n'
    printf 'Run the full test suite and remaining project tests. Restore files on failure.\n'
}

if [[ "$#" -eq 1 && ( "$1" == '-h' || "$1" == '--help' ) ]]; then
    show_help
    exit 0
elif [[ "$#" -ne 0 ]]; then
    error 'Unexpected argument. Use --help for usage.'
    exit 2
fi

command -v git >/dev/null 2>&1 || { error 'git is missing. Install Git and rerun make upgrade-deps.'; exit 2; }
command -v "$CARGO" >/dev/null 2>&1 || { error "Cargo command is missing: $CARGO. Install Rust with rustup or set CARGO to its executable path."; exit 2; }
if ! "$CARGO" upgrade --version >/dev/null 2>&1; then
    error 'cargo-upgrade is required. Install it with: cargo install cargo-edit'
    exit 2
fi

mapfile -d '' -t manifests < <(git ls-files -z -- '*Cargo.toml')
mapfile -d '' -t saved_files < <(git ls-files -z -- '*Cargo.toml' '*Cargo.lock' 'README.md')
[[ "${#manifests[@]}" -gt 0 && "${#saved_files[@]}" -gt 0 ]] || {
    error 'No tracked Cargo manifests were found. Run this command from a Git checkout containing Cargo.toml.'
    exit 2
}

mkdir -p target
log_file="$PWD/target/upgrade-deps.log"
snapshot="$(mktemp -d)"
restored=false
for file in "${saved_files[@]}"; do
    mkdir -p "$snapshot/$(dirname "$file")"
    cp -p -- "$file" "$snapshot/$file"
done

finish() {
    local status=$?
    local restore_failed=false
    trap - EXIT INT TERM
    if [[ "$status" -ne 0 && "$restored" == false ]]; then
        for file in "${saved_files[@]}"; do
            if ! cp -p -- "$snapshot/$file" "$file"; then
                error "Could not restore $file. Copy it manually from $snapshot/$file."
                restore_failed=true
            fi
        done
        if [[ "$restore_failed" == false ]]; then
            restored=true
            error 'Upgrade or tests failed. Restored Cargo manifests, lockfiles, and README.md.'
        else
            error "Some files could not be restored. Backup retained at $snapshot."
        fi
        info "Full log: $log_file"
    fi
    if [[ "$restore_failed" == false ]]; then rm -rf -- "$snapshot"; fi
    exit "$status"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

: > "$log_file"
step "Upgrading dependencies in ${#manifests[@]} Cargo manifests"
for manifest in "${manifests[@]}"; do
    info "$manifest"
    "$CARGO" upgrade --manifest-path "$manifest" --incompatible allow --pinned allow \
        --exclude libmaxminddb-rs --exclude libmaxminddb-rs-derive 2>&1 | tee -a "$log_file"
done

step 'Updating lockfiles'
for manifest in "${manifests[@]}"; do
    "$CARGO" update --manifest-path "$manifest" 2>&1 | tee -a "$log_file"
done

step 'Running the full test suite'
make tests 2>&1 | tee -a "$log_file"

# These standalone projects are outside the workspace tests target.
for manifest in "${manifests[@]}"; do
    case "$manifest" in
        Cargo.toml|derive/Cargo.toml|scripts/Cargo.toml|tools/rust-competitor-bench/Cargo.toml)
            continue ;;
    esac
    step "Testing $manifest"
    "$CARGO" test --manifest-path "$manifest" 2>&1 | tee -a "$log_file"
done

success 'All dependency upgrades and tests passed.'
info "Full log: $log_file"
