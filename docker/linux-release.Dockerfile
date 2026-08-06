# Local Linux release builder — mirrors .github/workflows/release.yml build-linux.
# Exists so a GitHub Actions outage can never block a Linux desktop release:
# the same artifacts (bare binary, headless CLI, .deb, AppImage) come out of
# `scripts/release-linux-docker.sh` on any machine with Docker.
FROM --platform=linux/amd64 ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
# Same dependency list as the CI job, verbatim.
RUN apt-get update && apt-get install -y \
    libwebkit2gtk-4.1-dev \
    libgtk-3-dev \
    libappindicator3-dev \
    librsvg2-dev \
    pkg-config \
    build-essential \
    protobuf-compiler \
    libopenblas-dev \
    libfuse2 \
    libssl-dev \
    curl git file ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Rust toolchain pinned to the workflow's version.
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.95 --profile minimal

# tauri-cli via prebuilt binary (cargo install takes ~10 min emulated).
RUN curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash \
    && cargo binstall -y tauri-cli --version "^2" || cargo install tauri-cli --version "^2" --locked

WORKDIR /work
