#!/bin/bash
# .SYNOPSIS
#     Writes the release body for a build or tag into $GITHUB_OUTPUT, for both
#     the Linux and the Windows workflow.
#
# .DESCRIPTION
#     Shared by linux-build.yml and windows-build.yml. Emits the release body as
#     `body<<CHANGELOG_EOF ... CHANGELOG_EOF`.
#
#     The body lists the built commit and the commits since the previous release, the
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

shallow="$(git rev-parse --is-shallow-repository)"
if [ "$shallow" = true ]; then
    echo 'Cannot generate a changelog from shallow history; fetch full history and tags (fetch-depth: 0).' >&2
    exit 1
fi
head="$(git rev-parse --verify 'HEAD^{commit}')"
tags="$(git tag --merged "$head")"
matches=()
while IFS= read -r tag; do
    # CONTRACT: Release tags are vN.N.N; ignore latest and every alias of HEAD.
    [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || continue
    tag_commit="$(git rev-parse --verify "refs/tags/$tag^{commit}")"
    if [ "$tag_commit" != "$head" ]; then
        matches+=(--match "$tag")
    fi
done <<< "$tags"

if [ "${#matches[@]}" -gt 0 ]; then
    # Search all parents and candidates, not only the first parent or newest ten tags.
    last_tag="$(git describe --tags --abbrev=0 --candidates="$((${#matches[@]} / 2))" "${matches[@]}" "$head")"
    revision="refs/tags/$last_tag..$head"
else
    last_tag='the beginning of history'
    revision="$head"
fi

all="$(git log "$revision" --pretty=format:'- %s' --)"
commit_count="$(git rev-list --count "$revision" --)"
first10="$(printf '%s\n' "$all" | sed -n '1,10p')"
rest_count=$((commit_count > 10 ? commit_count - 10 : 0))
rest="$(printf '%s\n' "$all" | tail -n +11)"
commit="$(git log -1 --pretty=format:'%h: %s (%an)' "$head" --)"

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
        echo # Keep the output delimiter on its own line even without a final notes newline.
    fi
    echo 'CHANGELOG_EOF'
} >> "$GITHUB_OUTPUT"

echo "    since $last_tag, $rest_count commits past the first ten"
