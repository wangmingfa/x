#!/usr/bin/env bash
# Generate CHANGELOG.md and the GitHub release notes from the commit history.
#
# One generator serves both, so the repository's changelog and the published
# release notes cannot drift apart in wording or in which commits they name.
#
# Usage:
#   scripts/changelog.sh                       # whole history, markdown on stdout
#   scripts/changelog.sh -o CHANGELOG.md       # write it to a file
#   scripts/changelog.sh --heading v0.1.2      # name the pending section (release-tag.sh does this)
#   scripts/changelog.sh --release v0.1.2      # only that tag, as release notes with an asset footer
#
# -o applies to both documents.
#
# Grouping rules, and why they are what they are:
#
# * A subject that starts with a recognised `type:` (or `type(scope):`,
#   `type!:`, `type(scope)!:`) is filed under that type and the prefix is
#   dropped, because the heading already states it.
# * A subject with an unknown prefix (`hotfix: …`) or with no prefix at all
#   (`C4 add a per-host timing baseline`, `P4 打包分发：…`) is listed under
#   "Other changes" **verbatim**. Rewriting or dropping those would hide a
#   shipped change, and a changelog whose omissions are invisible is worse than
#   one with an untidy last section.
# * Merge commits are skipped: their subjects carry nothing worth reading.
# * `!` marks a breaking change and stays visible in the entry.
# * A `docs: regenerate CHANGELOG …` commit is skipped, because it is this
#   script's own output; see commits_for.

set -euo pipefail

KNOWN_TYPES='feat|fix|perf|refactor|docs|style|test|build|ci|chore|revert'
TYPE_ORDER='feat fix perf refactor docs style test build ci chore revert'
TAB=$(printf '\t')

usage() {
    cat <<'USAGE'
Usage:
  scripts/changelog.sh                       whole history, markdown on stdout
  scripts/changelog.sh -o FILE               write the same document to FILE
  scripts/changelog.sh --heading NAME        name the pending section NAME
  scripts/changelog.sh --release TAG         only TAG, as GitHub release notes
  scripts/changelog.sh --help                this text
USAGE
}

title_for() {
    case "$1" in
        feat) echo "Features" ;;
        fix) echo "Fixes" ;;
        perf) echo "Performance" ;;
        refactor) echo "Refactoring" ;;
        docs) echo "Documentation" ;;
        style) echo "Style" ;;
        test) echo "Tests" ;;
        build) echo "Build" ;;
        ci) echo "CI" ;;
        chore) echo "Chores" ;;
        revert) echo "Reverts" ;;
    esac
}

output=""
pending_heading=""
release_tag=""
while [ $# -gt 0 ]; do
    case "$1" in
        -o | --output)
            output=${2:-}
            [ -n "$output" ] || { echo "error: $1 needs a file" >&2; exit 1; }
            shift 2
            ;;
        --heading)
            pending_heading=${2:-}
            [ -n "$pending_heading" ] || { echo "error: --heading needs a name" >&2; exit 1; }
            shift 2
            ;;
        --release)
            release_tag=${2:-}
            [ -n "$release_tag" ] || { echo "error: --release needs a tag" >&2; exit 1; }
            shift 2
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown argument \"$1\"" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if ! git rev-parse --git-dir >/dev/null 2>&1; then
    echo "error: run this from inside the repository" >&2
    exit 1
fi

# Fail before producing anything rather than after writing half a document.
if [ -n "$release_tag" ] &&
    ! git rev-parse -q --verify "refs/tags/$release_tag" >/dev/null; then
    echo "error: tag $release_tag not found; --release needs an existing tag" >&2
    echo "       (release-tag.sh creates the tag; the workflow runs this after it)" >&2
    exit 1
fi

# "<sha><tab><subject>" for a range, newest first, merge commits left out.
#
# The commit that carries this generated file is excluded too. Without that
# filter, committing the changelog would add an entry to the changelog, which
# changes the file, which makes the release script stop again: no run would
# ever settle. release-tag.sh prints the exact subject this filter matches.
commits_for() {
    git log --no-merges --format="%h${TAB}%s" \
        --invert-grep --grep='^docs: regenerate CHANGELOG' "$1"
}

tag_date() {
    git log -1 --format=%ad --date=short "$1"
}

# Tags in this line of history except `$2` itself, newest version first. The
# first one is the release `$2` was built on top of.
previous_tags() {
    git tag --list 'v*' --sort=-v:refname --merged "$1" 2>/dev/null |
        grep -v -x -F "$2" || true
}

# "- subject (sha)", with the type prefix removed because the heading states it.
# A subject no rule recognises is printed exactly as git stored it.
emit_entries() {
    while IFS=$TAB read -r sha subject; do
        [ -n "$sha" ] || continue
        rest=$(printf '%s' "$subject" |
            sed -E "s/^(${KNOWN_TYPES})(\(([^)]*)\))?(!)?:[[:space:]]*//")
        scope=$(printf '%s' "$subject" | sed -nE "s/^(${KNOWN_TYPES})\(([^)]*)\).*$/\2/p")
        breaking=""
        if printf '%s' "$subject" | grep -qE "^(${KNOWN_TYPES})(\([^)]*\))?!:"; then
            breaking="**breaking** "
        fi
        if [ -n "$scope" ]; then
            printf -- '- %s**%s:** %s (%s)\n' "$breaking" "$scope" "$rest" "$sha"
        else
            printf -- '- %s%s (%s)\n' "$breaking" "$rest" "$sha"
        fi
    done
}

