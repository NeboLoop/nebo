# Headless Nebo server image (one per SaaS tenant).
# Frontend is built on the host (pnpm) and embedded via rust-embed; this image
# compiles the binary and copies the prebuilt app/build in.
# ponytail: host-built frontend dodges pnpm-in-Docker; CI can add a node stage later.

FROM rust:1-bookworm AS build
# Build deps. nebo-cli is not cleanly headless — it compiles the Linux desktop
# GUI crates (clipboard/input/tray via wayland/x11/gtk/dbus) even though the
# server never uses them — so the full cluster is required to link.
# ponytail: bloated build deps to avoid feature-gating the workspace; revisit if
# a headless build feature ever lands.
RUN apt-get update && apt-get install -y --no-install-recommends \
      cmake clang libclang-dev pkg-config protobuf-compiler \
      libssl-dev libasound2-dev libdbus-1-dev \
      libwayland-dev libxkbcommon-dev \
      libx11-dev libxcb1-dev libxrandr-dev libxi-dev libxtst-dev libxdo-dev \
      libgtk-3-dev libayatana-appindicator3-dev \
    && rm -rf /var/lib/apt/lists/*
RUN rustup component add rustfmt   # whisper-rs-sys bindgen needs it
WORKDIR /src
COPY . .
RUN test -d app/build || { echo "app/build missing — run 'cd app && pnpm build' on the host first"; exit 1; }
RUN cargo build --release -p nebo-cli

FROM debian:bookworm-slim
# Runtime .so for the GUI crates the binary links (loaded but unused on a server).
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates libssl3 libasound2 libdbus-1-3 \
      libwayland-client0 libxkbcommon0 \
      libx11-6 libxcb1 libxrandr2 libxi6 libxtst6 libxdo3 \
      libgtk-3-0 libayatana-appindicator3-1 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -u 1000 -m nebo
COPY --from=build /src/target/release/nebo-cli /usr/local/bin/nebo-cli
USER 1000
ENV NEBO_HOST=0.0.0.0 NEBO_DATA_DIR=/data
EXPOSE 27895
ENTRYPOINT ["nebo-cli", "serve"]
