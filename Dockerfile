FROM archlinux:base-devel AS base

RUN pacman -Syu --noconfirm

# Install dependencies needed by all steps including runtime step
RUN pacman -S --noconfirm --needed python python-pip git ffms2 ffmpeg mkvtoolnix-cli aom svt-av1 rav1e libvpx
# Install Python runtime dependencies system-wide so they are available to the app
RUN python -m pip install --no-cache-dir --break-system-packages vsjetpack[full]==2.2.1 vsfgs==0.7.0 --extra-index-url https://jaded-encoding-thaumaturgy.github.io/vs-wheels/simple

# Add extra plugins to ENV to cover VS R74 packaging changes
ENV VAPOURSYNTH_EXTRA_PLUGIN_PATH="/usr/lib/vapoursynth"

# Create non-root user to install AUR packages
RUN useradd -m -G wheel aur && \
    echo "aur ALL=(ALL) NOPASSWD: ALL" >> /etc/sudoers

USER aur
WORKDIR /home/aur

# Clone and install paru
RUN git clone https://aur.archlinux.org/paru.git && cd paru && makepkg -si --noconfirm && cd .. && rm -rf paru
# Install ZooMVTools
RUN paru -S --noconfirm --needed vapoursynth-plugin-zoomvtools-git

USER root

FROM base AS build-base

# Install dependencies needed by build steps
RUN pacman -S --noconfirm --needed vapoursynth rust clang nasm git

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