# Every group with entries, in TYPE_ORDER, then "Other changes".
emit_groups() {
    list=$1
    [ -n "$list" ] || return 0

    for type in $TYPE_ORDER; do
        matched=$(printf '%s\n' "$list" |
            grep -E "^[0-9a-f]+${TAB}${type}(\([^)]*\))?!?: " 2>/dev/null || true)
        [ -n "$matched" ] || continue
        printf '### %s\n\n' "$(title_for "$type")"
        printf '%s\n' "$matched" | emit_entries
        printf '\n'
    done

    other=$(printf '%s\n' "$list" |
        grep -vE "^[0-9a-f]+${TAB}(${KNOWN_TYPES})(\([^)]*\))?!?: " 2>/dev/null || true)
    if [ -n "$other" ]; then
        printf '### Other changes\n\n'
        printf 'Subjects with no recognised `type:` prefix, kept verbatim.\n\n'
        printf '%s\n' "$other" | emit_entries
        printf '\n'
    fi
}

emit_section() {
    heading=$1
    date=$2
    list=$3
    if [ -n "$date" ]; then
        printf '## %s (%s)\n\n' "$heading" "$date"
    else
        printf '## %s\n\n' "$heading"
    fi
    if [ -z "$list" ]; then
        printf 'No commits recorded for this range.\n\n'
        return 0
    fi
    emit_groups "$list"
}

# Range a tag released: everything since the previous tag, or, for the first
# release, everything up to it.
tag_range() {
    previous=$(previous_tags "$1" "$1" | head -1)
    if [ -n "$previous" ]; then
        printf '%s..%s' "$previous" "$1"
    else
        printf '%s' "$1"
    fi
}

# --- --release: one tag, as the body of a GitHub release ----------------------
# The release title on GitHub is already the tag, so the body does not repeat it
# as a heading. It states which range of commits the release contains instead:
# that is the one fact a reader cannot recover from the tag name.
emit_release() {
    previous=$(previous_tags "$release_tag" "$release_tag" | head -1)
    if [ -n "$previous" ]; then
        printf 'Changes since `%s`, tagged %s.\n\n' "$previous" "$(tag_date "$release_tag")"
    else
        printf 'Initial release: everything up to the tag, tagged %s.\n\n' "$(tag_date "$release_tag")"
    fi
    list=$(commits_for "$(tag_range "$release_tag")")
    if [ -z "$list" ]; then
        printf 'No commits in this range.\n\n'
    else
        emit_groups "$list"
    fi
    printf -- '---\n\n'
    printf 'Built by `.github/workflows/release.yml` from `%s`.\n\n' \
        "$(git rev-parse --short "$release_tag^{commit}")"
    printf '### Assets\n\n'
    printf -- '- macOS / Linux: `x-%s-Linux.tar.gz`, `x-%s-macOS.tar.gz`\n' \
        "$release_tag" "$release_tag"
    printf -- '- Windows: `x-%s-windows-x86_64-setup.exe` (Inno Setup installer)\n' "$release_tag"
    printf -- '- From source: `scripts/install.sh` / `scripts/install.ps1` (they build a release binary and place it on your PATH)\n\n'
    printf 'Exit codes, output formats and the destructive-action confirmation behaviour are pinned by `docs/contract.md`; `x --version-info` prints the contract version this build follows.\n'
}

# --- whole history ------------------------------------------------------------
emit_history() {
    printf '# Changelog\n\n'
    printf 'Generated by `scripts/changelog.sh` from the commit history. Do not edit by\n'
    printf 'hand: the next run overwrites this file, and an entry the generator does not\n'
    printf 'produce is a claim nobody can trace back to a commit.\n\n'

    newest=$(git tag --list 'v*' --sort=-v:refname --merged HEAD | head -1)
    if [ -n "$newest" ]; then
        pending_range="$newest..HEAD"
    else
        pending_range="HEAD"
    fi

    # The pending section is what a release is about to ship. Without
    # --heading it is honestly called Unreleased; with it, the file names the
    # version the tag will carry.
    if [ -n "$pending_heading" ]; then
        heading=$pending_heading
        date=$(tag_date HEAD)
    else
        heading="Unreleased"
        date=""
    fi

    list=$(commits_for "$pending_range")
    if [ -n "$list" ]; then
        emit_section "$heading" "$date" "$list"
    elif [ -n "$pending_heading" ]; then
        printf '## %s\n\nNo commits since the previous release.\n\n' "$pending_heading"
    fi

    for tag in $(git tag --list 'v*' --sort=-v:refname --merged HEAD); do
        emit_section "$tag" "$(tag_date "$tag")" "$(commits_for "$(tag_range "$tag")")"
    done
}

# Both documents honour -o, so the workflow and a local run share one code path.
emit_document() {
    if [ -n "$release_tag" ]; then
        emit_release
    else
        emit_history
    fi
}

if [ -n "$output" ]; then
    # Write through a temporary file so a failure leaves the old document
    # intact instead of half written.
    temporary="$output.tmp.$$"
    trap 'rm -f "$temporary"' EXIT
    emit_document >"$temporary"
    mv -f "$temporary" "$output"
    printf 'wrote %s\n' "$output" >&2
else
    emit_document
fi
