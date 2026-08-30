FROM archlinux:base-devel AS base

RUN pacman -Syu --noconfirm

# Install dependencies needed by all steps including runtime step
RUN pacman -S --noconfirm --needed python python-pip git clang ffms2 ffmpeg mkvtoolnix-cli aom svt-av1 rav1e libvpx
# Install Python runtime dependencies system-wide so they are available to the app
RUN python -m pip install --no-cache-dir --break-system-packages vsjetpack[full]==2.2.4 vsfgs==0.7.0 --extra-index-url https://jaded-encoding-thaumaturgy.github.io/vs-wheels/simple

# Add extra plugins to ENV to cover VS R74 packaging changes
ENV VAPOURSYNTH_EXTRA_PLUGIN_PATH="/usr/lib/vapoursynth"

# Install ZooMVTools with generic linux binary
RUN ZOOMVTOOLS_VERSION="v2.0.2" && \
    PLUGIN_DIR="$(python -c 'import site; print(site.getsitepackages()[0])')/vapoursynth/plugins" && \
    mkdir -p "$PLUGIN_DIR" && \
    curl -fL -o "$PLUGIN_DIR/libzoomvtools.so" \
        "https://gitlab.com/api/v4/projects/78027771/packages/generic/vapoursynth-zoomvtools/${ZOOMVTOOLS_VERSION}/vapoursynth-zoomvtools-${ZOOMVTOOLS_VERSION}-linux-x86_64.so"

FROM base AS build-base

# Install dependencies needed by build steps
RUN pacman -S --noconfirm --needed vapoursynth rust nasm git

RUN cargo install cargo-chef
WORKDIR /tmp/Condor


FROM build-base AS planner

COPY . .
RUN cargo chef prepare


FROM build-base AS build

COPY --from=planner /tmp/Condor/recipe.json recipe.json
RUN cargo chef cook --release

# Build Condor California
COPY . /tmp/Condor

RUN cargo build --release -p california-condor && \
    mv ./target/release/condor /usr/local/bin && \
    cd .. && rm -rf ./Condor


FROM base AS runtime

ENV MPLCONFIGDIR="/home/app_user/"

COPY --from=build /usr/local/bin/condor /usr/local/bin/condor

# Create user
RUN useradd -ms /bin/bash app_user
USER app_user

VOLUME ["/videos"]
WORKDIR /videos

ENTRYPOINT [ "/usr/local/bin/condor" ]
