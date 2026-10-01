# syntax=docker/dockerfile:1
# Headless Nebo server image (one per SaaS tenant).
# Frontend is built on the host (pnpm) and embedded via rust-embed; this image
# compiles the binary and copies the prebuilt app/build in.
# ponytail: host-built frontend dodges pnpm-in-Docker; CI can add a node stage later.

# cargo-chef splits the build so DEPENDENCIES compile in a Docker layer keyed
# only by Cargo.lock/manifests: a source-only commit reuses the cached dep layer
# (including the whisper.cpp cmake build) and recompiles just the workspace —
# ~35min cold → single-digit minutes warm under the CI layer cache.
FROM rust:1-bookworm AS chef
# Build deps. nebo-cli is not cleanly headless — it compiles the Linux desktop
# GUI crates (clipboard/input/tray via wayland/x11/gtk/dbus) even though the
# server never uses them — so the full cluster is required to link.
# ponytail: bloated build deps to avoid feature-gating the workspace; revisit if
# a headless build feature ever lands.
# Empirical dependency set: `ldd nebo-cli` in the shipped image links ONLY
# libc/ssl/crypto, the OpenBLAS chain, libstdc++ (whisper.cpp), and
# libwayland-client. The previous gtk/x11/dbus/asound list was copied from the
# desktop (Tauri) docs and nothing in this binary ever linked it.
RUN apt-get update && apt-get install -y --no-install-recommends \
      cmake clang libclang-dev pkg-config protobuf-compiler \
      libssl-dev libopenblas-dev libwayland-dev libxkbcommon-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# The toolchain pinned in rust-toolchain.toml, installed HERE, once, in the
# cached base. Without it the cook step below compiled every dependency with
# the image's stable rustc, then the workspace build (which sees the pin)
# downloaded 1.95 and compiled every dependency AGAIN — the dep layer never
# saved a minute.
COPY rust-toolchain.toml .
RUN rustup toolchain install \
    && rustup component add rustfmt   # whisper-rs-sys bindgen needs it
RUN cargo install cargo-chef --locked
# sccache, prebuilt and checksum-pinned: the shared compile cache (see
# docker/server-cargo.sh).
ARG SCCACHE_VERSION=0.18.0
RUN set -eu; \
    case "$(uname -m)" in \
      x86_64) sum=45f1447fbe231e3037bde351ef70677dd212216c8d62ae7ca409fecc4d6acc89 ;; \
      aarch64) sum=2b3284d5da3b46a47dc4229e75bb7b88ac4aa99c8d754fb7d2f84997e5a4354a ;; \
      *) echo "no sccache build for $(uname -m)"; exit 1 ;; \
    esac; \
    name="sccache-v${SCCACHE_VERSION}-$(uname -m)-unknown-linux-musl"; \
    curl -fsSL -o /tmp/sccache.tgz "https://github.com/mozilla/sccache/releases/download/v${SCCACHE_VERSION}/${name}.tar.gz"; \
    echo "${sum}  /tmp/sccache.tgz" | sha256sum -c -; \
    tar -xzf /tmp/sccache.tgz -C /tmp; \
    install -m 0755 "/tmp/${name}/sccache" /usr/local/bin/sccache; \
    rm -rf /tmp/sccache.tgz "/tmp/${name}"
COPY docker/server-cargo.sh /usr/local/bin/server-cargo

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
# Compile cache (docker/server-cargo.sh). The bucket coordinates are build args;
# the keys arrive only as BuildKit secrets, which never enter a layer or the
# layer cache key. A build without them runs uncached, exactly as before.
ARG SCCACHE_BUCKET=""
ARG SCCACHE_ENDPOINT=""
ARG SCCACHE_REGION=""
ARG SCCACHE_S3_KEY_PREFIX=""
ENV SCCACHE_BUCKET=${SCCACHE_BUCKET} SCCACHE_ENDPOINT=${SCCACHE_ENDPOINT} \
    SCCACHE_REGION=${SCCACHE_REGION} SCCACHE_S3_KEY_PREFIX=${SCCACHE_S3_KEY_PREFIX} \
    SCCACHE_S3_USE_SSL=true
COPY --from=planner /src/recipe.json recipe.json
RUN --mount=type=secret,id=sccache_key,env=AWS_ACCESS_KEY_ID \
    --mount=type=secret,id=sccache_secret,env=AWS_SECRET_ACCESS_KEY \
    server-cargo cargo chef cook --profile server --recipe-path recipe.json -p nebo-cli
COPY . .
RUN test -d app/build || { echo "app/build missing — run 'cd app && pnpm build' on the host first"; exit 1; }
# The binary moves out and target/ goes in the same step: this layer then holds
# one file instead of gigabytes of intermediate artifacts that change on every
# commit, which is what made exporting the layer cache take five minutes.
RUN --mount=type=secret,id=sccache_key,env=AWS_ACCESS_KEY_ID \
    --mount=type=secret,id=sccache_secret,env=AWS_SECRET_ACCESS_KEY \
    server-cargo cargo build --profile server -p nebo-cli \
    && install -m 0755 target/server/nebo-cli /usr/local/bin/nebo-cli \
    && rm -rf target

