#!/bin/bash
# .SYNOPSIS
#     Writes the release body for a build or tag into $GITHUB_OUTPUT, for both
#     the Linux and the Windows workflow.
#
# .DESCRIPTION
#     Shared by linux-build.yml and windows-build.yml, which otherwise keep
#     their own copies of this logic in line. Emits the release body as
#     `body<<CHANGELOG_EOF ... CHANGELOG_EOF`.
#
#     The body lists the built commit and the commits since the last tag, the
#     first ten inline and the rest in a collapsible section. When an argument
#     names an existing file, its contents follow the changes as release-
#     specific notes, like the Linux host requirements.
#
#     Requires a full fetch (fetch-depth: 0): the tag lookup walks history.
#     Run with `id: changelog`; the release step reads outputs.body.
#
# .PARAMETER NotesFile
#     Optional file appended to the body verbatim after the changes.
#
# .EXAMPLE
#     ./.github/scripts/generate-changelog.sh
set -euo pipefail

NotesFile="${1:-}"

step() { echo "==> $1"; }

step 'Generating the changelog'

last_tag="$(git describe --tags --abbrev=0 2>/dev/null || true)"
if [ -z "$last_tag" ]; then
    last_tag="$(git rev-list --max-parents=0 HEAD 2>/dev/null || true)"
fi
if [ -z "$last_tag" ]; then
    last_tag='HEAD'
fi

all="$(git log "$last_tag..HEAD" --pretty=format:'- %s' 2>/dev/null || true)"
if [ -z "$all" ]; then
    all="No changes since $last_tag"
fi

first10="$(echo "$all" | head -10 || true)"
rest_count="$(echo "$all" | tail -n +11 | wc -l || true)"
rest="$(echo "$all" | tail -n +11 || true)"
commit="$(git log -1 --pretty=format:'%h: %s (%an)')"

# Step outputs are written as a `name<<delimiter` block.
{
    echo 'body<<CHANGELOG_EOF'
    echo "Latest build from $GITHUB_SHA"
    echo
    echo "Commit:"
    echo "$commit"
    echo
    echo "Changes since $last_tag:"
    echo "$first10"
    if [ "$rest_count" -gt 0 ]; then
        echo
        echo "<details>"
        echo "<summary>... and $rest_count more commits</summary>"
        echo
        echo "$rest"
        echo "</details>"
    fi
    if [ -n "$NotesFile" ] && [ -f "$NotesFile" ]; then
        echo
        echo "---"
        echo
        cat "$NotesFile"
    fi
    echo 'CHANGELOG_EOF'
} >> "$GITHUB_OUTPUT"

echo "    since $last_tag, $rest_count commits past the first ten"
