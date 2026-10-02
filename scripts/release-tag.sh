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
        printf '%s\n' "$title"
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

        # 2. Version bump with preview
        while true; do
            set +e
            pick "Bump version (current: $version)" \
                "patch  ->  v$major.$minor.$((patch + 1))" \
                "minor  ->  v$major.$((minor + 1)).0" \
                "major  ->  v$((major + 1)).0.0"
            bump_result=$?
            set -e
            [ "$bump_result" -ge 1 ] 2>/dev/null || exit 130

            case "$bump_result" in
                1) next="v$major.$minor.$((patch + 1))" ;;
                2) next="v$major.$((minor + 1)).0" ;;
                3) next="v$((major + 1)).0.0" ;;
            esac

            if [ "$kind" = "pre" ]; then
                set +e
                pick "Pre-release tag for $next" \
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
                printf 'Pre-release number for %s-<n>.1? [1]: ' "$prefix"
                read -r pre_num
                pre_num=${pre_num:-1}
                case "$pre_num" in
                    ''|*[!0-9]*)
                        printf 'error: pre-release number must be a number, got "%s"\n' "$pre_num" >&2
                        exit 1
                        ;;
                esac
                next="$next-$prefix.$pre_num"
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
