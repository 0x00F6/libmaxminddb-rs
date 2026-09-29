#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

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

parse_arguments() {
    force=false
    if [[ "${1:-}" == "--force" && "$#" -eq 1 ]]; then
        force=true
        warning "Force mode can replace origin/main, the version tag, and retarget its GitHub Release."
    elif [[ "$#" -ne 0 ]]; then
        printf 'Usage: %s [--force]\n' "$0" >&2
        exit 2
    fi
}

check_prerequisites_and_checkout() {
    step "Checking tools, branch, and working tree"
    local command
    for command in cargo git python3 gh; do
        command -v "$command" >/dev/null 2>&1 || die "Required command is missing: $command"
    done

    [[ "$(git branch --show-current)" == "main" ]] || \
        die "Run releases from main (current: $(git branch --show-current || printf 'detached HEAD'))."
    git diff --quiet || die "Commit working-tree changes before releasing."
    git diff --cached --quiet || die "Commit staged changes before releasing."
    [[ -z "$(git ls-files --others --exclude-standard)" ]] || \
        die "Remove or commit untracked files before releasing."
    gh auth status >/dev/null 2>&1 || die "GitHub CLI is not authenticated; run 'gh auth login'."
    success "Prerequisites passed; checkout is clean on main."
}

load_release_metadata() {
    step "Reading package version and repository"
    local metadata
    metadata="$(cargo metadata --no-deps --format-version 1)" || die "cargo metadata failed."
    mapfile -t package_info < <(python3 -c '
import json, sys
metadata = json.load(sys.stdin)
package = next(p for p in metadata["packages"] if p["name"] == "libmaxminddb-rs")
print(package["version"])
print(package["repository"])
' <<<"$metadata")

    version="${package_info[0]:-}"
    repository_url="${package_info[1]:-}"
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
        die "Cargo.toml version must use the X.Y.Z format (found: $version)."

    repository="${repository_url#https://github.com/}"
    [[ "$repository" != "$repository_url" ]] || die "Unsupported repository URL: $repository_url"
    [[ "$repository" == "${GITHUB_REPO:?Set GITHUB_REPO to the GitHub owner/repository}" ]] || \
        die "GITHUB_REPO does not match Cargo.toml repository: $repository_url"

    tag="v${version}"
    release_commit="$(git rev-parse HEAD)"
    info "Package: $version · Tag: $tag · Repository: $repository"
}

configure_origin() {
    step "Checking origin remote"
    if git remote get-url origin >/dev/null 2>&1; then
        if [[ "$(git remote get-url origin)" != "$repository_url" ]]; then
            info "Updating origin URL to $repository_url"
            git remote set-url origin "$repository_url" || die "Could not update origin URL."
        fi
    else
        info "Adding origin URL $repository_url"
        git remote add origin "$repository_url" || die "Could not add origin remote."
    fi
    [[ "$(git remote get-url origin)" == "$repository_url" ]] || \
        die "Origin must point to $repository_url."
    success "Origin points to the package repository."
}

check_existing_release_objects() {
    step "Checking existing tag and GitHub Release"
    remote_tag="$(git ls-remote --tags origin "refs/tags/$tag")" || \
        die "Could not check remote tag $tag."

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
            die "Could not inspect origin/main before the forced push."
        expected_main="${observed_main%%[[:space:]]*}"
        if [[ -n "$expected_main" ]]; then
            warning "Replacing origin/main at $expected_main with $release_commit."
            # Lease the exact remote tip we inspected so concurrent updates are not overwritten.
            git push -u "--force-with-lease=refs/heads/main:$expected_main" origin main || \
                die "Could not force-push main to origin."
        else
            info "origin/main does not exist yet; creating it from the current main branch."
            git push -u origin main || die "Could not push main to origin."
        fi
    else
        git push -u origin main || die "Could not push main to origin."
    fi
    local remote_main
    remote_main="$(git ls-remote --heads origin refs/heads/main)" || \
        die "Could not verify main on origin."
    [[ "${remote_main%%[[:space:]]*}" == "$release_commit" ]] || \
        die "Origin main does not point to release commit $release_commit."
    success "Origin main points to $release_commit."
}

publish_tag() {
    step "Creating and pushing $tag"
    if [[ "$force" == true ]]; then
        git tag -fa "$tag" -m "Release $tag" || die "Could not update local tag $tag."
        if [[ -n "$remote_tag" ]]; then
            # The lease protects a remote tag changed by another publisher after our check.
            git push "--force-with-lease=refs/tags/$tag:${remote_tag%%[[:space:]]*}" origin "$tag" || \
                die "Could not replace remote tag $tag."
        else
            git push origin "$tag" || die "Could not push tag $tag."
        fi
    else
        git tag -a "$tag" -m "Release $tag" || die "Could not create local tag $tag."
        git push origin "$tag" || die "Could not push tag $tag."
    fi

    local pushed_tag
    pushed_tag="$(git ls-remote --tags origin "refs/tags/$tag")" || \
        die "Could not verify remote tag $tag."
    [[ -n "$pushed_tag" ]] || die "Tag $tag is missing from origin."
    success "Tag $tag is present on origin."
}

publish_github_release() {
    step "Publishing GitHub Release $tag"
    if [[ "$release_exists" == true ]]; then
        # Retarget the existing release without deleting its release assets.
        gh release edit "$tag" --repo "$repository" --verify-tag \
            --target "$release_commit" --title "$tag" || die "Could not update GitHub Release $tag."
        success "Updated existing GitHub Release $tag."
    else
        gh release create "$tag" --repo "$repository" --verify-tag \
            --target "$release_commit" --generate-notes --title "$tag" || \
            die "Could not create GitHub Release $tag."
        success "Created GitHub Release $tag."
    fi
}

main() {
    parse_arguments "$@"
    check_prerequisites_and_checkout
    load_release_metadata
    configure_origin
    check_existing_release_objects

    step "Validating crate packages"
    make publish-check || die "Package validation failed."
    success "Package validation passed."

    publish_main
    publish_tag
    publish_github_release
    success "Release $tag completed."
}

main "$@"
