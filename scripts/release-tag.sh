#!/usr/bin/env bash
# Tag the current HEAD and push the tag, which triggers the GitHub release
# workflow (.github/workflows/release.yml).
#
# Usage:
#   scripts/release-tag.sh     # interactive menu (release kind, then version bump)
#   scripts/release-tag.sh v0.1.2          # explicit version, no menu
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

# --- interactive arrow-key menu -------------------------------------------------
# pick <title> <line1> <line2> ... -> prints the chosen 1-based index.
# Arrow keys / j k move, Enter confirms, Ctrl+C aborts.
pick() {
    local title=$1; shift
    local options=("$@")
    local count=${#options[@]}
    local sel=0

    hide_cursor=$'\e[?25l'
    show_cursor=$'\e[?25h'
    printf '%s' "$hide_cursor"
    trap 'printf "%s" "$show_cursor"; exit 130' INT

    while true; do
        # redraw: move cursor back up over ALL lines drawn last time
        # (title + one line per option = count + 1)
        if [ "${rendered:-0}" = 1 ]; then
            printf '\e[%dA' "$((count + 1))"
        fi
        printf '%s\e[0K\n' "$title"
        local i=0
        for opt in "${options[@]}"; do
            if [ "$i" = "$sel" ]; then
                printf '  \e[36m❯ %s\e[0m\e[0K\n' "$opt"
            else
                printf '    %s\e[0K\n' "$opt"
            fi
            i=$((i + 1))
        done
        rendered=1

        read -rsn1 key || exit 130
        case "$key" in
            $'\e')
                read -rsn2 -t 0.1 rest || true
                case "$rest" in
                    '[A'|'[D') sel=$(( (sel + count - 1) % count )) ;; # up/left
                    '[B'|'[C') sel=$(( (sel + 1) % count )) ;;         # down/right
                esac
                ;;
            'k') sel=$(( (sel + count - 1) % count )) ;;
            'j') sel=$(( (sel + 1) % count )) ;;
            '') break ;; # Enter
        esac
    done
    printf '%s' "$show_cursor"
    trap - INT
    return $((sel + 1))
}

if [ -z "$tag" ]; then
    step "Reading version from Cargo.toml [workspace.package]"
    version=$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p}' Cargo.toml | head -1)
    if [ -z "$version" ]; then
        printf 'error: could not read version from Cargo.toml\n' >&2
        exit 1
    fi
    tag="v$version"

    if [ "$interactive" = 1 ]; then
        # Parse current version into major.minor.patch
        IFS='.' read -r major minor patch <<< "$version"

        # 1. Release kind: stable or pre-release
        set +e
        pick "Release kind (↑/↓ to move, Enter to confirm)" \
            "stable" \
            "pre-release (e.g. rc.1, alpha.1)"
        kind_result=$?
        set -e
        [ "$kind_result" -ge 1 ] 2>/dev/null || exit 130
        kind="stable"
        [ "$kind_result" = 2 ] && kind="pre"

        # 2. Version selection with preview
        #    For pre-release, pick the prefix (rc/alpha/beta) FIRST so the
        #    base-version menu can show the FULL final tag as preview.
        next="" next_base=""
        while true; do
            if [ "$kind" = "pre" ]; then
                set +e
                pick "Pre-release tag (↑/↓, Enter)" \
                    "rc" \
                    "alpha" \
                    "beta"
                pre_result=$?
                set -e
                [ "$pre_result" -ge 1 ] 2>/dev/null || exit 130
                case "$pre_result" in
                    1) prefix="rc" ;;
                    2) prefix="alpha" ;;
                    3) prefix="beta" ;;
                esac

                set +e
                pick "Base version for $prefix (current: $version)" \
                    "keep current  ->  v$major.$minor.$patch-$prefix.N" \
                    "patch bump    ->  v$major.$minor.$((patch + 1))-$prefix.N" \
                    "minor bump    ->  v$major.$((minor + 1)).0-$prefix.N" \
                    "major bump    ->  v$((major + 1)).0.0-$prefix.N"
                base_result=$?
                set -e
                [ "$base_result" -ge 1 ] 2>/dev/null || exit 130
                case "$base_result" in
                    1) next_base="v$major.$minor.$patch" ;;
                    2) next_base="v$major.$minor.$((patch + 1))" ;;
                    3) next_base="v$major.$((minor + 1)).0" ;;
                    4) next_base="v$((major + 1)).0.0" ;;
                esac

                # scan existing tags of the same base+prefix, suggest next number
                pre_num=1
                last_num=""
                last_num=$(git tag --list "${next_base}-${prefix}.*" \
                    | sed -n "s/^${next_base}-${prefix}\.\([0-9][0-9]*\)$/\1/p" \
                    | sort -n | tail -1)
                if [ -n "$last_num" ]; then
                    pre_num=$((last_num + 1))
                fi
                printf 'Pre-release number for %s-%s.<n> [%s]: ' "$next_base" "$prefix" "$pre_num"
                read -r input_num
                input_num=${input_num:-$pre_num}
                case "$input_num" in
                    ''|*[!0-9]*)
                        printf 'error: pre-release number must be a number, got "%s"\n' "$input_num" >&2
                        exit 1
                        ;;
                esac
                next="$next_base-$prefix.$input_num"
            else
                set +e
                pick "Bump version (current: $version)" \
                    "keep current  ->  v$major.$minor.$patch" \
                    "patch bump    ->  v$major.$minor.$((patch + 1))" \
                    "minor bump    ->  v$major.$((minor + 1)).0" \
                    "major bump    ->  v$((major + 1)).0.0"
                bump_result=$?
                set -e
                [ "$bump_result" -ge 1 ] 2>/dev/null || exit 130
                case "$bump_result" in
                    1) next="v$major.$minor.$patch" ;;
                    2) next="v$major.$minor.$((patch + 1))" ;;
                    3) next="v$major.$((minor + 1)).0" ;;
                    4) next="v$((major + 1)).0.0" ;;
                esac
            fi

            printf 'Selected tag: %s\n' "$next"
            printf 'Confirm? [Y/n]: '
            read -r confirm
            case "$confirm" in
                n|N|no|No) continue ;;
                *) tag="$next"; break ;;
            esac
        done
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
    # Tag already exists — offer to retag (delete local + remote first), which
    # lets a failed release be re-published with the same version.
    retag=0
    if [ "$interactive" = 1 ]; then
        printf 'Tag %s already exists. Delete it and re-release? [y/N]: ' "$tag"
        read -r answer
        case "$answer" in
            y|Y|yes|Yes) retag=1 ;;
            *)
                printf 'aborted\n'
                exit 1
                ;;
        esac
    else
        printf 'error: tag %s already exists\n' "$tag" >&2
        exit 1
    fi
    if [ "$retag" = 1 ]; then
        step "Deleting existing tag $tag (local + remote)"
        git tag -d "$tag"
        git push origin ":refs/tags/$tag"
        printf 'note: the old GitHub release for %s (if any) becomes draft; delete or re-publish it manually\n' "$tag"
    fi
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
