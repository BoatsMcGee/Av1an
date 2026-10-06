#!/bin/bash
# .SYNOPSIS
#     Stages the native libraries a Linux release needs beside the binary.
#
# .DESCRIPTION
#     The Linux mirror of stage-native-libs.ps1, and much simpler: there is no
#     MinGW runtime to chase, because nothing staged here imports one.
#
#     Three libraries travel with the release:
#       * libvmaf.so, copied from the distribution package so the archive is
#         self-contained rather than depending on the host having `vmaf`
#         installed. The models are copied from the repository's own model/
#         directory, so nothing is downloaded.
#       * libvship.so, built from a pinned commit. Upstream publishes no Linux
#         binaries at all, so this is the only way to ship it.
#       * libfmetrics.so, built from a pinned commit the same way. This is the
#         CPU fallback for SSIMULACRA2, Butteraugli and CVVDP, so a machine
#         with no usable GPU still scores.
#
#     FFMS2 and VapourSynth are deliberately not staged: they are linked into
#     the executable at build time, so the host provides them through its
#     package manager like every other Linux binary.
#
# .PARAMETER StageDir
#     Where to place the libraries. Defaults to `target/release`, beside the
#     binary cargo is about to produce.
#
# .EXAMPLE
#     ./.github/scripts/stage-native-libs.sh -StageDir target/release
set -euo pipefail

StageDir="${1:-target/release}"

step() { echo "==> $1"; }

mkdir -p "$StageDir/model"

# The nine upstream models are checked in under model/, so the release copies
# them rather than downloading from Netflix like the Windows script does.
step 'Staging VMAF models'
for model in model/*.json; do
    cp -v "$model" "$StageDir/model/"
done

# libvmaf comes from the distribution package, which the build container
# installs. The resolved path is copied rather than symlinked so the archive is
# self-contained; ldconfig reports the versioned file, which the crate's
# candidate list includes alongside the bare name.
step 'Staging libvmaf'
vmaf_path="$(ldconfig -p | grep -m1 'libvmaf\.so' | grep -o '/[^ ]*libvmaf\.so[^ ]*')"
if [ -z "$vmaf_path" ]; then
    echo "ldconfig found no libvmaf. Install the vmaf package first." >&2
    exit 1
fi
cp -v "$vmaf_path" "$StageDir/"

# libvship, built from a pinned commit. The pins match the Dockerfile's metrics
# stage so the release and the image cannot drift.
step 'Building libvship'
VSHIP_VERSION="${VSHIP_VERSION:-v5.1.1}" \
VSHIP_COMMIT="${VSHIP_COMMIT:-256dc5a85e56e42a88a7a90d641037fdc148b91e}" \
    bash ./av-metrics-vship/scripts/install-libvship-linux.sh -Destination "$StageDir" -Quiet

# fmetrics, built from a pinned commit the same way.
step 'Building fmetrics'
FMETRICS_COMMIT="${FMETRICS_COMMIT:-ae87c8e5607063f0dcc8241b52cb143f6c9ad4ac}" \
    bash ./av-metrics-fmetrics/scripts/install-fmetrics-linux.sh -Destination "$StageDir" -Quiet

echo
echo 'Staged:'
ls -l "$StageDir" | grep -E 'libvmaf|libvship|libfmetrics' || true
ls -l "$StageDir/model" | head -3
