#!/bin/bash
# .SYNOPSIS
#     Builds fmetrics from source and installs the shared library for Linux.
#
# .DESCRIPTION
#     Installs fmetrics so av-metrics-fmetrics can load it at runtime with
#     `libloading`. fmetrics publishes no prebuilt Linux binaries and nothing
#     installable from a package mirror, so this fetches a pinned commit and
#     compiles it with Zig.
#
#     That makes this the one install script here that needs a toolchain: Zig
#     0.16.x, Git and a C compiler. Zig 0.17 is refused because it removed
#     `b.build_root`, which the pinned `build.zig` still uses.
#
#     Built from BoatsMcGee/fmetrics rather than halidecx/fmetrics, which
#     carries the shared-library target upstream lacks. The fcvvdp patch the
#     Windows script applies is not needed here: it works around POSIX
#     `sysconf()`, which does not compile on Windows and does on Linux.
#
#     `zig build -Dshared=true` is deliberately not used. It links the static
#     archive the ordinary way, so the linker pulls in only those members that
#     resolve an undefined symbol, and nothing references them. That leaves a
#     2.5 KB library exporting nothing at all. Windows is unaffected because
#     `win32_module_definition` (src/fmetrics.def) forces the members in; ELF
#     gets no export list, so the failure is Linux-only. Zig 0.16 offers no
#     whole-archive option, so the library is linked here instead.
#
# .PARAMETER Destination
#     Where to place `libfmetrics.so`. Defaults to `$PWD/fmetrics`.
#
# .PARAMETER Commit
#     The commit to build. Defaults to the pinned revision of the branch below.
#
# .PARAMETER Quiet
#     Suppress the environment-variable guidance printed at the end.
#
# .EXAMPLE
#     ./install-fmetrics-linux.sh
#
# .EXAMPLE
#     ./install-fmetrics-linux.sh -Destination /usr/lib -Quiet
set -euo pipefail

# The commit is pinnable from the environment so a caller (the Dockerfile) can
# set it; the default is what a user gets.
Destination="$PWD/fmetrics"
Commit="${FMETRICS_COMMIT:-ae87c8e5607063f0dcc8241b52cb143f6c9ad4ac}"
Quiet=0

while [ $# -gt 0 ]; do
    case "$1" in
        -Destination) Destination="$2"; shift 2 ;;
        -Commit)      Commit="$2";      shift 2 ;;
        -Quiet)       Quiet=1;          shift ;;
        -h|--help)    sed -n '2,38p' "$0"; exit 0 ;;
        *)            echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done

Repository='https://github.com/BoatsMcGee/fmetrics.git'
PatchRef='add-windows-shared-build'
RequiredZigMajor='0.16'

library=libfmetrics.so

# The branch head carries the shared-library target, so it is built rather than
# the upstream commit -- but the pin is checked anyway, so a later push cannot
# silently change what is installed.
work="$(mktemp -d "${TMPDIR:-/tmp}/fmetrics-build-XXXXXX")"

step() { echo "==> $1"; }

require() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "$2" >&2
        exit 1
    }
}

require git "git not found; it is needed to fetch a pinned commit."
require zig "zig not found. Install Zig ${RequiredZigMajor}.x from https://ziglang.org/download/."
require gcc "gcc not found; it links the shared library."

zig_version="$(zig version)"
if ! printf '%s' "$zig_version" | grep -q "^${RequiredZigMajor}\."; then
    echo "zig $zig_version found, but this needs ${RequiredZigMajor}.x: 0.17 removed" >&2
    echo "b.build_root, which the pinned build.zig uses." >&2
    exit 1
fi
echo "    zig $zig_version"

trap 'rm -rf "$work"' EXIT

step "Fetching fmetrics $PatchRef"
git clone --quiet --branch "$PatchRef" "$Repository" "$work"

actual="$(git -C "$work" rev-parse HEAD)"
if [ "$actual" != "$Commit" ]; then
    echo "$PatchRef is at $actual, not the expected $Commit. Rebase the branch, or" >&2
    echo "pass -Commit to build a different one deliberately." >&2
    exit 1
fi

step 'Fetching dependencies'
# `--fetch` populates zig-pkg/ without compiling. The Windows script needs it
# ordered this way so fcvvdp can be patched before it compiles; there is no patch
# here, but the two-step form is kept so both scripts fetch identically.
(cd "$work" && zig build --fetch)

step 'Building libfmetrics.a'
(cd "$work" && zig build --release=fast)

mkdir -p "$Destination"
target="$Destination/$library"

step "Linking $target"
# -lm and -lpthread are what the archive's cvvdp and workspace code need.
# Without -lm the library still loads, but fails on an undefined logf.
gcc -shared -fPIC -o "$target" \
    -Wl,--whole-archive "$work/zig-out/lib/libfmetrics.a" \
    -Wl,--no-whole-archive \
    -lm -lpthread

# The failure this link works around is a silently empty library, so the exports
# are asserted rather than assumed. Counted into a variable rather than piped to
# `grep -q`, which exits at the first match and leaves nm killed by SIGPIPE --
# under `pipefail` that is indistinguishable from "no match".
exports="$(nm -D --defined-only "$target" | grep -c ' T fmetrics_' || true)"
if [ "$exports" -lt 10 ]; then
    echo "$target exports only $exports fmetrics_* symbols." >&2
    echo "The whole-archive link did not pull the API in." >&2
    exit 1
fi

size="$(stat -c %s "$target")"

echo
echo 'Done.'
echo "  $library : $target ($size bytes, $exports exports)"
echo "  commit   : $Commit"

if [ "$Quiet" -eq 0 ]; then
    cat <<EOF

Set this environment variable to use the install:

  export FMETRICS_LIB_DIR='$Destination'

Not needed when the library sits beside the condor binary, which is where a
release stages it; the crate searches there on its own.

Verify with:

  cargo test -p av-metrics-fmetrics

Those tests skip themselves when the library is absent, so a pass alone does not
prove it loaded.
EOF
fi
