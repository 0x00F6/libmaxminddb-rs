#!/usr/bin/env bash
# Usage: bash scripts/release.sh X.Y.Z [--force]
# Synchronizes versions, then pushes main, publishes both crates, and creates the GitHub Release.
# Commit synchronized files and rerun before publication; use -h or --help for details.
set -euo pipefail

cd "$(dirname "$0")/.."
CARGO="${CARGO:-cargo}"

# Use colors in interactive terminals while respecting NO_COLOR.
if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
    RED=$'\033[31m'
    GREEN=$'\033[32m'
    YELLOW=$'\033[33m'
    CYAN=$'\033[36m'
    RESET=$'\033[0m'
else
    RED=''
    GREEN=''
    YELLOW=''
    CYAN=''
    RESET=''
fi

info() { printf '%sℹ️  %s%s\n' "$CYAN" "$*" "$RESET"; }
step() { printf '\n%s🚀 %s%s\n' "$CYAN" "$*" "$RESET"; }
success() { printf '%s✅ %s%s\n' "$GREEN" "$*" "$RESET"; }
warning() { printf '%s⚠️  %s%s\n' "$YELLOW" "$*" "$RESET" >&2; }
die() {
    printf '%s❌ %s%s\n' "$RED" "$*" "$RESET" >&2
    exit 1
}

show_help() {
    printf '%s🚀 Usage:%s bash scripts/release.sh X.Y.Z [--force]\n' "$CYAN" "$RESET"
    printf '   make release X.Y.Z [-- --force]\n\n'
    printf 'Synchronizes crate versions, tracked Cargo.lock files, and installation examples.\n'
    printf 'Commit any updated files and rerun to push main, publish both crates, tag, and create the GitHub Release.\n\n'
    printf '%sOptions:%s\n' "$CYAN" "$RESET"
    printf '  -h, --help  Show this help and exit.\n'
    printf '  --force     Replace an existing tag or Release and skip crates.io versions already published.\n'
}

