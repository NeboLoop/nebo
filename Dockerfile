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
RUN rustup component add rustfmt   # whisper-rs-sys bindgen needs it
RUN cargo install cargo-chef --locked
WORKDIR /src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
# `-l openblas` is REQUIRED — turbovec's `cblas_sgemm` reference otherwise goes
# unresolved at link time (blas-src is a link-only shim the linker drops).
# `--no-as-needed` forces the lib to stay on the link line regardless of
# placement; flip back to `--as-needed` so unrelated libs still get pruned.
# Same flags as .github/workflows/release.yml (tested on arm64 + amd64).
COPY --from=planner /src/recipe.json recipe.json
RUN MULTIARCH=$(dpkg-architecture -qDEB_HOST_MULTIARCH) && \
    export RUSTFLAGS="-C link-arg=-L/usr/lib/${MULTIARCH} -C link-arg=-Wl,--no-as-needed -C link-arg=-lopenblas -C link-arg=-Wl,--as-needed" && \
    cargo chef cook --profile server --recipe-path recipe.json -p nebo-cli
COPY . .
RUN test -d app/build || { echo "app/build missing — run 'cd app && pnpm build' on the host first"; exit 1; }
RUN MULTIARCH=$(dpkg-architecture -qDEB_HOST_MULTIARCH) && \
    export RUSTFLAGS="-C link-arg=-L/usr/lib/${MULTIARCH} -C link-arg=-Wl,--no-as-needed -C link-arg=-lopenblas -C link-arg=-Wl,--as-needed" && \
    cargo build --profile server -p nebo-cli

FROM debian:bookworm-slim
# Runtime .so for the GUI crates the binary links (loaded but unused on a server).
# Runtime .so set matches ldd of the binary — nothing speculative.
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates libssl3 libopenblas0-pthread \
      libwayland-client0 libxkbcommon0 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -u 1000 -m nebo
COPY --from=build /src/target/server/nebo-cli /usr/local/bin/nebo-cli
USER 1000
ENV NEBO_HOST=0.0.0.0 NEBO_DATA_DIR=/data NEBO_SERVER_MODE=1
EXPOSE 27895
ENTRYPOINT ["nebo-cli", "serve"]
