#!/usr/bin/env bash
# Tag the current HEAD as vX.Y.Z and push the tag, which triggers the
# GitHub release workflow (.github/workflows/release.yml).
#
# Usage:
#   scripts/release-tag.sh            # interactive prompt (TTY) or workspace version
#   scripts/release-tag.sh v0.1.2     # explicit version, no prompt
#   scripts/release-tag.sh --yes v0.1.2-rc.1  # skip the final confirmation
set -euo pipefail

step() {
    printf '==> %s\n' "$1"
}

assume_yes=0
tag=""
for arg in "$@"; do
    case "$arg" in
        -y|--yes) assume_yes=1 ;;
        *) tag="$arg" ;;
    esac
done

interactive=0
if [ -t 0 ] && [ -t 1 ]; then
    interactive=1
fi

if [ -z "$tag" ]; then
    step "Reading version from Cargo.toml [workspace.package]"
    version=$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p}' Cargo.toml | head -1)
    if [ -z "$version" ]; then
        printf 'error: could not read version from Cargo.toml\n' >&2
        exit 1
    fi
    tag="v$version"
    if [ "$interactive" = 1 ]; then
        printf 'Workspace version: %s\n' "$version"
        printf 'Tag to release [%s]: ' "$tag"
        read -r input
        if [ -n "$input" ]; then
            tag="$input"
        fi
    fi
fi

case "$tag" in
    v[0-9]*.[0-9]*.[0-9]*) ;;
    v[0-9]*.[0-9]*.[0-9]*-[0-9A-Za-z.-]*) ;;
    *)
        printf 'error: tag must look like v0.1.2 or v0.1.2-rc.1, got "%s"\n' "$tag" >&2
        exit 1
        ;;
esac

step "Preflight checks"
./scripts/check.sh

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    printf 'error: tag %s already exists\n' "$tag" >&2
    exit 1
fi

if [ -n "$(git status --porcelain)" ]; then
    printf 'error: working tree is not clean; commit or stash first\n' >&2
    exit 1
fi

branch=$(git rev-parse --abbrev-ref HEAD)
if [ "$branch" != "main" ]; then
    printf 'warning: releasing from %s, not main\n' "$branch" >&2
fi

if [ "$interactive" = 1 ] && [ "$assume_yes" = 0 ]; then
    printf 'About to tag %s on branch %s and push, starting the release.\n' "$tag" "$branch"
    printf 'Continue? [y/N]: '
    read -r answer
    case "$answer" in
        y|Y|yes|Yes) ;;
        *)
            printf 'aborted\n'
            exit 1
            ;;
    esac
fi

step "Tagging $tag"
git tag -a "$tag" -m "Release $tag"

step "Pushing $tag"
git push origin "$tag"

printf 'done: release workflow started for %s\n' "$tag"