parse_arguments() {
    if [[ "$#" -eq 1 && ( "$1" == "-h" || "$1" == "--help" ) ]]; then
        show_help
        exit 0
    fi
    force=false
    if [[ "$#" -lt 1 || "$#" -gt 2 || ! "${1:-}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        die "Usage: make release X.Y.Z [-- --force]"
    fi
    requested_version="$1"
    if [[ "${2:-}" == "--force" && "$#" -eq 2 ]]; then
        force=true
        warning "Force mode can replace origin/main and the version tag, retarget its GitHub Release, and skip published crates."
    elif [[ "$#" -ne 1 ]]; then
        die "Usage: make release X.Y.Z [-- --force]"
    fi
}

check_tools_and_branch() {
    step "Checking release tools and branch"
    local command
    for command in cargo git gh sed grep; do
        command -v "$command" >/dev/null 2>&1 || die "Required command is missing: $command. Install it and rerun make release."
    done
    command -v "$CARGO" >/dev/null 2>&1 || die "Required Cargo command is missing: $CARGO. Install Rust with rustup or set CARGO to its executable path."

    [[ "$(git branch --show-current)" == "main" ]] || \
        die "Run releases from main (current: $(git branch --show-current || printf 'detached HEAD')). Switch with 'git switch main', then rerun."
    success "Release tools are available on main."
}

sync_version() {
    step "Synchronizing version $requested_version in TOML, lock, and Markdown files"
    local -a files=(Cargo.toml derive/Cargo.toml README.md README.crates.md)
    local -a lock_files
    local file
    local -a changed_files=()
    mapfile -t lock_files < <(git ls-files '*Cargo.lock')
    [[ "${#lock_files[@]}" -gt 0 ]] || die "No tracked Cargo.lock files were found. Restore or commit the workspace lockfile, then rerun."

    # Check all expected references before writing any file.
    for file in "${files[@]}" "${lock_files[@]}"; do
        [[ -f "$file" ]] || die "Missing version file: $file. Restore it from Git, then rerun."
    done
    check_one_version Cargo.toml '^version = "[0-9]+\.[0-9]+\.[0-9]+"$'
    check_one_version Cargo.toml '^libmaxminddb-rs-derive = \{ version = "[0-9]+\.[0-9]+\.[0-9]+", path = "derive", optional = true \}$'
    check_one_version derive/Cargo.toml '^version = "[0-9]+\.[0-9]+\.[0-9]+"$'
    check_one_version README.md '^libmaxminddb-rs = "[0-9]+\.[0-9]+(\.[0-9]+)?"$'
    check_one_version README.crates.md '^libmaxminddb-rs = "[0-9]+\.[0-9]+(\.[0-9]+)?"$'
    for file in "${lock_files[@]}"; do
        if grep -Fq 'name = "libmaxminddb-rs"' "$file"; then
            check_lock_version "$file" libmaxminddb-rs
        fi
        if grep -Fq 'name = "libmaxminddb-rs-derive"' "$file"; then
            check_lock_version "$file" libmaxminddb-rs-derive
        fi
    done

    update_version_line Cargo.toml '^version = "[0-9]+\.[0-9]+\.[0-9]+"$' "version = \"$requested_version\""
    update_version_line Cargo.toml '^libmaxminddb-rs-derive = \{ version = "[0-9]+\.[0-9]+\.[0-9]+", path = "derive", optional = true \}$' \
        "libmaxminddb-rs-derive = { version = \"$requested_version\", path = \"derive\", optional = true }"
    update_version_line derive/Cargo.toml '^version = "[0-9]+\.[0-9]+\.[0-9]+"$' "version = \"$requested_version\""
    for file in "${lock_files[@]}"; do
        if grep -Fq 'name = "libmaxminddb-rs"' "$file"; then
            update_lock_version "$file" libmaxminddb-rs
        fi
        if grep -Fq 'name = "libmaxminddb-rs-derive"' "$file"; then
            update_lock_version "$file" libmaxminddb-rs-derive
        fi
    done
    update_version_line README.md '^libmaxminddb-rs = "[0-9]+\.[0-9]+(\.[0-9]+)?"$' "libmaxminddb-rs = \"$requested_version\""
    update_version_line README.crates.md '^libmaxminddb-rs = "[0-9]+\.[0-9]+(\.[0-9]+)?"$' "libmaxminddb-rs = \"$requested_version\""

    if [[ "${#changed_files[@]}" -gt 0 ]]; then
        for file in "${changed_files[@]}"; do
            info "Updated $file"
        done
        die "Version references were updated to $requested_version. Commit the changes, then rerun make release $requested_version."
    fi
    success "Version references match $requested_version."
}

check_one_version() {
    local count
    count="$(grep -Ec "$2" "$1" || true)"
    [[ "$count" -eq 1 ]] || die "Expected one version reference in $1, found $count. Fix its crate version entry, then rerun."
}

check_lock_version() {
    local lines
    lines="$(sed -n "/^name = \"$2\"$/{N;p;}" "$1")"
    [[ "$lines" =~ ^name\ =\ \"$2\"$'\n'version\ =\ \"[0-9]+\.[0-9]+\.[0-9]+\"$ ]] || \
        die "Expected one version entry for $2 in $1. Regenerate or repair this lockfile, then rerun."
}

record_change() {
    local file
    for file in "${changed_files[@]}"; do
        [[ "$file" == "$1" ]] && return
    done
    changed_files+=("$1")
}

update_version_line() {
    if ! grep -Fxq "$3" "$1"; then
        sed -E -i "s|$2|$3|" "$1"
        record_change "$1"
    fi
}

update_lock_version() {
    local current
    current="$(sed -n "/^name = \"$2\"$/{n;p;}" "$1")"
    if [[ "$current" != "version = \"$requested_version\"" ]]; then
        sed -E -i "/^name = \"$2\"$/{n;s|^version = \"[0-9]+\.[0-9]+\.[0-9]+\"$|version = \"$requested_version\"|;}" "$1"
        record_change "$1"
    fi
}

check_checkout_and_auth() {
    step "Checking working tree and GitHub authentication"
    git diff --quiet || die "Working-tree changes remain. Commit or stash them, then rerun make release $requested_version."
    git diff --cached --quiet || die "Staged changes remain. Commit them, then rerun make release $requested_version."
    [[ -z "$(git ls-files --others --exclude-standard)" ]] || \
        die "Untracked files remain. Commit or remove them, then rerun make release $requested_version."
    gh auth status >/dev/null 2>&1 || die "GitHub CLI is not authenticated; run 'gh auth login'."
    success "Checkout is clean and GitHub CLI is authenticated."
}

load_release_metadata() {
    step "Reading package version and repository"
    version="$(sed -nE 's/^version = "([0-9]+\.[0-9]+\.[0-9]+)"$/\1/p' Cargo.toml)"
    repository_url="$(sed -nE 's/^repository = "([^"]+)"$/\1/p' Cargo.toml)"
    [[ "$version" == "$requested_version" ]] || \
        die "Cargo.toml version $version does not match requested version $requested_version. Correct Cargo.toml, then rerun."

    repository="${repository_url#https://github.com/}"
    [[ "$repository" != "$repository_url" ]] || die "Unsupported repository URL: $repository_url. Set Cargo.toml repository to an HTTPS GitHub URL."
    [[ "$repository" == "${GITHUB_REPO:?Set GITHUB_REPO to the GitHub owner/repository}" ]] || \
        die "GITHUB_REPO does not match Cargo.toml repository: $repository_url. Set GITHUB_REPO to the matching owner/repository."

    tag="v${version}"
    release_commit="$(git rev-parse HEAD)"
    info "Package: $version · Tag: $tag · Repository: $repository"
}

configure_origin() {
    step "Checking origin remote"
    if git remote get-url origin >/dev/null 2>&1; then
        if [[ "$(git remote get-url origin)" != "$repository_url" ]]; then
            info "Updating origin URL to $repository_url"
            git remote set-url origin "$repository_url" || die "Could not update origin URL. Check write access to .git/config."
        fi
    else
        info "Adding origin URL $repository_url"
        git remote add origin "$repository_url" || die "Could not add origin remote. Check write access to .git/config."
    fi
    [[ "$(git remote get-url origin)" == "$repository_url" ]] || \
        die "Origin must point to $repository_url. Inspect 'git remote -v' and correct the origin URL."
    success "Origin points to the package repository."
}

check_existing_release_objects() {
    step "Checking existing tag and GitHub Release"
    remote_tag="$(git ls-remote --tags origin "refs/tags/$tag")" || \
        die "Could not check remote tag $tag. Check network access and 'git remote -v', then rerun."

    if [[ "$force" == false ]]; then
        git rev-parse --verify --quiet "refs/tags/$tag" >/dev/null && \
            die "Tag already exists locally: $tag (use --force to replace it)."
        [[ -z "$remote_tag" ]] || die "Tag already exists on origin: $tag (use --force to replace it)."
    fi

    release_exists=false
    if gh release view "$tag" --repo "$repository" >/dev/null 2>&1; then
        [[ "$force" == true ]] || die "GitHub Release already exists: $tag (use --force to update it)."
        release_exists=true
        warning "GitHub Release $tag already exists; force mode will update it and keep its assets."
    fi
    success "Release checks passed."
}

publish_main() {
    step "Publishing and verifying main"
    if [[ "$force" == true ]]; then
        local observed_main expected_main
        observed_main="$(git ls-remote --heads origin refs/heads/main)" || \
            die "Could not inspect origin/main before the forced push. Check network access and the origin URL."
        expected_main="${observed_main%%[[:space:]]*}"
        if [[ -n "$expected_main" ]]; then
            warning "Replacing origin/main at $expected_main with $release_commit."
            # Lease the exact remote tip we inspected so concurrent updates are not overwritten.
            git push -u "--force-with-lease=refs/heads/main:$expected_main" origin main || \
                die "Could not force-push main to origin. Check push permission and the remote branch, then rerun."
        else
            info "origin/main does not exist yet; creating it from the current main branch."
            git push -u origin main || die "Could not push main to origin. Check push permission and 'git remote -v'."
        fi
    else
        git push -u origin main || die "Could not push main to origin. Check push permission and 'git remote -v'."
    fi
    local remote_main
    remote_main="$(git ls-remote --heads origin refs/heads/main)" || \
        die "Could not verify main on origin. Check network access and 'git ls-remote origin main'."
    [[ "${remote_main%%[[:space:]]*}" == "$release_commit" ]] || \
        die "Origin main does not point to release commit $release_commit. Inspect 'git ls-remote origin main' before retrying."
    success "Origin main points to $release_commit."
}

publish_crate() {
    local crate="$1"
    shift

    step "Publishing $crate to crates.io"
    local output
    if output="$("$CARGO" publish "$@" 2>&1)"; then
        [[ -z "$output" ]] || printf '%s\n' "$output"
        success "Published $crate."
        return 0
    fi

    [[ -z "$output" ]] || printf '%s\n' "$output" >&2
    if grep -Fq 'already exists on crates.io index' <<<"$output"; then
        if [[ "$force" == true ]]; then
            warning "$crate is already published; skipping this immutable version."
            return 0
        fi
        die "$crate is already published. Use 'make release $version -- --force' to skip it and continue."
    fi
    die "Publishing $crate failed. Review Cargo's output above, fix the cause, and rerun the release."
}

publish_crates() {
    # The main crate depends on the derive crate's registry release.
    publish_crate libmaxminddb-rs-derive --manifest-path derive/Cargo.toml
    publish_crate libmaxminddb-rs --package libmaxminddb-rs
    success "Both crates.io publications completed."
}

publish_tag() {
    step "Creating and pushing $tag"
    if [[ "$force" == true ]]; then
        git tag -fa "$tag" -m "Release $tag" || die "Could not update local tag $tag. Check write access to .git/refs/tags."
        # --force deliberately replaces an existing origin tag with this release commit.
        git push --force origin "$tag" || die "Could not replace remote tag $tag. Check push permission before retrying."
    else
        git tag -a "$tag" -m "Release $tag" || die "Could not create local tag $tag. Check write access to .git/refs/tags."
        git push origin "$tag" || die "Could not push tag $tag. Check push permission before retrying."
    fi

    local pushed_tag_commit
    pushed_tag_commit="$(git ls-remote --tags origin "refs/tags/$tag^{}")" || \
        die "Could not verify remote tag $tag. Check network access and 'git ls-remote --tags origin'."
    [[ "${pushed_tag_commit%%[[:space:]]*}" == "$release_commit" ]] || \
        die "Remote tag $tag does not point to release commit $release_commit. Inspect the remote tag before retrying."
    success "Remote tag $tag points to $release_commit."
}

publish_github_release() {
    step "Publishing GitHub Release $tag"
    if [[ "$release_exists" == true ]]; then
        # Retarget the existing release without deleting its release assets.
        gh release edit "$tag" --repo "$repository" --verify-tag \
            --target "$release_commit" --title "$tag" || die "Could not update GitHub Release $tag. Check 'gh auth status' and repository permissions."
        success "Updated existing GitHub Release $tag."
    else
        gh release create "$tag" --repo "$repository" --verify-tag \
            --target "$release_commit" --generate-notes --title "$tag" || \
            die "Could not create GitHub Release $tag. Check 'gh auth status' and repository permissions."
        success "Created GitHub Release $tag."
    fi
}

main() {
    parse_arguments "$@"
    check_tools_and_branch
    sync_version
    check_checkout_and_auth
    load_release_metadata
    configure_origin
    check_existing_release_objects

    step "Validating crate packages"
    make publish-check || die "Package validation failed. Fix the failing check above, commit the repair, and rerun make release $requested_version."
    success "Package validation passed."

    publish_main
    publish_crates
    publish_tag
    publish_github_release
    success "Release $tag completed."
}

main "$@"
