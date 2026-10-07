#!/bin/bash
# .SYNOPSIS
#     Stages the native libraries a Linux release needs beside the binary.
#
# .DESCRIPTION
#     The Linux mirror of stage-native-libs.ps1.
#
#     Three libraries travel with the release:
#       * libvmaf.so, from the distribution's `vmaf` package, so the archive is
#         self-contained.
#       * libvship.so, built from a pinned commit; upstream publishes no Linux
#         binaries.
#       * libfmetrics.so, built the same way: the CPU fallback for SSIMULACRA2,
#         Butteraugli and CVVDP.
#
#     FFMS2 and VapourSynth are linked into the executable at build time.
#
# .PARAMETER StageDir
#     Where to place the libraries. Defaults to `target/release`, beside the
#     binary cargo is about to produce.
#
# .EXAMPLE
#     ./.github/scripts/stage-native-libs.sh target/release
set -euo pipefail

# Load the shared pins (Vship, fmetrics) from the single CI/CD file so this and
# the Docker image cannot drift. The install scripts are standalone and pick
# these up from the environment.
# shellcheck source=/dev/null
set -a
. "$(dirname "${BASH_SOURCE[0]}")/../.env"
set +a

StageDir="${1:-target/release}"

step() { echo "==> $1"; }

# model/ is gitignored, so the checkout usually has none; the `vmaf` package
# installs the same files under /usr/share.
step 'Staging VMAF models'
mkdir -p "$StageDir/model"

MODEL_DIR="${GITHUB_WORKSPACE:-$(pwd)}/model"
if [ ! -d "$MODEL_DIR" ]; then
    for candidate in /usr/share/model /usr/share/vmaf/model /usr/local/share/model; do
        if [ -d "$candidate" ]; then
            MODEL_DIR="$candidate"
            break
        fi
    done
fi

# vmaf_v0.6.1.json is the default model; a source without it is unusable.
if [ ! -f "$MODEL_DIR/vmaf_v0.6.1.json" ]; then
    echo "ERROR: no VMAF models in $MODEL_DIR." >&2
    exit 1
fi

echo "Model directory: $MODEL_DIR"
models=("$MODEL_DIR"/*.json)
cp -v "${models[@]}" "$StageDir/model/"

# The first libvmaf in the loader's cache is what a bare `-lvmaf` resolves to;
# awk reads the whole cache to find it.
step 'Staging libvmaf'
vmaf_path="$(ldconfig -p | awk -F ' => ' '/libvmaf\.so/ { if (!path) path = $2 } END { print path }')"
if [ -z "$vmaf_path" ]; then
    echo "ldconfig found no libvmaf. Install the vmaf package first." >&2
    exit 1
fi
cp -v "$vmaf_path" "$StageDir/"

# Each install script pins its own upstream commit, the same default the image
# builds.
step 'Building libvship'
bash ./av-metrics-vship/scripts/install-libvship-linux.sh -Destination "$StageDir" -Quiet

step 'Building fmetrics'
bash ./av-metrics-fmetrics/scripts/install-fmetrics-linux.sh -Destination "$StageDir" -Quiet

# `ls` fails the step if any of the three is missing.
echo
echo 'Staged:'
ls -l "$StageDir"/libvmaf.so* "$StageDir"/libvship.so "$StageDir"/libfmetrics.so
