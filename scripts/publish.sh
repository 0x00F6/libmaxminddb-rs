#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

RED=$'\033[31m'
GREEN=$'\033[32m'
YELLOW=$'\033[33m'
CYAN=$'\033[36m'
RESET=$'\033[0m'
if [[ ! -t 1 || -n "${NO_COLOR:-}" ]]; then
    RED=''
    GREEN=''
    YELLOW=''
    CYAN=''
    RESET=''
fi

info() { printf '%sℹ️  %s%s\n' "$CYAN" "$*" "$RESET"; }
success() { printf '%s✅ %s%s\n' "$GREEN" "$*" "$RESET"; }
warning() { printf '%s⚠️  %s%s\n' "$YELLOW" "$*" "$RESET" >&2; }
die() {
    printf '%s❌ %s%s\n' "$RED" "$*" "$RESET" >&2
    exit 1
}

force=false
if [[ "${1:-}" == "--force" && "$#" -eq 1 ]]; then
    force=true
elif [[ "$#" -ne 0 ]]; then
    printf 'Usage: %s [--force]\n' "$0" >&2
    exit 2
fi

command -v "$CARGO" >/dev/null 2>&1 || die "Required command is missing: $CARGO"

if [[ "$force" == true ]]; then
    warning "Force mode skips versions Cargo reports as already published; crates.io versions cannot be overwritten."
fi

publish_crate() {
    local crate="$1"
    shift

    info "Publishing $crate."
    local output
    if output="$("$CARGO" publish "$@" 2>&1)"; then
        [[ -z "$output" ]] || printf '%s\n' "$output"
        success "Published $crate."
        return 0
    fi

    [[ -z "$output" ]] || printf '%s\n' "$output" >&2
    if [[ "$force" == true ]] && grep -Fq 'already exists on crates.io index' <<<"$output"; then
        warning "$crate is already published; skipping this immutable version."
        return 0
    fi

    if [[ "$force" == false ]] && grep -Fq 'already exists on crates.io index' <<<"$output"; then
        die "$crate is already published. Use 'make publish -- --force' to skip it and continue."
    fi
    die "Publishing $crate failed."
}

# Publish the derive crate first because the main crate depends on its registry release.
publish_crate libmaxminddb-rs-derive --manifest-path derive/Cargo.toml
publish_crate libmaxminddb-rs --package libmaxminddb-rs

success "Crates.io publication checks completed."