FROM debian:bookworm-slim
# Runtime .so for the GUI crates the binary links (loaded but unused on a server).
# Runtime .so set matches ldd of the binary — nothing speculative.
#
# Below the .so line: the agent's toolbox. A cloud pod is the employee's
# computer: the standard dev/scripting tools ship here (git, curl, Python 3
# with pip and venv, build-essential, jq, unzip, …) + media processing
# (ffmpeg pairs with audio/video attachments). Node is the current LTS from
# the official image (below), not Debian's 18. Anything else the employee
# installs itself: `sudo apt-get install` is the one command sudo runs
# (assets/cloud-bot/sudoers), and Nebo puts those packages back after every
# restart (crates/tools/src/system_packages.rs).
#
# The desktop rows (xvfb…fonts) are the bot's on-demand computer: a curated
# xfce subset (NOT the xfce4 metapackage — no screensaver/power-manager),
# x11vnc for the live view, xdotool/wmctrl/scrot/xclip/at-spi2-core so the
# desktop tool can drive it, python3-gi + gir1.2-atspi-2.0 so the native
# accessibility walk (crates/tools/src/ax_native/ax_helper_atspi.py) can read
# the tree — without them every window is vision-only; tesseract reads the
# text the tree does not expose (the OCRText elements);
# existing desktop-tool Linux backend works against the session's DISPLAY.
# Nothing starts at boot; nebo-server spawns the tree on demand.
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates libssl3 libopenblas0-pthread \
      libwayland-client0 libxkbcommon0 \
      git openssh-client curl wget \
      python3 python3-pip python3-venv python3-dev \
      build-essential pkg-config \
      sudo \
      jq unzip zip ripgrep less procps sqlite3 \
      ffmpeg \
      xvfb x11vnc xdotool wmctrl scrot xclip x11-utils x11-xserver-utils xinput dbus-x11 at-spi2-core \
      python3-gi gir1.2-atspi-2.0 tesseract-ocr tesseract-ocr-eng \
      xfwm4 xfce4-panel xfce4-terminal thunar adwaita-icon-theme \
      chromium \
      fonts-dejavu fonts-liberation fonts-noto-color-emoji \
      zsh \
    && rm -rf /var/lib/apt/lists/* \
    && echo 'debconf debconf/frontend select Noninteractive' | debconf-set-selections \
    && git clone --depth=1 https://github.com/ohmyzsh/ohmyzsh.git /usr/share/oh-my-zsh \
    && useradd -u 1000 -m -s /usr/bin/zsh nebo
# Node LTS with npm, from the official image of the same Debian release.
COPY --from=node:24-bookworm-slim /usr/local/bin/node /usr/local/bin/node
COPY --from=node:24-bookworm-slim /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/npm
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx
# The installer, and nothing else, through sudo. visudo refuses a file sudo
# would reject, so a broken rule fails the build, not the bot.
COPY assets/cloud-bot/sudoers /etc/sudoers.d/nebo-packages
RUN chmod 0440 /etc/sudoers.d/nebo-packages && visudo -cf /etc/sudoers.d/nebo-packages
COPY --from=build /usr/local/bin/nebo-cli /usr/local/bin/nebo-cli
# The computer's face: a 3-launcher dock (Chromium / Terminal / Files) instead
# of xfce's placeholder gears, and the default ~/.zshrc oh-my-zsh seed. The
# panel default applies only until the user customizes (their config lands on
# the persistent volume and wins).
COPY assets/cloud-desktop/xfce4-panel.xml /etc/xdg/xfce4/xfconf/xfce-perchannel-xml/xfce4-panel.xml
COPY assets/cloud-desktop/xfce4-panel.xml /etc/nebo/desktop-skel/xfce4-panel.xml
COPY assets/cloud-desktop/zshrc /etc/nebo/zshrc
# $HOME lives under /data. What the employee installs for itself lands in
# /data/toolchains (the ENV below points npm -g, pip --user, cargo, rustup
# and go there), which is part of the bot's state whether /data is its own
# volume or scratch restored from the state commit: an ephemeral install
# reads as "my tools vanished". nebo-entry moves toolchains an older image
# left in $HOME into place, once (a bot on its own volume kept them there).
COPY assets/cloud-bot/nebo-entry /usr/local/bin/nebo-entry
USER 1000
# pip installs land in the user base (PEP 668 would otherwise refuse outside a
# venv — a disposable per-tenant container is exactly the case where that's
# noise). The Go module cache is re-downloadable, so it stays scratch.
ENV NEBO_HOST=0.0.0.0 NEBO_DATA_DIR=/data NEBO_SERVER_MODE=1 \
    HOME=/data/home/nebo \
    NPM_CONFIG_PREFIX=/data/toolchains/npm \
    PYTHONUSERBASE=/data/toolchains/python \
    CARGO_HOME=/data/toolchains/cargo RUSTUP_HOME=/data/toolchains/rustup \
    GOPATH=/data/toolchains/go GOMODCACHE=/data/cache/go-mod \
    PIP_BREAK_SYSTEM_PACKAGES=1 \
    PATH="/data/toolchains/npm/bin:/data/toolchains/python/bin:/data/toolchains/cargo/bin:/data/toolchains/go/bin:/data/home/nebo/.local/bin:${PATH}"
EXPOSE 27895
ENTRYPOINT ["nebo-entry"]
