#!/bin/bash
# .SYNOPSIS
#     Gives the docs the absolute links the published site cannot derive itself.
#
# .DESCRIPTION
#     The site is built from `california-condor/docs`, so a link that leaves that
#     tree has nothing to resolve against once mdBook has rewritten the `.md` in
#     it to `.html`: the screenshot media is generated during the build and never
#     committed, and the repository files the docs reference — the READMEs, the
#     metric install scripts — are not part of the site at all. Both 404.
#
#     Each destination is passed in rather than hardcoded:
#
#       * the site base comes from the deploy itself, so a fork's pages load, and
#         link through to, their own deploy, and a custom domain or a non-default
#         path needs no change here;
#       * the repository comes from the repository doing the build, for the same
#         reason, and is linked at `HEAD` so no branch name is assumed.
#
#     An absolute URL matches neither pattern, so running this twice is a no-op.
#     Links that stay inside `california-condor/docs` are left alone: mdBook
#     resolves those itself.
#
# .PARAMETER SiteBaseUrl
#     The published site's base URL, with or without a trailing slash, e.g.
#     `https://owner.github.io/repo/`.
#
# .PARAMETER RepositoryUrl
#     The repository's web URL, e.g. `https://github.com/owner/repo`.
#
# .EXAMPLE
#     ./.github/scripts/prepare-docs-for-deploy.sh \
#         https://owner.github.io/repo/ https://github.com/owner/repo
set -euo pipefail

SiteBaseUrl="${1:?usage: $0 <site-base-url> <repository-url>}"
RepositoryUrl="${2:?usage: $0 <site-base-url> <repository-url>}"

SiteBaseUrl="${SiteBaseUrl%/}/"
RepositoryUrl="${RepositoryUrl%/}"

DocsDir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../california-condor/docs" && pwd)"

# Resolving the relative targets is clearer, and easier to keep correct, in
# Python; the sibling scripts reach for it the same way.
python3 - "$DocsDir" "$SiteBaseUrl" "$RepositoryUrl" <<'PY'
import os
import re
import sys
from pathlib import Path

site_base, repository = sys.argv[2], sys.argv[3]
# Both sides go through Path, whose comparisons ignore which separator the shell
# handed us: `os.walk` reports forward slashes on Windows while `normpath`
# returns backslashes, and mixing them silently misreads every link as relative
# to nothing.
docs = Path(sys.argv[1]).resolve()
repo_root = docs.parent.parent

# The screenshot blocks are raw HTML, so the media is reached through an
# attribute; `../media/tui/` is how the nested pages spell the same thing.
MEDIA = re.compile(r'((?:href|src|srcset)=")(?:\.\./)?media/tui/')
LINK = re.compile(r'(!?)\[([^\]]*)\]\(([^)\s]+)\)')
ABSOLUTE = re.compile(r'[a-zA-Z][a-zA-Z0-9+.-]*:|^[#/]')

media = 0
outside = 0
files = 0


def inside(path, directory):
    """Whether `path` is `directory` itself or sits below it."""
    try:
        path.relative_to(directory)
    except ValueError:
        return False
    return True


def rewrite(text, directory):
    """Return `text` with both kinds of link given their absolute destination."""
    global media, outside

    def screenshot(match):
        global media
        media += 1
        return match.group(1) + site_base + "media/tui/"

    def leave_site(match):
        global outside
        target = match.group(3)
        if ABSOLUTE.match(target):
            return match.group(0)

        # The fragment is not part of the path, so keep it out of the lookups.
        location, _, fragment = target.partition("#")
        resolved = (directory / location).resolve()
        if inside(resolved, docs):
            # Inside the site, so mdBook resolves it and the site serves it.
            return match.group(0)
        if not inside(resolved, repo_root):
            print(f"{resolved} leaves the repository", file=sys.stderr)
            return match.group(0)
        if not resolved.exists():
            print(f"warning: {resolved} does not exist", file=sys.stderr)

        outside += 1
        suffix = f"#{fragment}" if fragment else ""
        path = resolved.relative_to(repo_root).as_posix()
        # `raw` keeps an image an image; a link opens the rendered file.
        kind = "raw" if match.group(1) else "blob"
        return f"{match.group(1)}[{match.group(2)}]({repository}/{kind}/HEAD/{path}{suffix})"

    return MEDIA.sub(screenshot, LINK.sub(leave_site, text))


for directory, _, names in os.walk(docs):
    directory = Path(directory)
    for name in sorted(names):
        if not name.endswith(".md"):
            continue
        files += 1
        path = directory / name
        # Keep the line endings exactly as committed: the docs are CRLF.
        with open(path, encoding="utf-8", newline="") as handle:
            original = handle.read()
        updated = rewrite(original, directory)
        if updated != original:
            with open(path, "w", encoding="utf-8", newline="") as handle:
                handle.write(updated)

if not files:
    sys.exit(f"no markdown found under {docs}")

print(f"==> Pointed {media} screenshot reference(s) at {site_base}")
print(f"==> Pointed {outside} link(s) that leave the site at {repository}")
PY
